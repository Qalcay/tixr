//! Simulated time.
//!
//! Two clocks run side by side:
//!
//! * **Calendar time** `t` in simulated seconds. Fundamentals (earnings,
//!   dividends, interest, inflation) live on this clock because companies
//!   earn money on weekends too.
//! * **Variance time** `v`, measured in "variance years". It advances at
//!   full speed while the market is open and at a trickle while it is
//!   closed. Prices diffuse on this clock, so overnight gaps exist but are
//!   smaller than a full session of movement, as in real markets (roughly a
//!   quarter of daily variance arrives overnight).
//!
//! Because every security stores the variance time of its last update, a
//! stale security can catch up in one exact step using `v_now - v_last`, no
//! matter how many sessions it slept through. That is what makes lazy,
//! randomly sampled updates possible.

pub const SECONDS_PER_DAY: f64 = 86_400.0;
pub const SECONDS_PER_YEAR: f64 = 365.25 * SECONDS_PER_DAY;

/// Real-time pacing at speed 1x: 90 real minutes per simulated day.
pub const REAL_SECONDS_PER_SIM_DAY: f64 = 90.0 * 60.0;
/// Simulated seconds per real second at 1x (= 16).
pub const BASE_SIM_RATE: f64 = SECONDS_PER_DAY / REAL_SECONDS_PER_SIM_DAY;

const OPEN_SEC: f64 = 9.5 * 3600.0; // 09:30
const CLOSE_SEC: f64 = 16.0 * 3600.0; // 16:00

/// When the market trades. Selected in the Settings window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionMode {
    /// 09:30 to 16:00, Monday to Friday. Realistic, but nights and weekends
    /// are quiet: at 1x a weekend lasts three real hours.
    Regular,
    /// 04:00 to 20:00, Monday to Friday, like pre and post market sessions.
    Extended,
    /// Trades around the clock, every day. Best for watching the engine.
    AlwaysOpen,
}

impl SessionMode {
    pub const ALL: [SessionMode; 3] = [Self::Regular, Self::Extended, Self::AlwaysOpen];

    pub fn label(self) -> &'static str {
        match self {
            Self::Regular => "Regular (09:30-16:00, Mon-Fri)",
            Self::Extended => "Extended (04:00-20:00, Mon-Fri)",
            Self::AlwaysOpen => "Always open",
        }
    }

    fn hours(self) -> (f64, f64) {
        match self {
            Self::Regular => (OPEN_SEC, CLOSE_SEC),
            Self::Extended => (4.0 * 3600.0, 20.0 * 3600.0),
            Self::AlwaysOpen => (0.0, SECONDS_PER_DAY),
        }
    }

    /// Seconds-of-day at which the daily close is recorded.
    pub fn close_second(self) -> f64 {
        match self {
            Self::AlwaysOpen => SECONDS_PER_DAY, // midnight
            _ => CLOSE_SEC,                      // always 16:00 for history
        }
    }

    pub fn is_open(self, t: f64) -> bool {
        let day = (t / SECONDS_PER_DAY).floor();
        let weekday = (day as i64).rem_euclid(7); // day 0 is a Monday
        if self != Self::AlwaysOpen && weekday >= 5 {
            return false;
        }
        let sod = t - day * SECONDS_PER_DAY;
        let (open, close) = self.hours();
        sod >= open && sod < close
    }

    /// The next daily close strictly after `t`. Closes happen on trading
    /// days only (every day when always open).
    pub fn next_close(self, t: f64) -> f64 {
        let mut day = (t / SECONDS_PER_DAY).floor();
        loop {
            let weekday = (day as i64).rem_euclid(7);
            let c = day * SECONDS_PER_DAY + self.close_second();
            if c > t + 1e-9 && (self == Self::AlwaysOpen || weekday < 5) {
                return c;
            }
            day += 1.0;
        }
    }

    /// Next time strictly after `t` at which `is_open` may change. The
    /// engine never lets a sub-step cross one of these, so the variance
    /// rate is constant within each sub-step.
    pub fn next_boundary(self, t: f64) -> f64 {
        let day = (t / SECONDS_PER_DAY).floor();
        let base = day * SECONDS_PER_DAY;
        let (open, close) = self.hours();
        let mut candidates = [base + open, base + close, base + SECONDS_PER_DAY];
        candidates.sort_by(f64::total_cmp);
        candidates
            .into_iter()
            .find(|&c| c > t + 1e-9)
            .unwrap_or(base + SECONDS_PER_DAY)
    }
}

