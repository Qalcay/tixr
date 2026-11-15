//! Exchanges and their market factor.
//!
//! Yes, Tixr-sigma still has several markets, and they now earn their keep:
//! each exchange has its own trading session (so the four together follow
//! the sun), its own volatility and growth tilt, and its own market factor.
//! The factors are correlated through a shared global shock, so a crisis hits
//! everyone, but not identically.
//!
//! The market factor is a valuation identity, not a random walk:
//!
//! ```text
//!   X = ln(Earnings) + ln(P/E)
//!   ln(Earnings) = nominal trend + 3 * output_gap + earnings noise (OU)
//!   ln(P/E)      = OU pulled toward ln(fair P/E from the yield curve)
//! ```
//!
//! So the index rises with nominal growth, falls when rates or the risk
//! premium rise, and can never drift arbitrarily far from what the economy
//! justifies. Each security's price loads on its exchange's X via beta.

use crate::macroecon::{MacroState, ou_step};
use crate::rng::Rng;
use crate::save::Persist;
use crate::time::Session;

pub const EXCHANGE_COUNT: usize = 4;
pub const INDEX_BASE: f64 = 1_000.0;

#[derive(Clone, Copy, Debug)]
pub struct ExchangeSpec {
    pub name: &'static str,
    pub session: Session,
    pub ticker_len: usize,
    pub vol_scale: f64,
    pub growth_tilt: f64,
}

/// Four venues spread around the clock (sim times are a single global clock).
pub const EXCHANGE_SPECS: [ExchangeSpec; EXCHANGE_COUNT] = [
    ExchangeSpec { name: "EX01", session: Session::hours(13.5, 20.0), ticker_len: 3, vol_scale: 1.0, growth_tilt: 0.0 },
    ExchangeSpec {
        name: "CHA2",
        session: Session::hours(1.5, 8.0),
        ticker_len: 4,
        vol_scale: 1.15,
        growth_tilt: 0.005,
    },
    ExchangeSpec {
        name: "4MAQ",
        session: Session::hours(8.0, 16.5),
        ticker_len: 4,
        vol_scale: 0.9,
        growth_tilt: -0.004,
    },
    ExchangeSpec {
        name: "AST8",
        session: Session::hours(16.0, 23.5),
        ticker_len: 4,
        vol_scale: 1.3,
        growth_tilt: 0.008,
    },
];

/// Correlation of each exchange's valuation shock with the global shock.
const GLOBAL_CORR: f64 = 0.65;
const PE_KAPPA: f64 = 2.0;
const PE_SIGMA: f64 = 0.13;
const EARN_NOISE_KAPPA: f64 = 0.5;
const EARN_NOISE_SIGMA: f64 = 0.05;
const GAP_EARNINGS_LEVERAGE: f64 = 3.0;

#[derive(Clone, Debug)]
pub struct Exchange {
    pub id: u8,
    pub spec: ExchangeSpec,
    /// Deterministic nominal earnings trend, integrated over time.
    pub trend: f64,
    pub earnings_noise: f64,
    pub log_pe: f64,
    /// The market factor `X` that securities load on.
    pub factor: f64,
    factor_at_start: f64,
    /// Index level at the last session close (for daily % change).
    pub prev_close_index: f64,
    pub is_open: bool,
}

impl Exchange {
    pub fn new(id: u8, macro_state: &MacroState, rng: &mut Rng) -> Self {
        let spec = EXCHANGE_SPECS[id as usize];
        let log_pe = macro_state.fair_pe().ln() + 0.05 * rng.normal();
        let mut ex = Self {
            id,
            spec,
            trend: 0.0,
            earnings_noise: 0.0,
            log_pe,
            factor: 0.0,
            factor_at_start: 0.0,
            prev_close_index: INDEX_BASE,
            is_open: false,
        };
        ex.factor = ex.compute_factor(macro_state);
        ex.factor_at_start = ex.factor;
        ex
    }

    fn compute_factor(&self, m: &MacroState) -> f64 {
        self.trend + GAP_EARNINGS_LEVERAGE * m.output_gap + self.earnings_noise + self.log_pe
    }

