//! Macro-economy, yield curve and market regime: the "financial market
//! drivers" every price in the simulation ultimately answers to.
//!
//! The model is a small New-Keynesian loop, the same shape central banks use
//! for teaching:
//!
//! * **Output gap** `x` - how far the economy runs above/below potential.
//!   Pulled toward 0, pushed down by high real rates, shocked by regime.
//! * **Inflation** `pi` - pulled toward anchored expectations, pushed up when
//!   the economy runs hot (Phillips curve).
//! * **Policy rate** `i` - follows a Taylor rule with smoothing, floored at 0.
//! * **Yield curve** - Nelson-Siegel: level/slope/curvature. The short end is
//!   the policy rate, the long end is neutral rate + expected inflation +
//!   term premium. When the bank hikes above the long anchor the curve
//!   inverts, and an inverted curve raises the odds of a downturn.
//!
//! Every state variable is a mean-reverting process with hard clamps, so
//! nothing can compound without bound. That is the structural fix for the
//! alpha's slide into inf/NaN after a few hundred sim days.

use crate::rng::Rng;

/// Real neutral rate (r*).
pub const R_STAR: f64 = 0.010;
pub const INFLATION_TARGET: f64 = 0.020;
/// Long-run real growth of the economy (and of aggregate earnings).
pub const TREND_GROWTH: f64 = 0.020;
pub const ERP_NEUTRAL: f64 = 0.045;

/// Tenors shown on the yield curve, in years.
pub const TENORS: [f64; 11] = [1.0 / 12.0, 0.25, 0.5, 1.0, 2.0, 3.0, 5.0, 7.0, 10.0, 20.0, 30.0];
pub const TENOR_LABELS: [&str; 11] = ["1M", "3M", "6M", "1Y", "2Y", "3Y", "5Y", "7Y", "10Y", "20Y", "30Y"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Regime {
    Calm,
    Bull,
    Correction,
    Crisis,
}

impl Regime {
    pub fn label(self) -> &'static str {
        match self {
            Regime::Calm => "Calm",
            Regime::Bull => "Bull",
            Regime::Correction => "Correction",
            Regime::Crisis => "Crisis",
        }
    }
    /// Scales every volatility in the market.
    pub fn vol_mult(self) -> f64 {
        match self {
            Regime::Calm => 0.85,
            Regime::Bull => 1.0,
            Regime::Correction => 1.4,
            Regime::Crisis => 2.1,
        }
    }
    pub fn jump_mult(self) -> f64 {
        match self {
            Regime::Calm => 0.8,
            Regime::Bull => 1.0,
            Regime::Correction => 1.6,
            Regime::Crisis => 3.0,
        }
    }
    /// Shift in the equity risk premium investors demand.
    fn erp_shift(self) -> f64 {
        match self {
            Regime::Calm => 0.0,
            Regime::Bull => -0.008,
            Regime::Correction => 0.012,
            Regime::Crisis => 0.030,
        }
    }
    /// Annual push on the output gap (demand shock).
    fn gap_push(self) -> f64 {
        match self {
            Regime::Calm => 0.0,
            Regime::Bull => 0.006,
            Regime::Correction => -0.010,
            Regime::Crisis => -0.040,
        }
    }
    fn spread_shift(self) -> f64 {
        match self {
            Regime::Calm => 0.0,
            Regime::Bull => -0.002,
            Regime::Correction => 0.006,
            Regime::Crisis => 0.025,
        }
    }
}

/// Nelson-Siegel yield curve: `y(tau) = L + S*f1(tau) + C*f2(tau)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct YieldCurve {
    pub level: f64,
    pub slope: f64,
    pub curvature: f64,
    /// Decay in years; where the curvature hump peaks.
    pub lambda: f64,
}

impl YieldCurve {
    fn loadings(&self, tau: f64) -> (f64, f64) {
        let x = (tau / self.lambda).max(1e-9);
        let e = (-x).exp();
        let f1 = (1.0 - e) / x;
        (f1, f1 - e)
    }

    /// Zero-coupon yield for maturity `tau` years.
    pub fn yield_at(&self, tau: f64) -> f64 {
        let (f1, f2) = self.loadings(tau);
        self.level + self.slope * f1 + self.curvature * f2
    }

    /// Instantaneous forward rate `tau` years ahead: the market's implied
    /// path for the short rate. Used as the policy-rate forecast.
    pub fn forward_at(&self, tau: f64) -> f64 {
        let x = tau / self.lambda;
        let e = (-x).exp();
        self.level + self.slope * e + self.curvature * x * e
    }

    pub fn points(&self) -> [f64; TENORS.len()] {
        TENORS.map(|t| self.yield_at(t))
    }

    /// 10Y minus 3M: the classic recession signal when negative.
    pub fn spread_10y_3m(&self) -> f64 {
        self.yield_at(10.0) - self.yield_at(0.25)
    }
}