/// Fraction of open-market variance that arrives per second while closed.
/// Chosen so about a quarter of a weekday's variance lands overnight.
const OVERNIGHT_RATE: f64 = 0.124;
const WEEKEND_RATE: f64 = 0.04;

/// Relative variance rate (1.0 while open).
pub fn variance_rate(mode: SessionMode, t: f64) -> f64 {
    if mode.is_open(t) {
        return 1.0;
    }
    let weekday = ((t / SECONDS_PER_DAY).floor() as i64).rem_euclid(7);
    if weekday >= 5 { WEEKEND_RATE } else { OVERNIGHT_RATE }
}

/// How many "rate-weighted seconds" make up one variance year for a mode.
/// Normalising this way means an annual volatility parameter of 30% gives
/// 30% per calendar year in every session mode.
pub fn variance_seconds_per_year(mode: SessionMode) -> f64 {
    // Integrate the rate over one week at one-minute resolution, then scale.
    let mut acc = 0.0;
    let step = 60.0;
    let mut t = 0.0;
    while t < 7.0 * SECONDS_PER_DAY {
        acc += variance_rate(mode, t + step * 0.5) * step;
        t += step;
    }
    acc * (SECONDS_PER_YEAR / (7.0 * SECONDS_PER_DAY))
}

/// Human-readable clock parts for display and logs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClockParts {
    pub day: u64,
    pub weekday: u8,
    pub hour: u8,
    pub minute: u8,
}

impl ClockParts {
    pub fn from_t(t: f64) -> Self {
        let day = (t / SECONDS_PER_DAY).floor().max(0.0);
        let sod = t - day * SECONDS_PER_DAY;
        Self {
            day: day as u64,
            weekday: ((day as u64) % 7) as u8,
            hour: (sod / 3600.0) as u8,
            minute: ((sod % 3600.0) / 60.0) as u8,
        }
    }

    pub fn weekday_name(self) -> &'static str {
        ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][self.weekday as usize % 7]
    }
}

/// The simulation starts at the opening bell of day 0 (a Monday).
pub const START_T: f64 = OPEN_SEC;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regular_hours() {
        let m = SessionMode::Regular;
        assert!(m.is_open(10.0 * 3600.0)); // Monday 10:00
        assert!(!m.is_open(8.0 * 3600.0)); // Monday 08:00
        assert!(!m.is_open(5.0 * SECONDS_PER_DAY + 12.0 * 3600.0)); // Saturday noon
        assert!(SessionMode::AlwaysOpen.is_open(5.0 * SECONDS_PER_DAY + 3.0 * 3600.0));
    }

    #[test]
    fn boundaries_are_ordered() {
        let m = SessionMode::Regular;
        let t = 9.0 * 3600.0;
        assert_eq!(m.next_boundary(t), OPEN_SEC);
        assert_eq!(m.next_boundary(OPEN_SEC), CLOSE_SEC);
        assert_eq!(m.next_boundary(CLOSE_SEC), SECONDS_PER_DAY);
    }

    #[test]
    fn overnight_share_is_about_a_quarter() {
        let m = SessionMode::Regular;
        let open = 6.5 * 3600.0;
        let closed = 17.5 * 3600.0 * OVERNIGHT_RATE;
        let share = closed / (open + closed);
        assert!((0.2..0.3).contains(&share), "{share}");
        assert!(variance_seconds_per_year(m) > 0.0);
    }

    #[test]
    fn pacing_is_ninety_minutes_per_day() {
        assert_eq!(BASE_SIM_RATE, 16.0);
    }
}
