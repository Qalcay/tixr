//! The Engine thread: owns the [`Engine`], paces it against the wall clock,
//! and talks to the Display thread only through channels.
//!
//! ```text
//!   Display thread                         Engine thread
//!   ──────────────                         ─────────────
//!   egui frame ──Command──▶ mpsc::Sender ─▶ drain commands
//!                                           advance sim to wall-clock target
//!                                           (10 ms budget per slice)
//!   latest()  ◀─Box<Snapshot>─ sync_channel ◀ every 100 ms: snapshot()
//! ```
//!
//! * Commands use an unbounded channel: they are tiny and rare.
//! * Snapshots use a bounded channel of 2. If the UI is slow, the engine
//!   skips a snapshot instead of queueing stale ones (back-pressure).
//! * The sim clock is tied to real time: `BASE_TIME_SCALE` (16 sim seconds
//!   per real second) times the user's speed multiplier.

use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError, TrySendError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::console::{self, Level};
use crate::engine::{Engine, EngineConfig};
use crate::protocol::{Command, Notice, Snapshot};
use crate::time::BASE_TIME_SCALE;

pub const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(100);
const SLICE_BUDGET: Duration = Duration::from_millis(10);
const IDLE_SLEEP: Duration = Duration::from_millis(4);
/// Extra speed while every exchange is closed.
pub const CLOSED_BOOST: f64 = 8.0;
pub const MAX_SPEED: f64 = 4_096.0;

pub struct EngineHandle {
    commands: Sender<Command>,
    snapshots: Receiver<Box<Snapshot>>,
    thread: Option<JoinHandle<()>>,
}

impl EngineHandle {
    pub fn send(&self, cmd: Command) {
        // A closed channel means the engine thread is gone; nothing to do.
        let _ = self.commands.send(cmd);
    }

