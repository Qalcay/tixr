//! The engine thread.
//!
//! ## Why two threads and message passing
//! The display thread must hit its frame deadline (~16 ms) no matter what
//! the market does; the engine must keep simulated time in step with the
//! wall clock no matter how slow a frame is. Running them on separate
//! threads that share *nothing* and talk only through channels is the
//! standard Rust answer ("do not communicate by sharing memory; share
//! memory by communicating"): no locks, no data races, and the borrow
//! checker proves it. `std::sync::mpsc` is enough here; `crossbeam-channel`
//! (used by tixr alpha) is faster under heavy contention, but two threads
//! exchanging ~30 messages a second is nowhere near that regime.
//!
//! * UI to engine: an unbounded `Sender<Command>` (commands are tiny and rare).
//! * Engine to UI: a `sync_channel(2)` of `Frame`s. If the UI falls behind,
//!   `try_send` fails and the engine simply skips that frame instead of
//!   piling up memory. Latest state wins, which is what a display wants.
//!
//! ## Wall-clock pacing
//! Each loop iteration measures the real time since the previous one and
//! advances simulated time by `real_dt * 16 * speed`, so 90 real minutes are
//! one simulated day at 1x regardless of machine speed. (tixr alpha
//! advanced a fixed amount per loop, so its clock depended on the CPU.)

use crate::clock::BASE_SIM_RATE;
use crate::engine::{Config, Engine};
use crate::log::{self, Level};
use crate::protocol::{Command, Frame};
use crate::slog;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Engine loop rate. 30 Hz is plenty: prices are sampled, not animated.
const TICK: Duration = Duration::from_millis(33);
const FRAME_EVERY: Duration = Duration::from_millis(100);
const TABLE_EVERY: Duration = Duration::from_millis(400);
const STATUS_EVERY: Duration = Duration::from_secs(5);

pub struct EngineHandle {
    pub commands: Sender<Command>,
    pub frames: Receiver<Frame>,
    join: Option<JoinHandle<()>>,
}

impl EngineHandle {
    pub fn send(&self, cmd: Command) {
        // If the engine thread is gone the app is shutting down anyway.
        let _ = self.commands.send(cmd);
    }

