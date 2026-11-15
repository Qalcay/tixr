//! Real-time debug output to the terminal (stderr), alongside the egui window.
//!
//! The alpha embedded a log panel inside egui, which cost frame time and
//! scrolled away. Here the engine thread writes straight to the console the
//! app was started from, tagged with the sim clock, and the level can be
//! changed at runtime from the Settings window. A global atomic is the
//! standard cheap way to do this without a logging crate: checking the level
//! is one relaxed load, so disabled log lines cost almost nothing.

use std::io::Write;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::time::{SimTime, clock_string, day_index, weekday_name};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Level {
    Off = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
    Trace = 4,
}

pub const ALL_LEVELS: [Level; 5] = [Level::Off, Level::Warn, Level::Info, Level::Debug, Level::Trace];

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::Off => "off",
            Level::Warn => "warn",
            Level::Info => "info",
            Level::Debug => "debug",
            Level::Trace => "trace",
        }
    }
    fn from_u8(v: u8) -> Level {
        match v {
            0 => Level::Off,
            1 => Level::Warn,
            2 => Level::Info,
            3 => Level::Debug,
            _ => Level::Trace,
        }
    }
}

static LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);

pub fn set_level(level: Level) {
    LEVEL.store(level as u8, Ordering::Relaxed);
}

pub fn level() -> Level {
    Level::from_u8(LEVEL.load(Ordering::Relaxed))
}

#[inline]
pub fn enabled(level: Level) -> bool {
    level != Level::Off && level as u8 <= LEVEL.load(Ordering::Relaxed)
}

/// Write one line. Prefer the [`console!`](crate::console!) macro, which skips
/// formatting entirely when the level is disabled.
pub fn emit(level: Level, t: SimTime, args: std::fmt::Arguments<'_>) {
    let mut err = std::io::stderr().lock();
    let _ =
        writeln!(err, "[d{:04} {} {}] {:<5} {}", day_index(t), weekday_name(t), clock_string(t), level.label(), args);
}

/// `console!(Level::Info, sim_time, "live={}", n);`
#[macro_export]
macro_rules! console {
    ($lvl:expr, $t:expr, $($arg:tt)*) => {
        if $crate::console::enabled($lvl) {
            $crate::console::emit($lvl, $t, format_args!($($arg)*));
        }
    };
}
