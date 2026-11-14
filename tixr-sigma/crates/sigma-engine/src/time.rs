//! Simulation clock and calendar.
//!
//! Sim time is an `f64` count of **simulated seconds** since the start of the
//! run. An `f64` holds whole seconds exactly up to 2^53, i.e. for ~285 million
//! simulated years, so there is no precision drift (the alpha kept prices and
//! time in `f32`, which runs out of digits within months of sim time).
//!
//! Pacing: 90 real minutes = 24 simulated hours, so the base speed is
//! 86_400 / 5_400 = 16 simulated seconds per real second.

pub type SimTime = f64;

pub const SECS_PER_MIN: f64 = 60.0;
pub const SECS_PER_HOUR: f64 = 3_600.0;
pub const SECS_PER_DAY: f64 = 86_400.0;
/// Calendar year used to convert annualised finance parameters.
pub const SECS_PER_YEAR: f64 = 365.0 * SECS_PER_DAY;

/// Real seconds that make up one simulated day at 1x speed (90 minutes).
pub const REAL_SECS_PER_SIM_DAY: f64 = 90.0 * 60.0;
/// Simulated seconds per real second at 1x speed (= 16).
pub const BASE_TIME_SCALE: f64 = SECS_PER_DAY / REAL_SECS_PER_SIM_DAY;

pub const DAYS_PER_WEEK: u64 = 7;
pub const TRADING_DAYS_PER_WEEK: u64 = 5;

#[inline]
pub fn years(secs: f64) -> f64 {
    secs / SECS_PER_YEAR
}

/// Whole simulated days elapsed (day 0 is a Monday).
#[inline]
pub fn day_index(t: SimTime) -> u64 {
    (t / SECS_PER_DAY).floor().max(0.0) as u64
}

/// 0 = Monday ... 6 = Sunday.
#[inline]
pub fn weekday(t: SimTime) -> u64 {
    day_index(t) % DAYS_PER_WEEK
}

#[inline]
pub fn is_weekend(t: SimTime) -> bool {
    weekday(t) >= TRADING_DAYS_PER_WEEK
}

/// Seconds since midnight of the current sim day.
#[inline]
pub fn time_of_day(t: SimTime) -> f64 {
    t - day_index(t) as f64 * SECS_PER_DAY
}

pub fn weekday_name(t: SimTime) -> &'static str {
    ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][weekday(t) as usize]
}

/// `HH:MM` of the sim clock.
pub fn clock_string(t: SimTime) -> String {
    let tod = time_of_day(t);
    let h = (tod / SECS_PER_HOUR) as u32;
    let m = ((tod % SECS_PER_HOUR) / SECS_PER_MIN) as u32;
    format!("{h:02}:{m:02}")
}

/// A daily trading window in sim seconds after midnight, Monday to Friday.
/// Sessions never wrap past midnight, which keeps open/close arithmetic trivial.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Session {
    pub open: f64,
    pub close: f64,
}

impl Session {
    pub const fn hours(open_h: f64, close_h: f64) -> Self {
        Self { open: open_h * SECS_PER_HOUR, close: close_h * SECS_PER_HOUR }
    }

    pub fn is_open(&self, t: SimTime) -> bool {
        if is_weekend(t) {
            return false;
        }
        let tod = time_of_day(t);
        tod >= self.open && tod < self.close
    }

    /// The next instant at or after `t` when this session is open.
    pub fn next_open_at_or_after(&self, t: SimTime) -> SimTime {
        if self.is_open(t) {
            return t;
        }
        let mut day = day_index(t);
        // Today's session still ahead of us?
        if time_of_day(t) < self.open && weekday(t) < TRADING_DAYS_PER_WEEK {
            return day as f64 * SECS_PER_DAY + self.open;
        }
        loop {
            day += 1;
            if day % DAYS_PER_WEEK < TRADING_DAYS_PER_WEEK {
                return day as f64 * SECS_PER_DAY + self.open;
            }
        }
    }

    /// The next close strictly after `t` (on a trading day).
    pub fn next_close_after(&self, t: SimTime) -> SimTime {
        let mut day = day_index(t);
        if time_of_day(t) < self.close && weekday(t) < TRADING_DAYS_PER_WEEK {
            return day as f64 * SECS_PER_DAY + self.close;
        }
        loop {
            day += 1;
            if day % DAYS_PER_WEEK < TRADING_DAYS_PER_WEEK {
                return day as f64 * SECS_PER_DAY + self.close;
            }
        }
    }

    pub fn length_secs(&self) -> f64 {
        self.close - self.open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_scale_is_sixteen() {
        assert_eq!(BASE_TIME_SCALE, 16.0);
    }

    #[test]
    fn weekend_detection() {
        assert!(!is_weekend(0.0)); // Monday
        assert!(!is_weekend(4.5 * SECS_PER_DAY)); // Friday noon
        assert!(is_weekend(5.1 * SECS_PER_DAY)); // Saturday
        assert!(is_weekend(6.9 * SECS_PER_DAY)); // Sunday
        assert!(!is_weekend(7.0 * SECS_PER_DAY)); // Monday again
    }

    #[test]
    fn session_open_and_next_open() {
        let s = Session::hours(9.5, 16.0);
        assert!(!s.is_open(8.0 * SECS_PER_HOUR));
        assert!(s.is_open(10.0 * SECS_PER_HOUR));
        assert!(!s.is_open(16.0 * SECS_PER_HOUR));
        // Friday evening -> next open is Monday 09:30.
        let fri_evening = 4.0 * SECS_PER_DAY + 18.0 * SECS_PER_HOUR;
        let next = s.next_open_at_or_after(fri_evening);
        assert_eq!(next, 7.0 * SECS_PER_DAY + 9.5 * SECS_PER_HOUR);
        assert_eq!(s.next_close_after(fri_evening), 7.0 * SECS_PER_DAY + 16.0 * SECS_PER_HOUR);
    }

    #[test]
    fn clock_formatting() {
        assert_eq!(clock_string(13.0 * SECS_PER_HOUR + 5.0 * SECS_PER_MIN), "13:05");
        assert_eq!(weekday_name(2.0 * SECS_PER_DAY), "Wed");
    }
}