    pub fn index_level(&self) -> f64 {
        INDEX_BASE * (self.factor - self.factor_at_start).exp()
    }

    pub fn pe(&self) -> f64 {
        self.log_pe.exp()
    }

    /// Expected change in `X` over `h` years, used for forecasts: trend growth
    /// plus closing part of today's valuation gap.
    pub fn expected_factor_drift(&self, m: &MacroState, h: f64) -> f64 {
        let growth = (m.long_growth() + self.spec.growth_tilt) * h;
        let pe_gap = m.fair_pe().ln() - self.log_pe;
        growth + pe_gap * (1.0 - (-PE_KAPPA * h).exp())
    }

    /// Annualised volatility of `X` in the current regime (for forecasts).
    pub fn factor_vol(&self, m: &MacroState) -> f64 {
        let s = PE_SIGMA * m.regime.vol_mult() * self.spec.vol_scale;
        (s * s + EARN_NOISE_SIGMA * EARN_NOISE_SIGMA).sqrt()
    }

    /// Advance by `dt` years. `global_z` is shared by all exchanges this step.
    pub fn step(&mut self, m: &MacroState, dt: f64, global_z: f64, rng: &mut Rng) {
        let local_z = rng.normal();
        let z = GLOBAL_CORR * global_z + (1.0 - GLOBAL_CORR * GLOBAL_CORR).sqrt() * local_z;
        let vol = PE_SIGMA * m.regime.vol_mult() * self.spec.vol_scale;
        let target = m.fair_pe().ln();
        self.log_pe = ou_step(self.log_pe, target, PE_KAPPA, vol, dt, z).clamp(0.5, 3.6);
        self.earnings_noise =
            ou_step(self.earnings_noise, 0.0, EARN_NOISE_KAPPA, EARN_NOISE_SIGMA, dt, rng.normal()).clamp(-0.5, 0.5);
        self.trend += (m.long_growth() + self.spec.growth_tilt) * dt;
        self.factor = self.compute_factor(m);
    }
}

impl crate::save::Persist for Exchange {
    fn put(&self, w: &mut crate::save::Writer) {
        self.id.put(w);
        self.trend.put(w);
        self.earnings_noise.put(w);
        self.log_pe.put(w);
        self.factor.put(w);
        self.factor_at_start.put(w);
        self.prev_close_index.put(w);
        self.is_open.put(w);
    }
    fn get(r: &mut crate::save::Reader) -> crate::save::Result<Self> {
        let id = u8::get(r)?;
        // The spec is static data: looked up, never trusted from the file.
        let spec = *EXCHANGE_SPECS.get(id as usize).ok_or(crate::save::SaveError::Corrupt("exchange id"))?;
        Ok(Self {
            id,
            spec,
            trend: Persist::get(r)?,
            earnings_noise: Persist::get(r)?,
            log_pe: Persist::get(r)?,
            factor: Persist::get(r)?,
            factor_at_start: Persist::get(r)?,
            prev_close_index: Persist::get(r)?,
            is_open: Persist::get(r)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_starts_at_base_and_tracks_growth() {
        let m = MacroState::new();
        let mut rng = Rng::seed_from_u64(1);
        let mut ex = Exchange::new(0, &m, &mut rng);
        assert!((ex.index_level() - INDEX_BASE).abs() < 1e-9);
        // 10 years of minute steps with no noise beyond the model's own.
        let dt = 1.0 / (365.0 * 24.0 * 60.0);
        for _ in 0..(10 * 365 * 24 * 6) {
            ex.step(&m, dt * 10.0, rng.normal(), &mut rng);
        }
        let level = ex.index_level();
        assert!(level.is_finite() && level > 300.0 && level < 6_000.0, "level {level}");
    }

    #[test]
    fn higher_rates_mean_lower_fair_pe() {
        let mut m = MacroState::new();
        let base = m.fair_pe();
        m.erp += 0.02;
        assert!(m.fair_pe() < base);
    }
}
