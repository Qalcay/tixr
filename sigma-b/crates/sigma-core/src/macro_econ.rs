//! The macro economy, the yield curve and the market-wide drivers.
//!
//! ## Why the original blew up
//! In tixr alpha several state variables were integrated with drift and no
//! restoring force (fair value drifted by `mu * DT` forever, regime drift
//! fed straight into log price, variance was clamped to 8.8 so sqrt(var)
//! could hit ~3 per tick, and order impact divided by a price that could
//! approach zero). Any one of those can walk to `inf` and then `NaN` once
//! enough simulated days pass.
//!
//! ## What sigma does instead
//! Every macro variable here is a **mean-reverting Ornstein-Uhlenbeck (OU)
//! process** stepped with its *exact* transition:
//!
//! ```text
//! x(t+dt) = mu + (x - mu) * e^(-k dt) + sigma * sqrt((1 - e^(-2k dt)) / (2k)) * Z
//! ```
//!
//! An OU process has a stationary distribution, so it cannot wander off to
//! infinity no matter how long the simulation runs, and the exact step is
//! correct for any `dt` (no Euler error that grows with step size). Values
//! that must respect hard bounds (rates cannot go below zero) are clamped
//! after the step as a last line of defence, not as the main mechanism.
//!
//! The yield curve uses the Nelson-Siegel model, the same three-factor form
//! (level, slope, curvature) many central banks publish curves with.

use crate::clock::SECONDS_PER_YEAR;
use crate::rng::Rng;

/// Exact OU transition. `dt` in years. Safe for `kappa` near zero.
pub fn ou_step(x: f64, mean: f64, kappa: f64, sigma: f64, dt: f64, z: f64) -> f64 {
    if dt <= 0.0 {
        return x;
    }
    let decay = (-kappa * dt).exp();
    let var = if kappa * dt < 1e-8 {
        sigma * sigma * dt // limit as kappa -> 0: Brownian motion
    } else {
        sigma * sigma * (1.0 - decay * decay) / (2.0 * kappa)
    };
    mean + (x - mean) * decay + var.sqrt() * z
}

/// Standard normal CDF (Abramowitz-Stegun 7.1.26 via erf, error < 1.5e-7).
pub fn norm_cdf(x: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.327_591_1 * x.abs() / std::f64::consts::SQRT_2);
    let poly = t
        * (0.254_829_592
            + t * (-0.284_496_736 + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    let erf = 1.0 - poly * (-(x * x) / 2.0).exp();
    if x >= 0.0 { 0.5 * (1.0 + erf) } else { 0.5 * (1.0 - erf) }
}

/// Tenors shown on the curve, in years.
pub const TENORS: [f64; 8] = [1.0 / 12.0, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0];
pub const TENOR_LABELS: [&str; 8] = ["1M", "3M", "6M", "1Y", "2Y", "5Y", "10Y", "30Y"];

/// Nelson-Siegel decay time (years). Curvature peaks near 2-3 years.
const NS_TAU: f64 = 2.0;

/// Nelson-Siegel curve: `y(T) = level + slope * f1(T) + curvature * f2(T)`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct YieldCurve {
    pub level: f64,
    pub slope: f64,
    pub curvature: f64,
}

impl YieldCurve {
    pub fn yield_at(&self, tenor_years: f64) -> f64 {
        let x = (tenor_years / NS_TAU).max(1e-9);
        let f1 = (1.0 - (-x).exp()) / x;
        let f2 = f1 - (-x).exp();
        self.level + self.slope * f1 + self.curvature * f2
    }

    pub fn points(&self) -> [f64; 8] {
        TENORS.map(|t| self.yield_at(t))
    }

    /// Instantaneous forward rate at `T`, which the market "expects" the
    /// short rate to be then. Used for the rate forecast.
    pub fn forward_at(&self, tenor_years: f64) -> f64 {
        let x = tenor_years / NS_TAU;
        self.level + self.slope * (-x).exp() + self.curvature * x * (-x).exp()
    }
}

/// Long-run anchors. Real-world-ish values for a developed economy.
#[derive(Clone, Copy, Debug)]
pub struct MacroParams {
    pub inflation_target: f64,
    pub neutral_real_rate: f64,
    pub potential_growth: f64,
}

impl Default for MacroParams {
    fn default() -> Self {
        Self { inflation_target: 0.02, neutral_real_rate: 0.01, potential_growth: 0.02 }
    }
}

