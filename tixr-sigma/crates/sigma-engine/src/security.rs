//! Securities and the closed-form price model.
//!
//! # Pricing model
//! ```text
//!   ln P = base + Σ_k e_k * F_k(t)  +  y(t)  +  u(t)
//!          ^^^^^^^^^^^^^^^^^^^^^^^     ^^^^     ^^^^
//!          systematic factors          firm     mispricing
//!          (market X, rates, inflation,fundamental (OU, mean-reverting)
//!           credit) - shared state     (drifting
//!                                       random walk + jumps)
//! ```
//! * Factors are global state. A security stores the factor values it last
//!   saw (`anchors`), so catching up after any gap is just
//!   `Σ e_k (F_k(now) - anchor_k)`: every security co-moves with the market
//!   exactly, no matter when (or how rarely) it is updated.
//! * `y` is a Brownian motion with drift plus compound-Poisson jumps
//!   (earnings surprises). Its increment over any gap is sampled exactly.
//! * `u` is an Ornstein-Uhlenbeck process: the "restoring force" pulling the
//!   price back to fair value. Order-flow impact is added to `u`, so it fades
//!   at the same rate (transient impact, as observed in real markets). Its
//!   exact transition holds for any gap length, so a stock untouched over a
//!   weekend opens with a correctly-sized gap and never explodes.
//!
//! Because every component has an exact transition, the time between updates
//! is a free parameter. That is what lets the engine update securities only
//! when their own random event fires.

use std::collections::VecDeque;

use crate::exchange::EXCHANGE_SPECS;
use crate::persona::{Perk, Persona};
use crate::rng::Rng;
use crate::time::{SimTime, years};

pub const FACTOR_COUNT: usize = 4;
/// Factor order: market X, rates (-10Y), inflation, credit (-spread).
pub type Factors = [f64; FACTOR_COUNT];

pub const DAILY_HISTORY: usize = 390;
pub const INTRADAY_HISTORY: usize = 240;
pub const PENNY_PRICE: f64 = 1.0;
pub const PENNY_DAYS_TO_DELIST: u16 = 20;
pub const SPLIT_TRIGGER: f64 = 2_500.0;
pub const SPLIT_RATIO: f64 = 10.0;
pub const MAX_MISPRICING: f64 = 1.2;

/// Generational index: low 16 bits = slot, high 16 bits = generation.
/// A recycled slot gets a new generation, so holdings of a dead security can
/// never silently transfer to its replacement (an alpha bug).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SecurityId(pub u32);

impl SecurityId {
    pub fn new(slot: u16, generation: u16) -> Self {
        Self(((generation as u32) << 16) | slot as u32)
    }
    pub fn slot(self) -> usize {
        (self.0 & 0xFFFF) as usize
    }
    pub fn generation(self) -> u16 {
        (self.0 >> 16) as u16
    }
}

/// Up to 5 ASCII letters, stored inline (no heap allocation per ticker).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ticker {
    bytes: [u8; 5],
    len: u8,
}

impl Ticker {
    pub fn new(s: &str) -> Self {
        let mut bytes = [b' '; 5];
        let len = s.len().min(5);
        bytes[..len].copy_from_slice(&s.as_bytes()[..len]);
        Self { bytes, len: len as u8 }
    }
    pub fn as_str(&self) -> &str {
        // Only ever built from ASCII letters.
        std::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("?")
    }
    pub fn random(rng: &mut Rng, len: usize, suffix: Option<u8>) -> Self {
        let mut bytes = [b' '; 5];
        for b in bytes.iter_mut().take(len) {
            *b = b'A' + rng.below(26) as u8;
        }
        if let Some(s) = suffix {
            bytes[len - 1] = s;
        }
        Self { bytes, len: len as u8 }
    }
}

