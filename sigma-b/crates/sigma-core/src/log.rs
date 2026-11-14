//! Console logging for real-time debugging, with no dependencies.
//!
//! The spec asks for debugging output on the console *next to* the egui
//! window, not an embedded log panel. The engine thread writes one status
//! line every few seconds (and events at `debug`), straight to stderr, so
//! the UI never spends a frame rendering log text.
//!
//! Level comes from the `SIGMA_LOG` environment variable (`off`, `warn`,
//! `info`, `debug`, `trace`; default `info`) and can be changed live from
//! the Settings window.
//!
//! A real team would likely reach for the `log` + `env_logger` or `tracing`
//! crates. Here we hand-roll ~40 lines to keep the dependency tree lean.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Level {
    Off = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
    Trace = 4,
}

impl Level {
    pub const ALL: [Level; 5] = [Level::Off, Level::Warn, Level::Info, Level::Debug, Level::Trace];

    pub fn name(self) -> &'static str {
        match self {
            Level::Off => "off",
            Level::Warn => "warn",
            Level::Info => "info",
            Level::Debug => "debug",
            Level::Trace => "trace",
        }
    }

    fn parse(s: &str) -> Option<Level> {
        Level::ALL.into_iter().find(|l| l.name().eq_ignore_ascii_case(s.trim()))
    }
}

static LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);
static START: OnceLock<Instant> = OnceLock::new();

/// Read `SIGMA_LOG` once at startup.
pub fn init_from_env() {
    START.get_or_init(Instant::now);
    if let Some(l) = std::env::var("SIGMA_LOG").ok().as_deref().and_then(Level::parse) {
        set_level(l);
    }
}

pub fn set_level(l: Level) {
    LEVEL.store(l as u8, Ordering::Relaxed);
}

pub fn level() -> Level {
    Level::ALL[LEVEL.load(Ordering::Relaxed) as usize]
}

pub fn enabled(l: Level) -> bool {
    l != Level::Off && l as u8 <= LEVEL.load(Ordering::Relaxed)
}

#[doc(hidden)]
pub fn write(l: Level, target: &str, msg: std::fmt::Arguments<'_>) {
    let secs = START.get_or_init(Instant::now).elapsed().as_secs_f64();
    let tag = match l {
        Level::Warn => "WARN ",
        Level::Info => "INFO ",
        Level::Debug => "DEBUG",
        Level::Trace => "TRACE",
        Level::Off => return,
    };
    eprintln!("[sigma {:>8.1}s {tag} {target}] {msg}", secs);
}

/// `slog!(Level::Info, "engine", "live {}", n);`
#[macro_export]
macro_rules! slog {
    ($lvl:expr, $target:expr, $($arg:tt)*) => {
        if $crate::log::enabled($lvl) {
            $crate::log::write($lvl, $target, format_args!($($arg)*));
        }
    };
}