    /// The newest snapshot, if one arrived since the last call.
    pub fn latest(&self) -> Option<Box<Snapshot>> {
        let mut newest: Option<Box<Snapshot>> = None;
        while let Ok(s) = self.snapshots.try_recv() {
            // Keep notices from skipped snapshots.
            if let Some(old) = newest.take() {
                let mut s = s;
                let mut notices = old.notices;
                notices.append(&mut s.notices);
                s.notices = notices;
                newest = Some(s);
            } else {
                newest = Some(s);
            }
        }
        newest
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Start the engine thread. `wake` is called after each snapshot is sent so
/// the UI can request a repaint (e.g. `egui::Context::request_repaint`).
pub fn spawn(config: EngineConfig, wake: impl Fn() + Send + 'static) -> EngineHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
    let (snap_tx, snap_rx) = mpsc::sync_channel::<Box<Snapshot>>(2);
    let thread = std::thread::Builder::new()
        .name("sigma-engine".into())
        .spawn(move || run(config, cmd_rx, snap_tx, wake))
        .expect("failed to spawn engine thread");
    EngineHandle { commands: cmd_tx, snapshots: snap_rx, thread: Some(thread) }
}

struct Pacing {
    speed: f64,
    paused: bool,
    closed_boost: bool,
}

fn run(config: EngineConfig, rx: Receiver<Command>, tx: SyncSender<Box<Snapshot>>, wake: impl Fn()) {
    let mut engine = Engine::new(config.clone());
    let mut pacing = Pacing { speed: 1.0, paused: false, closed_boost: true };
    let mut wanted = engine.now;
    let mut last = Instant::now();
    let mut last_snapshot = Instant::now() - SNAPSHOT_INTERVAL;
    let mut window_start = Instant::now();
    let mut window_busy = Duration::ZERO;
    let mut window_events = 0u64;
    let mut events_per_sec = 0.0;
    let mut last_health = Instant::now();
    let mut last_perf_log = Instant::now();
    let mut pending_notices: Vec<Notice> = Vec::new();

    loop {
        let mut force_snapshot = false;
        loop {
            match rx.try_recv() {
                Ok(cmd) => match cmd {
                    Command::Shutdown => return,
                    Command::SetSpeed(s) => pacing.speed = s.clamp(1.0 / 16.0, MAX_SPEED),
                    Command::SetPaused(p) => pacing.paused = p,
                    Command::SetClosedBoost(b) => pacing.closed_boost = b,
                    Command::SetLogLevel(l) => {
                        console::set_level(l);
                        crate::console!(Level::Warn, engine.now, "console level set to {}", l.label());
                    }
                    Command::Restart { seed } => {
                        engine = Engine::new(EngineConfig { seed, ..config.clone() });
                        wanted = engine.now;
                        force_snapshot = true;
                    }
                    other => {
                        engine.apply(&other);
                        force_snapshot = true;
                    }
                },
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }

        // Wall-clock pacing. Clamp the frame delta so a debugger pause or a
        // sleeping laptop does not try to simulate hours in one go.
        let now_real = Instant::now();
        let wall = (now_real - last).as_secs_f64().min(0.25);
        last = now_real;
        let boost_active = pacing.closed_boost && !engine.any_exchange_open();
        if !pacing.paused {
            let boost = if boost_active { CLOSED_BOOST } else { 1.0 };
            wanted += wall * BASE_TIME_SCALE * pacing.speed * boost;
        } else {
            wanted = engine.now;
        }
        // If we fall behind by more than 5 real seconds, drop the backlog:
        // the sim slows down honestly instead of spiralling.
        let max_lag = 5.0 * BASE_TIME_SCALE * pacing.speed;
        if wanted - engine.now > max_lag {
            wanted = engine.now + max_lag;
        }

        let slice_start = Instant::now();
        let adv = engine.advance_to(wanted, Some(slice_start + SLICE_BUDGET));
        window_busy += slice_start.elapsed();
        window_events += adv.events;

        // Adaptive print density: keep the engine under ~60% of one core.
        let window = window_start.elapsed();
        if window >= Duration::from_millis(500) {
            let util = window_busy.as_secs_f64() / window.as_secs_f64();
            if util > 0.6 {
                engine.tick_density = (engine.tick_density * 0.7).max(1e-4);
            } else if util < 0.25 {
                engine.tick_density = (engine.tick_density * 1.15).min(1.0);
            }
            events_per_sec = window_events as f64 / window.as_secs_f64();
            window_start = Instant::now();
            window_busy = Duration::ZERO;
            window_events = 0;
        }

        if last_perf_log.elapsed() >= Duration::from_secs(10) {
            crate::console!(
                Level::Debug,
                engine.now,
                "perf: {:.0} events/s, density {:.4}, lag {:.0} sim-s, live {}",
                events_per_sec,
                engine.tick_density,
                (wanted - engine.now).max(0.0),
                engine.live_count()
            );
            last_perf_log = Instant::now();
        }
        if last_health.elapsed() >= Duration::from_secs(5) {
            if let Err(e) = engine.health_check() {
                crate::console!(Level::Warn, engine.now, "health check failed: {e}");
            }
            last_health = Instant::now();
        }

        if force_snapshot || last_snapshot.elapsed() >= SNAPSHOT_INTERVAL {
            let mut snap = engine.snapshot();
            if !pending_notices.is_empty() {
                pending_notices.append(&mut snap.notices);
                snap.notices = std::mem::take(&mut pending_notices);
            }
            snap.speed = pacing.speed;
            snap.paused = pacing.paused;
            snap.closed_boost_active = boost_active && !pacing.paused;
            snap.lag_secs = (wanted - engine.now).max(0.0);
            snap.events_per_real_sec = events_per_sec;
            match tx.try_send(Box::new(snap)) {
                Ok(()) => wake(),
                Err(TrySendError::Full(s)) => pending_notices = s.notices,
                Err(TrySendError::Disconnected(_)) => return,
            }
            last_snapshot = Instant::now();
        }

        if adv.reached {
            std::thread::sleep(IDLE_SLEEP);
        }
    }
}