impl std::fmt::Debug for Ticker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
impl std::fmt::Display for Ticker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecurityKind {
    Stock,
    /// An index fund: beta 1 to its exchange, tiny idiosyncratic risk.
    Fund,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sponsor {
    Engine,
    Firm(crate::broker::BrokerId),
    Player,
}

/// Model parameters, fixed at listing. Annualised unless noted.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Exposures to [market, rates, inflation, credit].
    pub exposures: Factors,
    /// Drift of the firm fundamental `y`.
    pub alpha: f64,
    pub sigma_f: f64,
    /// Mean-reversion speed of mispricing; half-life = ln 2 / kappa years.
    pub kappa: f64,
    pub sigma_u: f64,
    pub jump_rate: f64,
    pub jump_mean: f64,
    pub jump_sd: f64,
    /// Price-update events per sim second while the exchange is open.
    pub tick_rate: f64,
    /// Square-root-law impact coefficient.
    pub impact: f64,
    /// Default when fair value falls this many log units below listing.
    pub barrier: f64,
    pub penny_exempt: bool,
    /// Fraction of market cap that trades per day (for impact scaling).
    pub turnover: f64,
}

impl Params {
    pub fn draw(kind: SecurityKind, persona: Persona, exchange: u8, rng: &mut Rng) -> Self {
        let e = persona.effect();
        let spec = &EXCHANGE_SPECS[exchange as usize];
        match kind {
            SecurityKind::Fund => Params {
                exposures: [1.0, 1.5, 0.0, 0.5],
                alpha: -0.002,
                sigma_f: 0.02,
                kappa: 40.0,
                sigma_u: 0.03,
                jump_rate: 0.0,
                jump_mean: 0.0,
                jump_sd: 0.0,
                tick_rate: rng.range(0.2, 0.5),
                impact: 0.05,
                barrier: 50.0,
                penny_exempt: true,
                turnover: 0.01,
            },
            SecurityKind::Stock => {
                let beta = (rng.range(0.55, 1.45) + e.beta_add).clamp(0.1, 2.4);
                let duration = (rng.range(1.0, 4.0) + e.duration_add).clamp(-2.0, 12.0);
                let infl = (rng.range(-1.0, 0.5) + e.inflation_add).clamp(-3.0, 5.0);
                let credit = (rng.range(0.3, 1.5) + e.credit_add).clamp(0.0, 6.0);
                let alpha = (0.04 * rng.normal() - 0.005 + e.alpha_add).clamp(-0.15, 0.15);
                let sigma_f = (rng.range(0.10, 0.30) * spec.vol_scale.sqrt() * e.sigma_f_mul).clamp(0.04, 0.6);
                let kappa = (rng.range(5.0, 16.0) * e.kappa_mul).clamp(1.5, 40.0);
                let sigma_u = (rng.range(0.15, 0.40) * e.sigma_u_mul).clamp(0.1, 1.2);
                let jump_rate = (rng.range(0.5, 2.0) * e.jump_rate_mul).clamp(0.0, 12.0);
                let jump_mean = (-0.02 + e.jump_mean_add).clamp(-0.08, 0.08);
                let tick_rate = (rng.range(0.08, 0.5) * e.rate_mul).clamp(0.01, 2.0);
                let impact = (rng.range(0.6, 1.2) * e.impact_mul).clamp(0.1, 4.0);
                let barrier = (rng.range(1.1, 2.0) + e.barrier_add).clamp(0.4, 4.0);
                Params {
                    exposures: [beta, duration, infl, credit],
                    alpha,
                    sigma_f,
                    kappa,
                    sigma_u,
                    jump_rate,
                    jump_mean,
                    jump_sd: 0.06,
                    tick_rate,
                    impact,
                    barrier,
                    penny_exempt: persona.has(Perk::Phoenix),
                    turnover: (0.004 * e.rate_mul).clamp(0.001, 0.02),
                }
            }
        }
    }

    pub fn beta(&self) -> f64 {
        self.exposures[0]
    }

    /// Stationary standard deviation of the mispricing `u`.
    pub fn mispricing_sd(&self) -> f64 {
        self.sigma_u / (2.0 * self.kappa).sqrt()
    }
}

#[derive(Clone, Debug)]
pub struct Security {
    pub id: SecurityId,
    pub ticker: Ticker,
    pub kind: SecurityKind,
    pub exchange: u8,
    pub persona: Persona,
    pub params: Params,
    pub sponsor: Sponsor,
    pub listed_at: SimTime,

    // --- model state ---
    pub base: f64,
    pub y: f64,
    pub u: f64,
    pub anchors: Factors,
    pub last_update: SimTime,
    /// `y + Σ e·F` at listing; the default barrier is measured from here.
    pub fair_at_listing: f64,