/// Everything the economy knows about itself at one instant.
#[derive(Clone, Debug)]
pub struct Macro {
    pub params: MacroParams,
    /// Output gap (fraction of potential GDP). Positive = running hot.
    pub output_gap: f64,
    /// Annual CPI inflation.
    pub inflation: f64,
    /// Central bank policy rate.
    pub policy_rate: f64,
    /// Term premium demanded for lending long.
    pub term_premium: f64,
    /// Nelson-Siegel curvature factor.
    pub curvature: f64,
    /// Risk appetite, roughly N(0,1). Negative = fear.
    pub risk_appetite: f64,
    /// High-yield credit spread over treasuries.
    pub credit_spread: f64,
    /// Log of the CPI price level (starts at 0).
    pub log_cpi: f64,
    /// Calendar years elapsed (for potential GDP).
    pub years: f64,
    pub curve: YieldCurve,
}

impl Macro {
    pub fn new(params: MacroParams) -> Self {
        let mut m = Self {
            params,
            output_gap: 0.0,
            inflation: params.inflation_target,
            policy_rate: params.neutral_real_rate + params.inflation_target,
            term_premium: 0.008,
            curvature: 0.0,
            risk_appetite: 0.3,
            credit_spread: 0.038,
            log_cpi: 0.0,
            years: 0.0,
            curve: YieldCurve::default(),
        };
        m.rebuild_curve();
        m
    }

    /// Anchored inflation expectations: mostly the target, a bit of today.
    pub fn expected_inflation(&self) -> f64 {
        0.75 * self.params.inflation_target + 0.25 * self.inflation
    }

    /// Taylor (1993) rule target for the policy rate.
    pub fn taylor_target(&self) -> f64 {
        let p = self.params;
        p.neutral_real_rate
            + self.inflation
            + 0.5 * (self.inflation - p.inflation_target)
            + 0.5 * self.output_gap
    }

    /// Equity risk premium: higher when investors are scared.
    pub fn equity_risk_premium(&self) -> f64 {
        (0.045 - 0.012 * self.risk_appetite.tanh() * 1.5 + 0.25 * (self.credit_spread - 0.04))
            .clamp(0.02, 0.10)
    }

    /// Market-wide volatility multiplier (a VIX-like fear gauge).
    pub fn vol_multiplier(&self) -> f64 {
        (1.0 - 0.30 * self.risk_appetite.tanh()).clamp(0.6, 1.8)
    }

    /// Log of real GDP relative to day 0: potential growth plus the gap.
    pub fn log_real_gdp(&self) -> f64 {
        self.params.potential_growth * self.years + self.output_gap
    }

    pub fn y10(&self) -> f64 {
        self.curve.yield_at(10.0)
    }

    pub fn y3m(&self) -> f64 {
        self.curve.yield_at(0.25)
    }

    /// 10y minus 3m, the spread the New York Fed uses.
    pub fn term_spread(&self) -> f64 {
        self.y10() - self.y3m()
    }

    /// Probability of recession within 12 months from the term spread:
    /// `P = Phi(-0.5333 - 0.6330 * spread_pct)` (Estrella and Mishkin,
    /// as published by the New York Fed).
    pub fn recession_probability(&self) -> f64 {
        norm_cdf(-0.5333 - 0.6330 * self.term_spread() * 100.0)
    }

    fn rebuild_curve(&mut self) {
        // Level = where long rates settle: neutral real rate + expected
        // inflation + term premium. Slope ties the short end to policy.
        let level = self.params.neutral_real_rate + self.expected_inflation() + self.term_premium;
        self.curve = YieldCurve {
            level,
            slope: self.policy_rate - level,
            curvature: self.curvature,
        };
    }