#[derive(Clone, Debug)]
pub struct MacroState {
    pub output_gap: f64,
    pub inflation: f64,
    pub policy_rate: f64,
    pub term_premium: f64,
    pub curvature: f64,
    /// Equity risk premium: excess return investors demand over the 10Y.
    pub erp: f64,
    /// Corporate credit spread over government yields.
    pub credit_spread: f64,
    /// ln(money supply); kept in log space so it can never overflow.
    pub log_money: f64,
    pub regime: Regime,
    pub regime_days: u32,
    pub curve: YieldCurve,
    /// Business-cycle factor: the running integral of the output gap, in
    /// gap-years. Securities with a cycle exposure (the Finance perk) drift
    /// up while the economy runs hot and down in recessions.
    pub cycle_index: f64,
    /// Recovery factor: climbs at [`RECOVERY_RATE`] while a post-stress
    /// recovery window is open, flat otherwise (the Profit perk loads on it).
    pub recovery_index: f64,
    /// Sim days left in the current recovery window.
    pub recovery_days_left: u32,
}

/// Annual climb of the recovery factor while a recovery window is open.
pub const RECOVERY_RATE: f64 = 0.15;
/// Recovery window opened when a Crisis / a Correction gives way to calm.
pub const RECOVERY_DAYS_AFTER_CRISIS: u32 = 300;
pub const RECOVERY_DAYS_AFTER_CORRECTION: u32 = 90;

/// Exact Ornstein-Uhlenbeck step: valid for any `dt`, never overshoots.
#[inline]
pub fn ou_step(x: f64, target: f64, kappa: f64, sigma: f64, dt: f64, z: f64) -> f64 {
    let e = (-kappa * dt).exp();
    let sd = sigma * ((1.0 - e * e) / (2.0 * kappa)).sqrt();
    target + (x - target) * e + sd * z
}

impl MacroState {
    pub fn new() -> Self {
        let mut m = Self {
            output_gap: 0.0,
            inflation: INFLATION_TARGET,
            policy_rate: R_STAR + INFLATION_TARGET,
            term_premium: 0.008,
            curvature: 0.0,
            erp: ERP_NEUTRAL,
            credit_spread: 0.012,
            log_money: (21.0e12f64).ln(),
            regime: Regime::Calm,
            regime_days: 0,
            curve: YieldCurve { level: 0.0, slope: 0.0, curvature: 0.0, lambda: 1.6 },
            cycle_index: 0.0,
            recovery_index: 0.0,
            recovery_days_left: 0,
        };
        m.rebuild_curve();
        m
    }

    pub fn money_supply(&self) -> f64 {
        self.log_money.exp()
    }

    /// Anchored inflation expectations.
    pub fn expected_inflation(&self) -> f64 {
        0.75 * INFLATION_TARGET + 0.25 * self.inflation
    }

    /// Long-run nominal growth of earnings.
    pub fn long_growth(&self) -> f64 {
        TREND_GROWTH + self.expected_inflation()
    }

    pub fn y10(&self) -> f64 {
        self.curve.yield_at(10.0)
    }

    pub fn taylor_target(&self) -> f64 {
        R_STAR + self.inflation + 0.5 * (self.inflation - INFLATION_TARGET) + 0.5 * self.output_gap
    }

    /// Gordon growth model: fair price/earnings = payout / (r - g),
    /// with `r = 10Y + ERP`. Clamped so P/E stays in about 2.4..30.
    pub fn fair_pe(&self) -> f64 {
        const PAYOUT: f64 = 0.6;
        let denom = (self.y10() + self.erp - self.long_growth()).clamp(0.02, 0.25);
        PAYOUT / denom
    }

    fn rebuild_curve(&mut self) {
        let level = R_STAR + self.expected_inflation() + self.term_premium + 0.004;
        self.curve = YieldCurve { level, slope: self.policy_rate - level, curvature: self.curvature, lambda: 1.6 };
    }

    /// Advance the continuous macro variables by `dt` years.
    pub fn step(&mut self, dt: f64, rng: &mut Rng) {
        let sq = dt.sqrt();
        let real_rate = self.policy_rate - self.inflation;

        let gap_drift = -0.6 * self.output_gap - 0.5 * (real_rate - R_STAR) + self.regime.gap_push();
        self.output_gap = (self.output_gap + gap_drift * dt + 0.012 * sq * rng.normal()).clamp(-0.08, 0.06);

        let pi_e = self.expected_inflation();
        let pi_drift = 0.8 * (pi_e - self.inflation) + 0.3 * self.output_gap;
        self.inflation = (self.inflation + pi_drift * dt + 0.006 * sq * rng.normal()).clamp(-0.02, 0.12);

        // Policy smoothing: the bank closes ~78% of the gap to target per year.
        let target = self.taylor_target().max(0.0);
        self.policy_rate = ou_step(self.policy_rate, target, 1.5, 0.002, dt, rng.normal()).clamp(0.0, 0.15);

        let tp_target = 0.008 + if self.regime == Regime::Crisis { 0.004 } else { 0.0 };
        self.term_premium = ou_step(self.term_premium, tp_target, 1.0, 0.004, dt, rng.normal()).clamp(-0.01, 0.03);
        self.curvature = ou_step(self.curvature, 0.0, 1.0, 0.004, dt, rng.normal()).clamp(-0.02, 0.02);

        let erp_target = ERP_NEUTRAL + self.regime.erp_shift();
        self.erp = ou_step(self.erp, erp_target, 3.0, 0.004, dt, rng.normal()).clamp(0.02, 0.12);

        let cs_target = 0.012 + 0.4 * (-self.output_gap).max(0.0) + self.regime.spread_shift();
        self.credit_spread = ou_step(self.credit_spread, cs_target, 4.0, 0.003, dt, rng.normal()).clamp(0.003, 0.10);

        // Money grows with nominal GDP, faster when policy is loose.
        let neutral_nominal = R_STAR + INFLATION_TARGET;
        let m_growth = TREND_GROWTH + self.inflation + 0.8 * (neutral_nominal - self.policy_rate);
        self.log_money += m_growth * dt;

        // Slow factors used by perks (see the field docs).
        self.cycle_index += self.output_gap * dt;
        if self.recovery_days_left > 0 {
            self.recovery_index += RECOVERY_RATE * dt;
        }

        self.rebuild_curve();
    }