    // --- market data ---
    pub log_price: f64,
    pub price: f64,
    pub shares: f64,
    pub prev_close: f64,
    pub day_open: f64,
    pub day_high: f64,
    pub day_low: f64,
    pub volume_today: f64,
    pub ticks_today: u32,
    pub penny_days: u16,
    pub closes: VecDeque<f32>,
    pub intraday: VecDeque<f32>,
}

/// Why a security left the market.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    Bankrupt,
    PennyDelisted,
    Acquired,
    FundClosed,
}

impl Exit {
    /// Cash paid to holders per share, as a multiple of the last price.
    pub fn payout_multiple(self) -> f64 {
        match self {
            Exit::Bankrupt => 0.04,
            Exit::PennyDelisted => 0.5,
            Exit::Acquired => 1.3,
            Exit::FundClosed => 1.0,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Exit::Bankrupt => "bankrupt",
            Exit::PennyDelisted => "delisted (penny)",
            Exit::Acquired => "acquired",
            Exit::FundClosed => "fund closed",
        }
    }
}

pub struct ListingSpec {
    pub kind: SecurityKind,
    pub exchange: u8,
    pub persona: Persona,
    pub ticker: Ticker,
    pub ipo_price: f64,
    pub market_cap: f64,
    pub sponsor: Sponsor,
    /// Pre-aged state for securities created at world start.
    pub initial_y: f64,
}

impl Security {
    pub fn list(id: SecurityId, spec: ListingSpec, factors: &Factors, now: SimTime, rng: &mut Rng) -> Self {
        let params = Params::draw(spec.kind, spec.persona, spec.exchange, rng);
        let u = 0.5 * params.mispricing_sd() * rng.normal();
        let systematic: f64 = params.exposures.iter().zip(factors).map(|(e, f)| e * f).sum();
        let log_price = spec.ipo_price.ln();
        // Pre-aged losers start closer to default, but never already past it.
        let y = spec.initial_y.max(-0.6 * params.barrier);
        let base = log_price - systematic - y - u;
        let price = spec.ipo_price;
        let mut closes = VecDeque::with_capacity(DAILY_HISTORY);
        closes.push_back(price as f32);
        Self {
            id,
            ticker: spec.ticker,
            kind: spec.kind,
            exchange: spec.exchange,
            persona: spec.persona,
            params,
            sponsor: spec.sponsor,
            listed_at: now,
            base,
            y,
            u,
            anchors: *factors,
            last_update: now,
            // Measure the barrier from the *un-aged* fundamental so pre-aged
            // losers at world start are already closer to default.
            fair_at_listing: systematic,
            log_price,
            price,
            shares: (spec.market_cap / price).max(1.0),
            prev_close: price,
            day_open: price,
            day_high: price,
            day_low: price,
            volume_today: 0.0,
            ticks_today: 0,
            penny_days: 0,
            closes,
            intraday: VecDeque::with_capacity(INTRADAY_HISTORY),
        }
    }

    pub fn market_cap(&self) -> f64 {
        self.price * self.shares
    }

    pub fn systematic(&self) -> f64 {
        self.params.exposures.iter().zip(&self.anchors).map(|(e, f)| e * f).sum()
    }

    pub fn log_fair_value(&self) -> f64 {
        self.base + self.systematic() + self.y
    }

    pub fn fair_value(&self) -> f64 {
        self.log_fair_value().exp()
    }

    /// Distance (log units) above the default barrier.
    pub fn distance_to_default(&self) -> f64 {
        (self.systematic() + self.y - self.fair_at_listing) + self.params.barrier
    }

    /// Typical dollar volume per day, for the square-root impact law.
    pub fn daily_dollar_volume(&self) -> f64 {
        (self.market_cap() * self.params.turnover).max(1_000.0)
    }

