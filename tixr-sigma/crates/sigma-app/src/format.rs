//! Number formatting with **bounded width**.
//!
//! The alpha printed raw `{:.2}` prices, so a stock worth $1,234,567.89 was
//! wider than one worth $12.34 and pushed the side panel out. Every formatter
//! here has a maximum output width, switching to k/M/B/T suffixes as values
//! grow, so layouts can rely on fixed column widths.

/// Price with at most 8 characters, e.g. `12.34`, `1,234.5`, `12.35k`, `1.23M`.
pub fn price(v: f64) -> String {
    if !v.is_finite() {
        return "--".into();
    }
    let a = v.abs();
    if a < 10.0 {
        format!("{v:.3}")
    } else if a < 1_000.0 {
        format!("{v:.2}")
    } else if a < 10_000.0 {
        let whole = v.trunc() as i64;
        format!("{},{:03}.{}", whole / 1_000, (whole % 1_000).abs(), ((a.fract() * 10.0) as i64).min(9))
    } else {
        compact(v, 2)
    }
}

/// Money: `$12.3k`, `$4.56M`, `-$1.20B` (max ~9 characters).
pub fn money(v: f64) -> String {
    if !v.is_finite() {
        return "--".into();
    }
    let sign = if v < 0.0 { "-" } else { "" };
    let a = v.abs();
    if a < 10_000.0 { format!("{sign}${a:.2}") } else { format!("{sign}${}", compact(a, 2)) }
}

/// `1.23k`, `45.6M`, `7.89B`, `1.00T`, `2.0Q`.
pub fn compact(v: f64, decimals: usize) -> String {
    let a = v.abs();
    let (n, s) = if a >= 1e15 {
        (v / 1e15, "Q")
    } else if a >= 1e12 {
        (v / 1e12, "T")
    } else if a >= 1e9 {
        (v / 1e9, "B")
    } else if a >= 1e6 {
        (v / 1e6, "M")
    } else if a >= 1e3 {
        (v / 1e3, "k")
    } else {
        (v, "")
    };
    format!("{n:.decimals$}{s}")
}

/// Signed percentage, clamped to a sane display range: `+1.23%`.
pub fn pct(v: f64) -> String {
    if !v.is_finite() {
        return "--".into();
    }
    if v.abs() >= 1_000.0 {
        return if v > 0.0 { ">+999%".into() } else { "<-999%".into() };
    }
    format!("{v:+.2}%")
}

/// A rate given as a fraction: 0.0412 -> `4.12%`.
pub fn rate(v: f64) -> String {
    format!("{:.2}%", v * 100.0)
}

pub fn shares(v: f64) -> String {
    if v.abs() < 10_000.0 { format!("{v:.1}") } else { compact(v, 2) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_are_bounded() {
        for v in [0.001, 1.5, 99.99, 1234.5, 98_765.4, 1.2e7, 3.3e10, 9.9e13, 5e16] {
            assert!(price(v).len() <= 8, "price({v}) = {}", price(v));
            assert!(money(v).len() <= 9, "money({v}) = {}", money(v));
        }
        assert_eq!(price(1234.56), "1,234.5");
        assert_eq!(money(2_500_000.0), "$2.50M");
        assert_eq!(pct(5000.0), ">+999%");
    }
}