    /// Advance by `dt_sec` simulated seconds.
    pub fn step(&mut self, dt_sec: f64, rng: &mut Rng) {
        if dt_sec <= 0.0 {
            return;
        }
        let dt = dt_sec / SECONDS_PER_YEAR;
        let p = self.params;
        self.years += dt;

        // Risk appetite: OU plus rare panic jumps (a few per decade).
        self.risk_appetite = ou_step(self.risk_appetite, 0.3, 1.5, 0.9, dt, rng.normal());
        if rng.chance(0.25 * dt) {
            self.risk_appetite -= rng.range(1.0, 2.5);
        }
        self.risk_appetite = self.risk_appetite.clamp(-4.0, 4.0);

        // Output gap: pulled toward zero, pushed down by tight money (real
        // rate above neutral) and by fear.
        let real_gap = self.policy_rate - self.inflation - p.neutral_real_rate;
        let gap_target = (-0.6 * real_gap + 0.004 * self.risk_appetite).clamp(-0.05, 0.05);
        self.output_gap = ou_step(self.output_gap, gap_target, 0.8, 0.012, dt, rng.normal())
            .clamp(-0.10, 0.10);

        // Inflation: Phillips curve around target.
        let infl_target = p.inflation_target + 0.35 * self.output_gap;
        self.inflation =
            ou_step(self.inflation, infl_target, 0.9, 0.006, dt, rng.normal()).clamp(-0.03, 0.15);
        self.log_cpi += self.inflation * dt;

        // Policy rate: inertial pursuit of the Taylor rule, floored at zero.
        let target = self.taylor_target().clamp(0.0, 0.15);
        self.policy_rate =
            ou_step(self.policy_rate, target, 1.6, 0.002, dt, rng.normal()).clamp(0.0, 0.15);

        self.term_premium =
            ou_step(self.term_premium, 0.008, 0.6, 0.004, dt, rng.normal()).clamp(-0.01, 0.04);
        self.curvature =
            ou_step(self.curvature, 0.0, 1.0, 0.008, dt, rng.normal()).clamp(-0.03, 0.03);

        // Credit spread: widens when the economy is weak and investors fear.
        let spread_target = (0.038
            + 0.6 * (-self.output_gap).max(0.0)
            + 0.012 * (-self.risk_appetite).max(0.0))
        .clamp(0.02, 0.15);
        self.credit_spread =
            ou_step(self.credit_spread, spread_target, 2.0, 0.01, dt, rng.normal()).clamp(0.015, 0.20);

        self.rebuild_curve();
    }

    /// Sanity check used by tests and the console watchdog.
    pub fn is_finite(&self) -> bool {
        [
            self.output_gap,
            self.inflation,
            self.policy_rate,
            self.term_premium,
            self.curvature,
            self.risk_appetite,
            self.credit_spread,
            self.log_cpi,
        ]
        .iter()
        .all(|v| v.is_finite())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ou_exact_step_has_right_stationary_variance() {
        // Simulate many independent paths with one big step each and with
        // many small steps: both must land on the same distribution.
        let (kappa, sigma, mean) = (2.0, 0.5, 1.0);
        let mut rng = Rng::seeded(17);
        let n = 40_000;
        let horizon = 3.0;
        let mut one = Vec::with_capacity(n);
        let mut many = Vec::with_capacity(n);
        for _ in 0..n {
            one.push(ou_step(0.0, mean, kappa, sigma, horizon, rng.normal()));
            let mut x = 0.0;
            for _ in 0..30 {
                x = ou_step(x, mean, kappa, sigma, horizon / 30.0, rng.normal());
            }
            many.push(x);
        }
        let stats = |v: &[f64]| {
            let m = v.iter().sum::<f64>() / v.len() as f64;
            let var = v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / v.len() as f64;
            (m, var)
        };
        let (m1, v1) = stats(&one);
        let (m2, v2) = stats(&many);
        let expected_var = sigma * sigma * (1.0 - (-2.0 * kappa * horizon).exp()) / (2.0 * kappa);
        assert!((m1 - m2).abs() < 0.02, "{m1} vs {m2}");
        assert!((v1 - expected_var).abs() < 0.05 * expected_var, "{v1}");
        assert!((v2 - expected_var).abs() < 0.05 * expected_var, "{v2}");
    }

    #[test]
    fn curve_short_end_is_policy_rate() {
        let m = Macro::new(MacroParams::default());
        assert!((m.curve.yield_at(1e-6) - m.policy_rate).abs() < 1e-6);
    }

    #[test]
    fn recession_probability_rises_when_curve_inverts() {
        let mut m = Macro::new(MacroParams::default());
        let normal = m.recession_probability();
        m.policy_rate = 0.07;
        m.rebuild_curve();
        assert!(m.term_spread() < 0.0);
        assert!(m.recession_probability() > normal);
    }

    #[test]
    fn macro_stays_finite_for_a_century() {
        let mut m = Macro::new(MacroParams::default());
        let mut rng = Rng::seeded(99);
        let day = 86_400.0;
        for _ in 0..(365 * 100) {
            m.step(day, &mut rng);
            assert!(m.is_finite());
        }
        assert!(m.policy_rate >= 0.0 && m.policy_rate <= 0.15);
        assert!(m.inflation.abs() < 0.2);
    }

    #[test]
    fn norm_cdf_known_values() {
        assert!((norm_cdf(0.0) - 0.5).abs() < 1e-7);
        assert!((norm_cdf(1.96) - 0.975).abs() < 1e-3);
        assert!((norm_cdf(-1.0) - 0.1587).abs() < 1e-3);
    }
}