    /// Exact catch-up from `last_update` to `now`.
    /// `vol_mult`/`jump_mult` come from the regime in force now (a standard
    /// piecewise-constant approximation; regimes last weeks, gaps hours).
    pub fn advance(&mut self, now: SimTime, factors: &Factors, vol_mult: f64, jump_mult: f64, rng: &mut Rng) {
        let dt = years(now - self.last_update).max(0.0);
        self.last_update = now;
        self.anchors = *factors;
        if dt > 0.0 {
            let p = &self.params;
            // Fundamental: Brownian motion with drift (Ito-corrected).
            let sf = p.sigma_f * vol_mult;
            self.y += (p.alpha - 0.5 * sf * sf) * dt + sf * dt.sqrt() * rng.normal();
            // Jumps: number in the gap is Poisson; their sum is normal.
            let n = rng.poisson(p.jump_rate * jump_mult * dt);
            if n > 0 {
                let nf = n as f64;
                self.y += p.jump_mean * nf + p.jump_sd * nf.sqrt() * rng.normal();
            }
            // Mispricing: exact OU transition toward zero.
            let e = (-p.kappa * dt).exp();
            let sd = p.sigma_u * vol_mult * ((1.0 - e * e) / (2.0 * p.kappa)).sqrt();
            self.u = (self.u * e + sd * rng.normal()).clamp(-MAX_MISPRICING, MAX_MISPRICING);
        }
        self.reprice();
    }

    /// Recompute price from state; repairs (and reports) non-finite values.
    /// Returns `false` if a repair was needed.
    pub fn reprice(&mut self) -> bool {
        let lp = self.base + self.systematic() + self.y + self.u;
        if lp.is_finite() {
            // e^±60 keeps prices in a printable range even in absurd runs.
            self.log_price = lp.clamp(-30.0, 30.0);
            self.price = self.log_price.exp();
            true
        } else {
            // Never let one bad number poison the whole market.
            self.u = 0.0;
            if !self.y.is_finite() {
                self.y = 0.0;
            }
            self.log_price = self.prev_close.max(0.01).ln();
            self.base = self.log_price - self.systematic() - self.y;
            self.price = self.prev_close.max(0.01);
            false
        }
    }

    /// Record an intraday print and day stats.
    pub fn record_tick(&mut self) {
        let p = self.price;
        if self.ticks_today == 0 {
            self.day_open = p;
            self.day_high = p;
            self.day_low = p;
        }
        self.day_high = self.day_high.max(p);
        self.day_low = self.day_low.min(p);
        self.ticks_today += 1;
        if self.intraday.len() == INTRADAY_HISTORY {
            self.intraday.pop_front();
        }
        self.intraday.push_back(p as f32);
    }

    /// End-of-session bookkeeping. Returns `Some(exit)` if it must delist.
    pub fn close_session(&mut self) -> Option<Exit> {
        if self.closes.len() == DAILY_HISTORY {
            self.closes.pop_front();
        }
        self.closes.push_back(self.price as f32);
        self.prev_close = self.price;
        self.volume_today = 0.0;
        self.ticks_today = 0;
        self.intraday.clear();
        if self.price < PENNY_PRICE && !self.params.penny_exempt {
            self.penny_days += 1;
            if self.penny_days >= PENNY_DAYS_TO_DELIST {
                return Some(Exit::PennyDelisted);
            }
        } else {
            self.penny_days = 0;
        }
        None
    }

    /// Momentum signal: log return over the last `days` closes.
    pub fn momentum(&self, days: usize) -> f64 {
        let n = self.closes.len();
        if n <= days {
            return 0.0;
        }
        let then = self.closes[n - 1 - days] as f64;
        if then > 0.0 { (self.price / then).ln() } else { 0.0 }
    }

    /// Apply a forward split; returns the ratio so holders can be adjusted.
    pub fn maybe_split(&mut self) -> Option<f64> {
        if self.price < SPLIT_TRIGGER {
            return None;
        }
        let r = SPLIT_RATIO;
        self.base -= r.ln();
        self.shares *= r;
        self.reprice();
        self.prev_close /= r;
        for c in self.closes.iter_mut() {
            *c /= r as f32;
        }
        Some(r)
    }