    /// Drain the channel and return only the newest frame.
    pub fn latest(&self) -> Option<Frame> {
        let mut last = None;
        while let Ok(f) = self.frames.try_recv() {
            last = Some(f);
        }
        last
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Live settings owned by the loop (not by the engine, so a reset keeps them).
struct Pace {
    speed: f64,
    paused: bool,
    budget_ms: f64,
    /// Smoothed cost of one security update, in nanoseconds.
    ns_per_update: f64,
}

/// Spawn the engine thread. `wake` is called after each frame is sent so the
/// UI can repaint on demand (egui's `Context::request_repaint`).
pub fn spawn(cfg: Config, wake: impl Fn() + Send + 'static) -> EngineHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
    let (frame_tx, frame_rx) = mpsc::sync_channel::<Frame>(2);
    let join = thread::Builder::new()
        .name("sigma-engine".into())
        .spawn(move || run(cfg, cmd_rx, frame_tx, wake))
        .expect("failed to spawn engine thread");
    EngineHandle { commands: cmd_tx, frames: frame_rx, join: Some(join) }
}

fn run(cfg: Config, rx: Receiver<Command>, tx: SyncSender<Frame>, wake: impl Fn()) {
    log::init_from_env();
    let started = Instant::now();
    let mut engine = Engine::new(cfg);
    slog!(Level::Info, "runtime", "engine ready in {:.2}s", started.elapsed().as_secs_f64());
    let mut pace = Pace { speed: 1.0, paused: false, budget_ms: 1.5, ns_per_update: 400.0 };
    let mut last = Instant::now();
    let (mut last_frame, mut last_table, mut last_status) = (last, last, last);
    let mut status_updates: u64 = 0;
    let mut tick_ms_ema = 0.0;

    'outer: loop {
        let tick_start = Instant::now();
        for cmd in rx.try_iter() {
            match cmd {
                Command::Shutdown => break 'outer,
                Command::SetPaused(p) => pace.paused = p,
                Command::SetSpeed(s) => pace.speed = s.clamp(0.25, 1024.0),
                Command::SetBudgetMs(ms) => pace.budget_ms = ms.clamp(0.1, 20.0),
                Command::SetLogLevel(l) => log::set_level(l),
                Command::Reset { seed } => {
                    let mut c = engine.cfg.clone();
                    c.seed = seed;
                    engine = Engine::new(c);
                    slog!(Level::Info, "runtime", "reset with seed {seed:#x}");
                }
                other => engine.apply(other),
            }
        }
        // Clamp the real step so a laptop waking from sleep does not jump
        // the market by hours in one go.
        let now = Instant::now();
        let real_dt = (now - last).as_secs_f64().min(0.25);
        last = now;
        let sim_dt = if pace.paused { 0.0 } else { real_dt * BASE_SIM_RATE * pace.speed };

        // Adaptive budget: as many random updates as fit in `budget_ms`,
        // but never more than one visit per live security per sim second.
        let fit = (pace.budget_ms * 1.0e6 / pace.ns_per_update) as usize;
        let useful = (engine.live() as f64 * sim_dt.max(0.0)).ceil() as usize;
        let samples = fit.min(useful).clamp(if sim_dt > 0.0 { 32 } else { 0 }, 200_000);

        let work_start = Instant::now();
        let updated = engine.advance(sim_dt, samples);
        let work_ns = work_start.elapsed().as_nanos() as f64;
        if updated > 64 {
            let per = work_ns / updated as f64;
            pace.ns_per_update = 0.9 * pace.ns_per_update + 0.1 * per.clamp(20.0, 50_000.0);
        }
        status_updates += updated as u64;

        let now = Instant::now();
        if now - last_frame >= FRAME_EVERY {
            let with_table = now - last_table >= TABLE_EVERY;
            if with_table {
                engine.rebuild_table();
                last_table = now;
            }
            let mut f = engine.frame(with_table);
            f.clock.speed = pace.speed;
            f.clock.paused = pace.paused;
            f.stats.tick_ms = tick_ms_ema;
            f.stats.updates_per_sec = pace_rate(status_updates, now - last_status);
            match tx.try_send(f) {
                Ok(()) => wake(),
                Err(TrySendError::Full(_)) => {} // UI busy: drop, next one is fresher
                Err(TrySendError::Disconnected(_)) => break 'outer,
            }
            last_frame = now;
        }

        if now - last_status >= STATUS_EVERY {
            let secs = (now - last_status).as_secs_f64();
            let (n, mean_age, max_age) = engine.take_window();
            slog!(
                Level::Info,
                "engine",
                "{} | upd {:.0}/s ({:.0} ns each) samples/tick {} | age at update avg {:.1}m max {:.1}m | tick {:.2}ms x{}",
                engine.status_line(),
                n as f64 / secs,
                pace.ns_per_update,
                samples,
                mean_age / 60.0,
                max_age / 60.0,
                tick_ms_ema,
                pace.speed
            );
            if let Err(e) = engine.audit() {
                slog!(Level::Warn, "audit", "{e}");
            }
            status_updates = 0;
            last_status = now;
        }

        let spent = tick_start.elapsed();
        tick_ms_ema = 0.9 * tick_ms_ema + 0.1 * spent.as_secs_f64() * 1000.0;
        slog!(Level::Trace, "tick", "sim_dt {:.2}s samples {samples} updated {updated} in {:.3}ms", sim_dt, spent.as_secs_f64() * 1000.0);
        if let Some(rest) = TICK.checked_sub(spent) {
            thread::sleep(rest);
        }
    }
    slog!(Level::Info, "runtime", "engine thread stopped");
}

fn pace_rate(count: u64, over: Duration) -> f64 {
    let s = over.as_secs_f64();
    if s > 0.0 { count as f64 / s } else { 0.0 }
}
