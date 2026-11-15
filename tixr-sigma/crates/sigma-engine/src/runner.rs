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
//!   per real second). A release game is locked to that pace; the speed
//!   multiplier only exists in development builds (see [`TIME_CONTROLS`]).
//! * Saving and loading run on this thread too, because it owns the
//!   engine: the UI just sends `Command::Save` / `Command::Load`.

use std::path::PathBuf;
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
/// Whether this build lets anyone change how fast sim time runs.
///
/// A real game is time-locked: 90 real minutes are 24 sim hours, always,
/// so every play-through of a seed is comparable. Speed and the
/// closed-hours boost exist for development (soak tests, tuning) and are
/// compiled in only for debug builds or with `--features dev-time`:
///
/// ```text
/// cargo run --release                      # the game: time-locked
/// cargo run --release --features dev-time  # development: speed controls
/// ```
pub const TIME_CONTROLS: bool = cfg!(any(debug_assertions, feature = "dev-time"));

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

/// Slot written when the game shuts down with autosave on.
pub const AUTOSAVE_SLOT: &str = "autosave";

#[derive(Clone, Debug)]
pub struct RunnerOptions {
    /// Directory that holds one sub-directory per save slot.
    pub save_root: PathBuf,
    /// Write [`AUTOSAVE_SLOT`] when the engine is shut down.
    pub autosave_on_exit: bool,
    /// Load this slot instead of generating a new world, if it exists.
    pub load_slot: Option<String>,
}

impl Default for RunnerOptions {
    fn default() -> Self {
        Self { save_root: PathBuf::from("saves"), autosave_on_exit: false, load_slot: None }
    }
}

/// Start the engine thread. `wake` is called after each snapshot is sent so
/// the UI can request a repaint (e.g. `egui::Context::request_repaint`).
pub fn spawn(config: EngineConfig, options: RunnerOptions, wake: impl Fn() + Send + 'static) -> EngineHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
    let (snap_tx, snap_rx) = mpsc::sync_channel::<Box<Snapshot>>(2);
    let thread = std::thread::Builder::new()
        .name("sigma-engine".into())
        .spawn(move || run(config, options, cmd_rx, snap_tx, wake))
        .expect("failed to spawn engine thread");
    EngineHandle { commands: cmd_tx, snapshots: snap_rx, thread: Some(thread) }
}

fn save_game(engine: &mut Engine, options: &RunnerOptions, slot: &str) {
    let notice = match engine.save_to(&options.save_root, slot) {
        Ok(r) => {
            crate::console!(
                Level::Info,
                engine.now,
                "saved slot '{slot}': state {} KB, history {} KB, {} ms",
                r.state_bytes / 1024,
                r.history_bytes / 1024,
                r.millis
            );
            Notice::Info(format!("Saved to slot '{slot}' ({} ms)", r.millis))
        }
        Err(e) => {
            crate::console!(Level::Warn, engine.now, "save to '{slot}' failed: {e}");
            Notice::Error(format!("Save failed: {e}"))
        }
    };
    engine.notify(notice);
}

fn load_game(options: &RunnerOptions, slot: &str) -> Result<Engine, Notice> {
    match Engine::load_from(&options.save_root, slot) {
        Ok((mut engine, report)) => {
            let charts = match &report.history {
                Ok(n) => format!("{n} charts restored"),
                Err(e) => format!("charts start empty: {e}"),
            };
            crate::console!(Level::Info, engine.now, "loaded slot '{slot}' in {} ms ({charts})", report.millis);
            engine.notify(Notice::Info(format!("Loaded slot '{slot}' ({charts})")));
            Ok(engine)
        }
        Err(e) => {
            crate::console!(Level::Warn, 0.0, "load of '{slot}' failed: {e}");
            Err(Notice::Error(format!("Load failed: {e}")))
        }
    }
}

struct Pacing {
    speed: f64,
    paused: bool,
    closed_boost: bool,
}

