//! Number formatting with *bounded width*.
//!
//! tixr alpha printed raw prices with `{:.2}`, so a stock worth 1.2e9 made
//! a 13-character label that pushed the side panel wider. Every formatter
//! here has a maximum width, switching to K/M/B/T suffixes before a number
//! can grow past it. Combined with fixed-size panels, that keeps the
//! layout stable whatever the market does.

pub const CURRENCY: &str = "§";

/// Compact number: at most ~7 characters (e.g. "999.99", "12.3K", "4.56B").
pub fn compact(v: f64) -> String {
    if !v.is_finite() {
        return "-".into();
    }
    let a = v.abs();
    // Thresholds sit just below each power of 1,000 so 999,960 prints as
    // "1.0M", never "1000K".
    let (n, s) = if a >= 999.95e12 {
        return format!("{:.1e}", v);
    } else if a >= 999.5e9 {
        (v / 1e12, "T")
    } else if a >= 999.5e6 {
        (v / 1e9, "B")
    } else if a >= 999.5e3 {
        (v / 1e6, "M")
    } else if a >= 1e4 {
        (v / 1e3, "K")
    } else {
        return format!("{v:.2}");
    };
    if n.abs() >= 100.0 { format!("{n:.0}{s}") } else { format!("{n:.1}{s}") }
}

pub fn money(v: f64) -> String {
    format!("{CURRENCY}{}", compact(v))
}

/// Price: two decimals below 10,000, compact above. Max 7 chars.
pub fn price(p: f64) -> String {
    if p.is_finite() && p.abs() < 1e4 { format!("{p:.2}") } else { compact(p) }
}

/// Signed percent, clamped for display so it never widens a column.
pub fn pct(v: f64) -> String {
    if !v.is_finite() {
        return "-".into();
    }
    let v = v.clamp(-999.9, 9999.0);
    format!("{v:+.2}%")
}

/// A rate shown in percent (0.0425 -> "4.25%").
pub fn rate(v: f64) -> String {
    if v.is_finite() { format!("{:.2}%", v * 100.0) } else { "-".into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_are_bounded() {
        for v in [0.0, 1.5, 999.99, 12_345.0, 9.99e8, 1.2e9, 3.4e12, 7.7e14, -2.5e10] {
            assert!(compact(v).chars().count() <= 7, "{v} -> {}", compact(v));
            assert!(price(v).chars().count() <= 9, "{v} -> {}", price(v));
        }
        assert!(pct(123456.0).chars().count() <= 9);
        assert_eq!(compact(999_960.0), "1.0M");
        assert_eq!(compact(12_345.0), "12.3K");
    }
}