    /// Composite stress index (0 = relaxed) feeding regime transitions.
    pub fn stress(&self) -> f64 {
        let inversion = (-self.curve.spread_10y_3m()).max(0.0);
        20.0 * (-self.output_gap).max(0.0)
            + 30.0 * inversion
            + 15.0 * (self.inflation - 0.04).max(0.0)
            + 20.0 * (self.credit_spread - 0.02).max(0.0)
    }

    /// Once per sim day: maybe switch regime (a hidden Markov chain whose
    /// transition odds depend on stress). Returns the new regime on change.
    pub fn step_regime_daily(&mut self, rng: &mut Rng) -> Option<Regime> {
        let s = self.stress();
        let u = rng.uniform();
        use Regime::*;
        // (probability, destination) per day; cumulative check below.
        let moves: [(f64, Regime); 3] = match self.regime {
            Calm => [(0.004, Bull), (0.003 * (1.0 + s), Correction), (0.0005 * (1.0 + 2.0 * s), Crisis)],
            Bull => [(0.003, Calm), (0.004 * (1.0 + s), Correction), (0.0008 * (1.0 + 2.0 * s), Crisis)],
            Correction => [(0.020, Calm), (0.005, Bull), (0.006 * (1.0 + s), Crisis)],
            Crisis => [(0.012, Correction), (0.002, Calm), (0.0, Crisis)],
        };
        let mut acc = 0.0;
        for (p, dest) in moves {
            acc += p;
            if u < acc && dest != self.regime {
                // Stress giving way to calm opens a recovery window.
                if matches!(dest, Calm | Bull) {
                    let window = match self.regime {
                        Crisis => RECOVERY_DAYS_AFTER_CRISIS,
                        Correction => RECOVERY_DAYS_AFTER_CORRECTION,
                        _ => 0,
                    };
                    self.recovery_days_left = self.recovery_days_left.max(window);
                }
                self.regime = dest;
                self.regime_days = 0;
                return Some(dest);
            }
        }
        self.regime_days += 1;
        self.recovery_days_left = self.recovery_days_left.saturating_sub(1);
        None
    }
}

impl Default for MacroState {
    fn default() -> Self {
        Self::new()
    }
}

crate::tag_enum!(Regime { Calm = 0, Bull = 1, Correction = 2, Crisis = 3 });
crate::record!(YieldCurve { level, slope, curvature, lambda });
crate::record!(MacroState {
    output_gap,
    inflation,
    policy_rate,
    term_premium,
    curvature,
    erp,
    credit_spread,
    log_money,
    regime,
    regime_days,
    curve,
    cycle_index,
    recovery_index,
    recovery_days_left,
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_short_end_is_policy_rate() {
        let m = MacroState::new();
        assert!((m.curve.yield_at(1e-6) - m.policy_rate).abs() < 1e-6);
        // Long end approaches the level.
        assert!((m.curve.yield_at(1_000.0) - m.curve.level).abs() < 0.002);
    }

    #[test]
    fn hiking_above_level_inverts_curve() {
        let mut m = MacroState::new();
        m.policy_rate = 0.09;
        m.rebuild_curve();
        assert!(m.curve.spread_10y_3m() < 0.0);
    }

    #[test]
    fn ou_step_is_exact_in_the_limit() {
        // With zero noise, a huge dt lands exactly on the target.
        assert!((ou_step(5.0, 1.0, 2.0, 0.0, 1e6, 0.0) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn macro_stays_bounded_for_a_century() {
        let mut m = MacroState::new();
        let mut rng = Rng::seed_from_u64(77);
        let dt = 1.0 / 8_760.0; // hourly
        for hour in 0..(100 * 8_760) {
            m.step(dt, &mut rng);
            if hour % 24 == 0 {
                m.step_regime_daily(&mut rng);
            }
            for v in [m.output_gap, m.inflation, m.policy_rate, m.erp, m.credit_spread, m.y10(), m.fair_pe()] {
                assert!(v.is_finite());
            }
            assert!(m.fair_pe() > 2.0 && m.fair_pe() < 31.0);
        }
    }
}