    /// Analytic forecast of ln P at horizon `h` years: (mean, std dev).
    /// `factor_drift`/`factor_vol` describe the market factor over `h`.
    pub fn forecast_log(&self, h: f64, factor_drift: f64, factor_vol: f64, vol_mult: f64) -> (f64, f64) {
        let p = &self.params;
        let beta = p.beta();
        let sf = p.sigma_f * vol_mult;
        let e = (-p.kappa * h).exp();
        let mean = self.log_price + beta * factor_drift + (p.alpha - 0.5 * sf * sf + p.jump_rate * p.jump_mean) * h
            - self.u * (1.0 - e);
        let su = p.sigma_u * vol_mult;
        let var = beta * beta * factor_vol * factor_vol * h
            + sf * sf * h
            + p.jump_rate * (p.jump_sd * p.jump_sd + p.jump_mean * p.jump_mean) * h
            + su * su * (1.0 - e * e) / (2.0 * p.kappa);
        (mean, var.max(0.0).sqrt())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persona::Persona;

    fn make(rng: &mut Rng) -> Security {
        Security::list(
            SecurityId::new(1, 0),
            ListingSpec {
                kind: SecurityKind::Stock,
                exchange: 0,
                persona: Persona::EMPTY,
                ticker: Ticker::new("ABC"),
                ipo_price: 50.0,
                market_cap: 1e9,
                sponsor: Sponsor::Engine,
                initial_y: 0.0,
            },
            &[0.0; 4],
            0.0,
            rng,
        )
    }

    #[test]
    fn ids_pack_and_unpack() {
        let id = SecurityId::new(65_535, 7);
        assert_eq!(id.slot(), 65_535);
        assert_eq!(id.generation(), 7);
        assert_ne!(SecurityId::new(3, 1), SecurityId::new(3, 2));
    }

    #[test]
    fn listing_prices_at_ipo() {
        let mut rng = Rng::seed_from_u64(1);
        let s = make(&mut rng);
        assert!((s.price - 50.0).abs() < 1e-9);
    }

    #[test]
    fn factor_moves_pass_through_with_beta() {
        let mut rng = Rng::seed_from_u64(2);
        let mut s = make(&mut rng);
        let before = s.log_price;
        // Zero elapsed time: only the factor change can move the price.
        s.advance(0.0, &[0.1, 0.0, 0.0, 0.0], 1.0, 1.0, &mut rng);
        assert!((s.log_price - before - 0.1 * s.params.beta()).abs() < 1e-12);
    }

    /// The core claim of path A: the distribution of a one-day move does not
    /// depend on how many updates happen inside the day.
    #[test]
    fn variance_is_independent_of_update_frequency() {
        let day = 86_400.0;
        let var_of = |steps: usize, seed: u64| {
            let mut rng = Rng::seed_from_u64(seed);
            let trials = 4_000;
            let (mut s1, mut s2) = (0.0, 0.0);
            for _ in 0..trials {
                let mut s = make(&mut rng);
                s.u = 0.0;
                s.reprice();
                let p0 = s.log_price;
                for k in 1..=steps {
                    s.advance(day * k as f64 / steps as f64, &[0.0; 4], 1.0, 1.0, &mut rng);
                }
                let r = s.log_price - p0;
                s1 += r;
                s2 += r * r;
            }
            s2 / trials as f64 - (s1 / trials as f64).powi(2)
        };
        let coarse = var_of(1, 10);
        let fine = var_of(200, 11);
        let ratio = coarse / fine;
        assert!((0.88..1.12).contains(&ratio), "coarse {coarse} fine {fine}");
    }

    #[test]
    fn mispricing_reverts() {
        let mut rng = Rng::seed_from_u64(3);
        let mut s = make(&mut rng);
        s.params.sigma_u = 0.0;
        s.u = 0.5;
        // Five half-lives later, ~3% of the shock should remain.
        let half_life_secs = (2f64.ln() / s.params.kappa) * crate::time::SECS_PER_YEAR;
        s.advance(5.0 * half_life_secs, &[0.0; 4], 1.0, 1.0, &mut rng);
        assert!((s.u - 0.5 / 32.0).abs() < 1e-9, "u {}", s.u);
    }

    #[test]
    fn split_preserves_market_cap() {
        let mut rng = Rng::seed_from_u64(4);
        let mut s = make(&mut rng);
        s.base += (3_000.0f64 / 50.0).ln();
        s.reprice();
        let cap = s.market_cap();
        assert_eq!(s.maybe_split(), Some(SPLIT_RATIO));
        assert!((s.market_cap() / cap - 1.0).abs() < 1e-9);
        assert!(s.price < SPLIT_TRIGGER);
    }
}