fn run(
    config: EngineConfig,
    options: RunnerOptions,
    rx: Receiver<Command>,
    tx: SyncSender<Box<Snapshot>>,
    wake: impl Fn(),
) {
    let mut engine = match options.load_slot.as_deref().map(|slot| load_game(&options, slot)) {
        Some(Ok(engine)) => engine,
        Some(Err(notice)) => {
            let mut fresh = Engine::new(config.clone());
            fresh.notify(notice);
            fresh
        }
        None => Engine::new(config.clone()),
    };
    // Time-locked by default: 1x, no closed-hours boost.
    let mut pacing = Pacing { speed: 1.0, paused: false, closed_boost: false };
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
                    Command::Shutdown => {
                        if options.autosave_on_exit {
                            save_game(&mut engine, &options, AUTOSAVE_SLOT);
                        }
                        return;
                    }
                    // Time controls: ignored outright in a time-locked build,
                    // whatever the UI (or anything else) sends.
                    Command::SetSpeed(s) => {
                        if TIME_CONTROLS {
                            pacing.speed = s.clamp(1.0 / 16.0, MAX_SPEED);
                            engine.dev_time_used |= pacing.speed != 1.0;
                        }
                    }
                    Command::SetClosedBoost(b) => {
                        if TIME_CONTROLS {
                            pacing.closed_boost = b;
                            engine.dev_time_used |= b;
                        }
                    }
                    Command::SetPaused(p) => pacing.paused = p,
                    Command::Save { slot } => {
                        save_game(&mut engine, &options, &slot);
                        // Do not count the time spent writing as sim time.
                        last = Instant::now();
                        force_snapshot = true;
                    }
                    Command::Load { slot } => {
                        match load_game(&options, &slot) {
                            Ok(loaded) => {
                                engine = loaded;
                                wanted = engine.now;
                                engine.dev_time_used |= pacing.speed != 1.0 || pacing.closed_boost;
                            }
                            Err(notice) => engine.notify(notice),
                        }
                        last = Instant::now();
                        force_snapshot = true;
                    }
                    Command::SetLogLevel(l) => {
                        console::set_level(l);
                        crate::console!(Level::Warn, engine.now, "console level set to {}", l.label());
                    }
                    Command::Restart { seed } => {
                        // A new world is a new game: it gets a fresh game seed.
                        engine = Engine::new(EngineConfig { seed, game_seed: None, ..config.clone() });
                        wanted = engine.now;
                        last = Instant::now();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save;

    fn tiny() -> EngineConfig {
        EngineConfig { initial_securities: 300, brokers: 30, firms: 6, warmup: false, ..Default::default() }
    }

    fn wait_for(handle: &EngineHandle, mut done: impl FnMut(&Snapshot) -> bool) -> Box<Snapshot> {
        for _ in 0..600 {
            if let Some(s) = handle.latest()
                && done(&s)
            {
                return s;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("engine thread did not respond in time");
    }

    /// The whole loop through the real thread and channels: save on
    /// command, autosave on shutdown, then boot straight from the autosave.
    #[test]
    fn save_command_autosave_on_exit_and_boot_from_slot() {
        console::set_level(Level::Off);
        let root = std::env::temp_dir().join(format!("tixr-runner-test-{}", save::fresh_seed()));
        let options = RunnerOptions { save_root: root.clone(), autosave_on_exit: true, load_slot: None };
        let handle = spawn(tiny(), options.clone(), || {});
        handle.send(Command::Save { slot: "manual".into() });
        let snap = wait_for(&handle, |s| s.notices.iter().any(|n| matches!(n, Notice::Info(t) if t.contains("Saved"))));
        let seed = snap.seed;
        drop(handle); // Shutdown -> autosave, and the thread is joined.
        let slots: Vec<String> = save::list_slots(&root).into_iter().map(|s| s.name).collect();
        assert!(slots.contains(&"manual".to_string()) && slots.contains(&AUTOSAVE_SLOT.to_string()), "{slots:?}");

        let options = RunnerOptions { load_slot: Some(AUTOSAVE_SLOT.into()), autosave_on_exit: false, ..options };
        let handle = spawn(EngineConfig { seed: seed + 1, ..tiny() }, options, || {});
        let snap =
            wait_for(&handle, |s| s.notices.iter().any(|n| matches!(n, Notice::Info(t) if t.contains("Loaded"))));
        assert_eq!(snap.seed, seed, "the saved world was loaded, not a new one generated");
        drop(handle);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A time-locked build must ignore speed commands no matter who sends
    /// them; a development build must honour them and mark the game.
    #[test]
    fn speed_commands_follow_the_build_kind() {
        console::set_level(Level::Off);
        let handle = spawn(tiny(), RunnerOptions::default(), || {});
        handle.send(Command::SetSpeed(64.0));
        handle.send(Command::SetPaused(true));
        let snap = wait_for(&handle, |s| s.paused);
        assert_eq!(snap.time_controls, TIME_CONTROLS);
        if TIME_CONTROLS {
            assert_eq!(snap.speed, 64.0);
            assert!(snap.dev_time_used);
        } else {
            assert_eq!(snap.speed, 1.0);
            assert!(!snap.dev_time_used);
        }
    }
}
