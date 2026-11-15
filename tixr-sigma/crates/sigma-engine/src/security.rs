//! Securities and the closed-form price model.
//!
//! # Pricing model
//! ```text
//!   ln P = base + Σ_k e_k * F_k(t)  +  life(age)  +  y(t)  +  u(t)
//!          ^^^^^^^^^^^^^^^^^^^^^^^     ^^^^^^^^^     ^^^^     ^^^^
//!          systematic factors          lifecycle     firm     mispricing
//!          (shared global state)       (Hype hump,   fundamental (OU, mean-
//!                                       Legacy decay) (random walk  reverting)
//!                                                     + jumps)
//! ```
//! * Factors are global state. A security stores the factor values it last
//!   saw (`anchors`), so catching up after any gap is just
//!   `Σ e_k (F_k(now) - anchor_k)`: every security co-moves with the market
//!   exactly, no matter when (or how rarely) it is updated.
//! * `life` is a deterministic function of company age (closed form).
//! * `y` is a Brownian motion with drift plus compound-Poisson jumps
//!   (earnings surprises, contract wins, a Novel stock's verdict). Its
//!   increment over any gap is sampled exactly.
//! * `u` is an Ornstein-Uhlenbeck process: the "restoring force" pulling the
//!   price back to fair value. Order-flow impact, Innovate catalysts and
//!   Opportunity crashes are shocks added to `u`, so they fade at the same
//!   rate. Its exact transition holds for any gap length, so a stock
//!   untouched over a weekend opens with a correctly-sized gap.
//!
//! Because every component has an exact transition, the time between updates
//! is a free parameter. That is what lets the engine update securities only
//! when their own random event fires.
//!
//! # Why no stack of perks can flatline, oscillate or saw-tooth
//! * **Flatline** needs every noise source near zero. `Params::draw` clamps
//!   `sigma_f >= 0.05`, `sigma_u >= 0.10` and total beta `>= 0.15` after all
//!   multipliers are applied, so the quietest possible stock still moves
//!   about 8 % a year.
//! * **Sine waves** need a second-order (inertial) restoring force. The only
//!   restoring force here is first-order (OU): it decays toward fair value
//!   and mathematically cannot overshoot, whatever `kappa` is.
//! * **Saw-teeth** need regular shocks that are large against the noise they
//!   sit in. All shock processes are Poisson (irregular) with random sizes,
//!   and every shock size is *defined relative to that stock's own noise*
//!   (a contract win is 0.55 sigma_f, a catalyst 0.9 mispricing-sd), so
//!   damping perks shrink the shocks by the same factor as the noise.
//!
//! The tests at the bottom check this for every one of the 65,536 perk sets.

use crate::exchange::EXCHANGE_SPECS;
use crate::history::{History, HistoryKind};
use crate::persona::Persona;
use crate::rng::Rng;
use crate::time::{SECS_PER_YEAR, SimTime, years};

pub const FACTOR_COUNT: usize = 7;
/// Factor order; see the `F_*` indices.
pub type Factors = [f64; FACTOR_COUNT];
/// Home exchange's market factor X.
pub const F_MARKET: usize = 0;
/// Minus the 10-year yield (exposure = duration).
pub const F_RATES: usize = 1;
pub const F_INFLATION: usize = 2;
/// Minus the credit spread.
pub const F_CREDIT: usize = 3;
/// Average of all exchanges' market factors (the Global perk).
pub const F_GLOBAL: usize = 4;
/// Integrated output gap (the Finance perk).
pub const F_CYCLE: usize = 5;
/// Post-stress recovery index (the Profit perk).
pub const F_RECOVERY: usize = 6;

/// Closes kept for broker momentum signals. This is simulation state (it
/// changes decisions), unlike the chart [`History`], which is display only.
pub const RECENT_CLOSES: usize = 8;
pub const PENNY_PRICE: f64 = 1.0;
pub const PENNY_DAYS_TO_DELIST: u16 = 20;
pub const SPLIT_TRIGGER: f64 = 2_500.0;
pub const SPLIT_RATIO: f64 = 10.0;
pub const MAX_MISPRICING: f64 = 1.2;
/// Cap on shocks sampled individually per catch-up (keeps a pathological
/// multi-year gap from looping for long; the expected count is far lower).
const MAX_SHOCKS_PER_GAP: u32 = 64;
/// Largest long-run average mispricing the Innovate / Opportunity shock
/// processes may cause (log units), whatever else is stacked on the stock.
const MAX_CATALYST_BIAS: f64 = 0.04;
const MAX_CRASH_BIAS: f64 = 0.10;

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
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// Exposures to the factors, indexed by the `F_*` constants.
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
    /// Builder: contract wins per year and their mean size (permanent, up).
    pub win_rate: f64,
    pub win_mean: f64,
    /// Innovate: sentiment catalysts per year and mean size (transient, up).
    pub catalyst_rate: f64,
    pub catalyst_mean: f64,
    /// Opportunity: sentiment crashes per year and mean depth (transient).
    pub crash_rate: f64,
    pub crash_mean: f64,
    /// Share of the regime's volatility multiplier that applies (Global).
    pub regime_sens: f64,
    /// Hype: height (log units) and time-to-peak (years) of the listing hump.
    pub hype_amp: f64,
    pub hype_tau: f64,
    /// Legacy: terminal decline per year, reached `decline_ramp` years after
    /// a `decline_grace` period.
    pub decline_rate: f64,
    pub decline_grace: f64,
    pub decline_ramp: f64,
    /// Novel: sim time of the verdict (infinite = none), its signed size,
    /// and the volatility multiplier that applies before it.
    pub verdict_at: SimTime,
    pub verdict_size: f64,
    pub pre_verdict_vol: f64,
    /// Price-update events per sim second while the exchange is open.
    pub tick_rate: f64,
    /// Square-root-law impact coefficient.
    pub impact: f64,
    /// Default when fair value falls this many log units below listing.
    pub barrier: f64,
    /// Key: rescued instead of going bankrupt or being penny-delisted.
    pub rescuable: bool,
    /// Fraction of market cap that trades per day (for impact scaling).
    pub turnover: f64,
}

impl Params {
    /// Draw parameters for a new listing. `born_at` is when the company was
    /// founded (equal to the listing time for IPOs, earlier for the
    /// pre-aged population at world start); lifecycle perks count from it.
    pub fn draw(kind: SecurityKind, persona: Persona, exchange: u8, born_at: SimTime, rng: &mut Rng) -> Self {
        let e = persona.effect();
        let spec = &EXCHANGE_SPECS[exchange as usize];
        match kind {
            SecurityKind::Fund => Params {
                exposures: [1.0, 1.5, 0.0, 0.5, 0.0, 0.0, 0.0],
                alpha: -0.002,
                sigma_f: 0.02,
                kappa: 40.0,
                sigma_u: 0.03,
                jump_rate: 0.0,
                jump_mean: 0.0,
                jump_sd: 0.0,
                win_rate: 0.0,
                win_mean: 0.0,
                catalyst_rate: 0.0,
                catalyst_mean: 0.0,
                crash_rate: 0.0,
                crash_mean: 0.0,
                regime_sens: 1.0,
                hype_amp: 0.0,
                hype_tau: 1.0,
                decline_rate: 0.0,
                decline_grace: 0.0,
                decline_ramp: 1.0,
                verdict_at: f64::INFINITY,
                verdict_size: 0.0,
                pre_verdict_vol: 1.0,
                tick_rate: rng.range(0.2, 0.5),
                impact: 0.05,
                barrier: 50.0,
                rescuable: true,
                turnover: 0.01,
            },
            SecurityKind::Stock => {
                // --- shared knobs: draw, apply the stack, then clamp ---
                let beta = (rng.range(0.55, 1.45) + e.beta_add).clamp(0.15, 2.4);
                let global = e.global_share.clamp(0.0, 0.8);
                let duration = (rng.range(1.0, 4.0) + e.duration_add).clamp(-2.0, 12.0);
                let infl = ((rng.range(-1.0, 0.5) + e.inflation_add) * e.inflation_mul).clamp(-3.0, 5.0);
                let credit = (rng.range(0.3, 1.5) + e.credit_add).clamp(0.0, 6.0);
                let sigma_f = (rng.range(0.10, 0.30) * spec.vol_scale.sqrt() * e.sigma_f_mul).clamp(0.05, 0.6);
                let kappa = (rng.range(5.0, 16.0) * e.kappa_mul).clamp(2.0, 36.0);
                let sigma_u = (rng.range(0.15, 0.40) * e.sigma_u_mul).clamp(0.10, 1.0);
                let mispricing_sd = sigma_u / (2.0 * kappa).sqrt();
                let jump_rate = (rng.range(0.5, 2.0) * e.jump_rate_mul).clamp(0.1, 10.0);
                let jump_mean = (-0.02 + e.jump_mean_add).clamp(-0.06, 0.04);
                let jump_sd = (0.06 * e.jump_sd_mul).clamp(0.03, 0.16);
                let tick_rate = (rng.range(0.08, 0.5) * e.rate_mul).clamp(0.02, 2.0);
                let impact = (rng.range(0.6, 1.2) * e.impact_mul).clamp(0.1, 4.0);
                let barrier = (rng.range(1.1, 2.0) + e.barrier_add).clamp(0.5, 4.0);
                let base_alpha = ((0.04 * rng.normal() - 0.005 + e.alpha_add) * e.alpha_mul).clamp(-0.12, 0.14);

                // --- signature mechanisms. Every shock size is a multiple of
                // this stock's own noise, so damping perks damp the shocks
                // too and no stack can turn them into a visible saw-tooth. ---
                let (win_rate, win_mean) =
                    if e.wins > 0.0 { (rng.range(0.8, 1.6), 0.55 * sigma_f) } else { (0.0, 0.0) };
                // Sentiment shocks pile up when reversion is slow (their
                // long-run average is rate * size / kappa), so the size is
                // also capped to keep that average small: a stock stacked
                // with Hype's slow reversion must not sit 20 % above fair
                // value forever because it also has Innovate.
                let (catalyst_rate, catalyst_mean) = if e.catalysts > 0.0 {
                    let rate = rng.range(4.0, 7.0);
                    (rate, (0.9 * mispricing_sd).min(MAX_CATALYST_BIAS * kappa / rate))
                } else {
                    (0.0, 0.0)
                };
                let (crash_rate, crash_mean) = if e.crashes > 0.0 {
                    let rate = rng.range(0.5, 0.9);
                    (rate, (4.5 * mispricing_sd).clamp(0.12, 0.40).min(MAX_CRASH_BIAS * kappa / rate))
                } else {
                    (0.0, 0.0)
                };
                let (hype_amp, hype_tau) = if e.hype > 0.0 {
                    (rng.range(0.35, 0.8) * (sigma_f / 0.2).clamp(0.6, 1.3), rng.range(0.25, 0.6))
                } else {
                    (0.0, 1.0)
                };
                let (decline_rate, decline_grace, decline_ramp) = if e.decline > 0.0 {
                    // Never steeper than ~45 % of the stock's own volatility:
                    // a decline you can see in the trend, not a ruler line.
                    (rng.range(0.04, 0.09).min(0.45 * sigma_f.max(0.09)), rng.range(2.0, 5.0), rng.range(3.0, 5.0))
                } else {
                    (0.0, 0.0, 1.0)
                };
                let (verdict_at, verdict_size, pre_verdict_vol) = if e.verdict > 0.0 {
                    let delay_years = 0.1 + rng.exponential(1.0 / 1.5);
                    let sign = if rng.chance(0.5) { 1.0 } else { -1.0 };
                    (born_at + delay_years * SECS_PER_YEAR, sign * rng.range(0.45, 0.85), 1.4)
                } else {
                    (f64::INFINITY, 0.0, 1.0)
                };
                // Contract wins are partly "pre-paid" by a lower drift
                // between them, so Builder is lumpy rather than simply rich.
                let alpha = base_alpha - 0.6 * win_rate * win_mean;

                let mut exposures = [0.0; FACTOR_COUNT];
                exposures[F_MARKET] = beta * (1.0 - global);
                exposures[F_GLOBAL] = beta * global;
                exposures[F_RATES] = duration;
                exposures[F_INFLATION] = infl;
                exposures[F_CREDIT] = credit;
                exposures[F_CYCLE] = if e.cycle > 0.0 { rng.range(3.0, 6.0) } else { 0.0 };
                exposures[F_RECOVERY] = if e.recovery > 0.0 { rng.range(0.7, 1.3) } else { 0.0 };
                Params {
                    exposures,
                    alpha,
                    sigma_f,
                    kappa,
                    sigma_u,
                    jump_rate,
                    jump_mean,
                    jump_sd,
                    win_rate,
                    win_mean,
                    catalyst_rate,
                    catalyst_mean,
                    crash_rate,
                    crash_mean,
                    regime_sens: e.regime_sens_mul.clamp(0.3, 1.0),
                    hype_amp,
                    hype_tau,
                    decline_rate,
                    decline_grace,
                    decline_ramp,
                    verdict_at,
                    verdict_size,
                    pre_verdict_vol,
                    tick_rate,
                    impact,
                    barrier,
                    rescuable: e.rescue,
                    turnover: (0.004 * e.rate_mul).clamp(0.001, 0.02),
                }
            }
        }
    }

    /// Total market beta (home exchange plus world).
    pub fn beta(&self) -> f64 {
        self.exposures[F_MARKET] + self.exposures[F_GLOBAL]
    }

    /// Stationary standard deviation of the diffusive part of mispricing.
    pub fn mispricing_sd(&self) -> f64 {
        self.sigma_u / (2.0 * self.kappa).sqrt()
    }

    /// Long-run mean of `u`: catalysts push it up, crashes pull it down.
    pub fn mispricing_mean(&self) -> f64 {
        (self.catalyst_rate * self.catalyst_mean - self.crash_rate * self.crash_mean) / self.kappa
    }

    /// Lifecycle term of ln P at company age `age` (years). Closed form:
    /// the Hype hump `A x e^(1-x)` with `x = age / tau` (peaks at `A` when
    /// `age = tau`, then decays) minus the integral of the Legacy decline.
    pub fn life(&self, age: f64) -> f64 {
        let age = age.max(0.0);
        let mut g = 0.0;
        if self.hype_amp > 0.0 {
            let x = age / self.hype_tau;
            g += self.hype_amp * x * (1.0 - x).exp();
        }
        if self.decline_rate > 0.0 {
            let a = (age - self.decline_grace).max(0.0);
            g -= if a < self.decline_ramp {
                self.decline_rate * a * a / (2.0 * self.decline_ramp)
            } else {
                self.decline_rate * (a - 0.5 * self.decline_ramp)
            };
        }
        g
    }

    /// Annualised variance of the firm's own permanent shocks.
    fn fundamental_var(&self, vol_mult: f64) -> f64 {
        let sf = self.sigma_f * vol_mult;
        sf * sf
            + self.jump_rate * (self.jump_sd * self.jump_sd + self.jump_mean * self.jump_mean)
            + self.win_rate * 2.0 * self.win_mean * self.win_mean
    }

    /// Rough annualised volatility of the price for display.
    pub fn annual_vol(&self, factor_vol: f64, vol_mult: f64) -> f64 {
        (self.beta().powi(2) * factor_vol.powi(2) + self.fundamental_var(vol_mult) + (self.sigma_u * vol_mult).powi(2))
            .sqrt()
    }

    /// Regime multiplier as felt by this security.
    fn felt(&self, mult: f64) -> f64 {
        1.0 + (mult - 1.0) * self.regime_sens
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
    /// Company founding time; lifecycle perks count age from here.
    pub born_at: SimTime,

    // --- model state ---
    pub base: f64,
    pub y: f64,
    pub u: f64,
    pub anchors: Factors,
    pub last_update: SimTime,
    /// Fair value (ex `base`) at listing; the default barrier is measured
    /// from here.
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
    pub rescues: u16,
    /// Last few session closes, newest last (momentum signals).
    pub recent_closes: [f32; RECENT_CLOSES],
    pub recent_len: u8,
    /// Chart data. Display only: never read by the simulation.
    pub history: History,
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
    /// Founding time (`<= now`); use `now` for a fresh IPO.
    pub born_at: SimTime,
}

impl Security {
    pub fn list(id: SecurityId, spec: ListingSpec, factors: &Factors, now: SimTime, rng: &mut Rng) -> Self {
        let born_at = spec.born_at.min(now);
        let params = Params::draw(spec.kind, spec.persona, spec.exchange, born_at, rng);
        let u = 0.5 * params.mispricing_sd() * rng.normal();
        let systematic: f64 = params.exposures.iter().zip(factors).map(|(e, f)| e * f).sum();
        let life = params.life(years(now - born_at));
        let log_price = spec.ipo_price.ln();
        // Pre-aged losers start closer to default, but never already past it.
        let y = spec.initial_y.max(-0.6 * params.barrier);
        let base = log_price - systematic - life - y - u;
        let price = spec.ipo_price;
        let mut sec = Self {
            id,
            ticker: spec.ticker,
            kind: spec.kind,
            exchange: spec.exchange,
            persona: spec.persona,
            params,
            sponsor: spec.sponsor,
            listed_at: now,
            born_at,
            base,
            y,
            u,
            anchors: *factors,
            last_update: now,
            // Measure the barrier from the *un-aged* fundamental so pre-aged
            // losers at world start are already closer to default.
            fair_at_listing: systematic + life,
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
            rescues: 0,
            recent_closes: [0.0; RECENT_CLOSES],
            recent_len: 0,
            history: History::new(HistoryKind::Price),
        };
        sec.push_close();
        sec.history.record(now, price);
        sec
    }

    pub fn market_cap(&self) -> f64 {
        self.price * self.shares
    }

    pub fn systematic(&self) -> f64 {
        self.params.exposures.iter().zip(&self.anchors).map(|(e, f)| e * f).sum()
    }

    /// Company age in years at the last update.
    pub fn age_years(&self) -> f64 {
        years(self.last_update - self.born_at).max(0.0)
    }

    fn life_now(&self) -> f64 {
        self.params.life(self.age_years())
    }

    pub fn log_fair_value(&self) -> f64 {
        self.base + self.systematic() + self.life_now() + self.y
    }

    pub fn fair_value(&self) -> f64 {
        self.log_fair_value().exp()
    }

    /// Distance (log units) above the default barrier.
    pub fn distance_to_default(&self) -> f64 {
        (self.systematic() + self.life_now() + self.y - self.fair_at_listing) + self.params.barrier
    }

    /// Typical dollar volume per day, for the square-root impact law.
    pub fn daily_dollar_volume(&self) -> f64 {
        (self.market_cap() * self.params.turnover).max(1_000.0)
    }

    /// Exact catch-up from `last_update` to `now`.
    /// `vol_mult`/`jump_mult` come from the regime in force now (a standard
    /// piecewise-constant approximation; regimes last weeks, gaps hours).
    pub fn advance(&mut self, now: SimTime, factors: &Factors, vol_mult: f64, jump_mult: f64, rng: &mut Rng) {
        let prev = self.last_update;
        let dt = years(now - prev).max(0.0);
        self.last_update = now.max(prev);
        self.anchors = *factors;
        if dt > 0.0 {
            let p = &self.params;
            let (vm, jm) = (p.felt(vol_mult), p.felt(jump_mult));
            // Fundamental: Brownian motion with drift (Ito-corrected). A
            // Novel stock is more volatile before its verdict; the variance
            // of the gap is the sum of the two parts, so this stays exact.
            let sf = p.sigma_f * vm;
            let pre = years((p.verdict_at.min(now) - prev).max(0.0)).min(dt);
            let var = sf * sf * (p.pre_verdict_vol * p.pre_verdict_vol * pre + (dt - pre));
            self.y += p.alpha * dt - 0.5 * var + var.sqrt() * rng.normal();
            // Earnings jumps: the count in the gap is Poisson and the sum of
            // that many normal jumps is itself normal.
            let n = rng.poisson(p.jump_rate * jm * dt);
            if n > 0 {
                let nf = n as f64;
                self.y += p.jump_mean * nf + p.jump_sd * nf.sqrt() * rng.normal();
            }
            // Builder: contract wins, each a permanent step up.
            if p.win_rate > 0.0 {
                for _ in 0..rng.poisson(p.win_rate * dt).min(MAX_SHOCKS_PER_GAP) {
                    self.y += rng.exponential(1.0 / p.win_mean);
                }
            }
            // Novel: the verdict lands exactly once, when its date is crossed.
            if prev < p.verdict_at && p.verdict_at <= now {
                self.y += p.verdict_size;
            }
            // Mispricing: exact OU transition toward zero ...
            let e = (-p.kappa * dt).exp();
            let sd = p.sigma_u * vm * ((1.0 - e * e) / (2.0 * p.kappa)).sqrt();
            let mut u = self.u * e + sd * rng.normal();
            // ... plus sentiment shocks. A shock that arrived a fraction `f`
            // of the way back in the gap has already decayed by
            // e^(-kappa*dt*f); arrival times of a Poisson process are
            // uniform within the gap, so this is exact as well.
            if p.catalyst_rate > 0.0 {
                for _ in 0..rng.poisson(p.catalyst_rate * dt).min(MAX_SHOCKS_PER_GAP) {
                    u += rng.exponential(1.0 / p.catalyst_mean) * (-p.kappa * dt * rng.uniform()).exp();
                }
            }
            if p.crash_rate > 0.0 {
                for _ in 0..rng.poisson(p.crash_rate * jm * dt).min(MAX_SHOCKS_PER_GAP) {
                    u -= rng.exponential(1.0 / p.crash_mean) * (-p.kappa * dt * rng.uniform()).exp();
                }
            }
            self.u = u.clamp(-MAX_MISPRICING, MAX_MISPRICING);
        }
        self.reprice();
    }

    /// Recompute price from state; repairs (and reports) non-finite values.
    /// Returns `false` if a repair was needed.
    pub fn reprice(&mut self) -> bool {
        let lp = self.base + self.systematic() + self.life_now() + self.y + self.u;
        if lp.is_finite() {
            // e^±30 keeps prices in a printable range even in absurd runs.
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
            self.base = self.log_price - self.systematic() - self.life_now() - self.y;
            self.price = self.prev_close.max(0.01);
            false
        }
    }

    /// Record a print: day stats and chart history.
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
        self.history.record(self.last_update, p);
    }

    fn push_close(&mut self) {
        let n = self.recent_len as usize;
        if n == RECENT_CLOSES {
            self.recent_closes.copy_within(1.., 0);
            self.recent_closes[RECENT_CLOSES - 1] = self.price as f32;
        } else {
            self.recent_closes[n] = self.price as f32;
            self.recent_len += 1;
        }
    }

    /// End-of-session bookkeeping. Returns `Some(exit)` if it must delist.
    pub fn close_session(&mut self) -> Option<Exit> {
        self.push_close();
        self.history.record(self.last_update, self.price);
        self.prev_close = self.price;
        self.volume_today = 0.0;
        self.ticks_today = 0;
        if self.price < PENNY_PRICE {
            self.penny_days = self.penny_days.saturating_add(1);
            if self.penny_days >= PENNY_DAYS_TO_DELIST && !self.params.rescuable {
                return Some(Exit::PennyDelisted);
            }
        } else {
            self.penny_days = 0;
        }
        None
    }

    /// Momentum signal: log return versus the close `days` sessions ago.
    pub fn momentum(&self, days: usize) -> f64 {
        let n = self.recent_len as usize;
        if n <= days {
            return 0.0;
        }
        let then = self.recent_closes[n - 1 - days] as f64;
        if then > 0.0 { (self.price / then).ln() } else { 0.0 }
    }

    /// Re-denominate: `ratio` new shares per old share (10 = forward split,
    /// 0.1 = reverse split). Market cap and chart shape are unchanged.
    pub fn apply_split(&mut self, ratio: f64) {
        self.base -= ratio.ln();
        self.shares *= ratio;
        self.reprice();
        self.prev_close /= ratio;
        self.day_open /= ratio;
        self.day_high /= ratio;
        self.day_low /= ratio;
        for c in self.recent_closes.iter_mut() {
            *c /= ratio as f32;
        }
        self.history.scale(1.0 / ratio);
    }

    /// Forward split when the price gets unwieldy; returns the ratio so
    /// holders can be adjusted.
    pub fn maybe_split(&mut self) -> Option<f64> {
        if self.price < SPLIT_TRIGGER {
            return None;
        }
        self.apply_split(SPLIT_RATIO);
        Some(SPLIT_RATIO)
    }

    /// Key stocks are never penny-delisted; they reverse-split instead.
    pub fn maybe_reverse_split(&mut self) -> Option<f64> {
        if !(self.params.rescuable && self.penny_days >= PENNY_DAYS_TO_DELIST && self.kind == SecurityKind::Stock) {
            return None;
        }
        self.penny_days = 0;
        self.apply_split(1.0 / SPLIT_RATIO);
        Some(1.0 / SPLIT_RATIO)
    }

    /// Key perk: instead of going bankrupt the company is recapitalised.
    /// Holders are diluted 20 %, the default barrier is measured afresh
    /// from today's (lower) fair value, and the company clock restarts
    /// ("survives through adaptation, mergers, or rescue").
    pub fn rescue(&mut self) {
        const DILUTION: f64 = 1.25;
        let life_old = self.life_now();
        self.params.hype_amp = 0.0; // a mania does not repeat
        self.born_at = self.last_update;
        // Keep the price continuous across the lifecycle reset.
        self.base += life_old - self.life_now();
        self.y -= DILUTION.ln();
        self.shares *= DILUTION;
        self.fair_at_listing = self.systematic() + self.life_now() + self.y;
        self.rescues = self.rescues.saturating_add(1);
        self.reprice();
    }

    /// Analytic forecast of ln P at horizon `h` years: (mean, std dev).
    /// `factor_drift`/`factor_vol` describe the market factor over `h`.
    /// The sign of a pending Novel verdict is not public, so it widens the
    /// cone instead of shifting the mean.
    pub fn forecast_log(&self, h: f64, factor_drift: f64, factor_vol: f64, vol_mult: f64) -> (f64, f64) {
        let p = &self.params;
        let beta = p.beta();
        let vm = p.felt(vol_mult);
        let sf = p.sigma_f * vm;
        let e = (-p.kappa * h).exp();
        let age = self.age_years();
        let drift = p.alpha - 0.5 * sf * sf + p.jump_rate * p.jump_mean + p.win_rate * p.win_mean;
        let mean = self.log_price + beta * factor_drift + drift * h + (p.life(age + h) - p.life(age))
            - (self.u - p.mispricing_mean()) * (1.0 - e);
        let su = p.sigma_u * vm;
        let shock_var = p.catalyst_rate * 2.0 * p.catalyst_mean.powi(2) + p.crash_rate * 2.0 * p.crash_mean.powi(2);
        let verdict_in = years(p.verdict_at - self.last_update);
        let verdict_var = if verdict_in > 0.0 && verdict_in <= h { p.verdict_size * p.verdict_size } else { 0.0 };
        let var = beta * beta * factor_vol * factor_vol * h
            + p.fundamental_var(vm) * h
            + verdict_var
            + (su * su + shock_var) * (1.0 - e * e) / (2.0 * p.kappa);
        (mean, var.max(0.0).sqrt())
    }
}

// ------------------------------------------------------------- persistence

mod persist {
    use super::*;
    use crate::save::{Persist, Reader, Result, SaveError, Writer};

    impl Persist for SecurityId {
        fn put(&self, w: &mut Writer) {
            self.0.put(w);
        }
        fn get(r: &mut Reader) -> Result<Self> {
            Ok(SecurityId(u32::get(r)?))
        }
    }

    impl Persist for Ticker {
        fn put(&self, w: &mut Writer) {
            self.bytes.put(w);
            self.len.put(w);
        }
        fn get(r: &mut Reader) -> Result<Self> {
            let (bytes, len) = (<[u8; 5]>::get(r)?, u8::get(r)?);
            if len as usize > bytes.len() || !bytes[..len as usize].iter().all(u8::is_ascii_uppercase) {
                return Err(SaveError::Corrupt("ticker"));
            }
            Ok(Ticker { bytes, len })
        }
    }

    crate::tag_enum!(SecurityKind { Stock = 0, Fund = 1 });

    impl Persist for Sponsor {
        fn put(&self, w: &mut Writer) {
            match self {
                Sponsor::Engine => 0u8.put(w),
                Sponsor::Player => 1u8.put(w),
                Sponsor::Firm(id) => {
                    2u8.put(w);
                    id.put(w);
                }
            }
        }
        fn get(r: &mut Reader) -> Result<Self> {
            match u8::get(r)? {
                0 => Ok(Sponsor::Engine),
                1 => Ok(Sponsor::Player),
                2 => Ok(Sponsor::Firm(Persist::get(r)?)),
                _ => Err(SaveError::Corrupt("sponsor")),
            }
        }
    }

    crate::record!(Params {
        exposures,
        alpha,
        sigma_f,
        kappa,
        sigma_u,
        jump_rate,
        jump_mean,
        jump_sd,
        win_rate,
        win_mean,
        catalyst_rate,
        catalyst_mean,
        crash_rate,
        crash_mean,
        regime_sens,
        hype_amp,
        hype_tau,
        decline_rate,
        decline_grace,
        decline_ramp,
        verdict_at,
        verdict_size,
        pre_verdict_vol,
        tick_rate,
        impact,
        barrier,
        rescuable,
        turnover,
    });

    // `history` is display data and lives in the sidecar file, so a loaded
    // security starts with an empty chart that the sidecar then fills.
    crate::record!(Security {
        id,
        ticker,
        kind,
        exchange,
        persona,
        params,
        sponsor,
        listed_at,
        born_at,
        base,
        y,
        u,
        anchors,
        last_update,
        fair_at_listing,
        log_price,
        price,
        shares,
        prev_close,
        day_open,
        day_high,
        day_low,
        volume_today,
        ticks_today,
        penny_days,
        rescues,
        recent_closes,
        recent_len,
    } skip { history: History::new(HistoryKind::Price) });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persona::{ALL_PERKS, Persona};

    const NO_FACTORS: Factors = [0.0; FACTOR_COUNT];

    fn make_with(persona: Persona, rng: &mut Rng) -> Security {
        Security::list(
            SecurityId::new(1, 0),
            ListingSpec {
                kind: SecurityKind::Stock,
                exchange: 0,
                persona,
                ticker: Ticker::new("ABC"),
                ipo_price: 50.0,
                market_cap: 1e9,
                sponsor: Sponsor::Engine,
                initial_y: 0.0,
                born_at: 0.0,
            },
            &NO_FACTORS,
            0.0,
            rng,
        )
    }

    fn make(rng: &mut Rng) -> Security {
        make_with(Persona::EMPTY, rng)
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
        for letters in ["", "H", "L", "HN", "ABCEFGHIJKNOP"] {
            let s = make_with(Persona::parse(letters), &mut rng);
            assert!((s.price - 50.0).abs() < 1e-9, "{letters}");
            assert!((s.distance_to_default() - s.params.barrier).abs() < 1e-9, "{letters}");
        }
    }

    #[test]
    fn factor_moves_pass_through_with_beta() {
        let mut rng = Rng::seed_from_u64(2);
        let mut s = make(&mut rng);
        let before = s.log_price;
        // Zero elapsed time: only the factor change can move the price.
        let mut f = NO_FACTORS;
        f[F_MARKET] = 0.1;
        s.advance(0.0, &f, 1.0, 1.0, &mut rng);
        assert!((s.log_price - before - 0.1 * s.params.beta()).abs() < 1e-12);
    }

    /// The core claim of path A: the distribution of a one-day move does not
    /// depend on how many updates happen inside the day. Checked for a plain
    /// stock and for one carrying every shock mechanism at once.
    #[test]
    fn variance_is_independent_of_update_frequency() {
        let day = 86_400.0;
        for (letters, tolerance) in [("", 0.12), ("BIO", 0.2)] {
            let var_of = |steps: usize, seed: u64| {
                let mut rng = Rng::seed_from_u64(seed);
                let trials = 6_000;
                let (mut s1, mut s2) = (0.0, 0.0);
                for _ in 0..trials {
                    let mut s = make_with(Persona::parse(letters), &mut rng);
                    s.u = 0.0;
                    s.reprice();
                    let p0 = s.log_price;
                    for k in 1..=steps {
                        s.advance(day * k as f64 / steps as f64, &NO_FACTORS, 1.0, 1.0, &mut rng);
                    }
                    let r = s.log_price - p0;
                    s1 += r;
                    s2 += r * r;
                }
                s2 / trials as f64 - (s1 / trials as f64).powi(2)
            };
            let ratio = var_of(1, 10) / var_of(200, 11);
            assert!((1.0 - tolerance..1.0 + tolerance).contains(&ratio), "{letters:?}: ratio {ratio}");
        }
    }

    #[test]
    fn mispricing_reverts() {
        let mut rng = Rng::seed_from_u64(3);
        let mut s = make(&mut rng);
        s.params.sigma_u = 0.0;
        s.u = 0.5;
        // Five half-lives later, ~3% of the shock should remain.
        let half_life_secs = (2f64.ln() / s.params.kappa) * crate::time::SECS_PER_YEAR;
        s.advance(5.0 * half_life_secs, &NO_FACTORS, 1.0, 1.0, &mut rng);
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

    #[test]
    fn lifecycle_shapes() {
        let mut rng = Rng::seed_from_u64(5);
        let hype = make_with(Persona::parse("H"), &mut rng).params;
        // Peaks at exactly `amp` when age = tau, then unwinds toward zero.
        assert!((hype.life(hype.hype_tau) - hype.hype_amp).abs() < 1e-12);
        assert!(hype.life(0.5 * hype.hype_tau) < hype.hype_amp);
        assert!(hype.life(8.0 * hype.hype_tau) < 0.05 * hype.hype_amp);
        let legacy = make_with(Persona::parse("L"), &mut rng).params;
        // Flat through the grace period, then smoothly downhill.
        assert_eq!(legacy.life(legacy.decline_grace), 0.0);
        let (a, b) = (legacy.decline_grace + legacy.decline_ramp, legacy.decline_grace + legacy.decline_ramp + 1.0);
        assert!((legacy.life(a) - legacy.life(b) - legacy.decline_rate).abs() < 1e-12);
    }

    #[test]
    fn novel_verdict_lands_exactly_once() {
        let mut rng = Rng::seed_from_u64(6);
        let mut s = make_with(Persona::parse("N"), &mut rng);
        // Silence everything else so the verdict is the only move in `y`.
        s.params.sigma_f = 0.0;
        s.params.alpha = 0.0;
        s.params.jump_rate = 0.0;
        let (at, size) = (s.params.verdict_at, s.params.verdict_size);
        assert!(at.is_finite() && size.abs() >= 0.45);
        s.advance(at - 1.0, &NO_FACTORS, 1.0, 1.0, &mut rng);
        assert_eq!(s.y, 0.0);
        s.advance(at + 1.0, &NO_FACTORS, 1.0, 1.0, &mut rng);
        assert!((s.y - size).abs() < 1e-12);
        s.advance(at + 1e7, &NO_FACTORS, 1.0, 1.0, &mut rng);
        assert!((s.y - size).abs() < 1e-12);
    }

    #[test]
    fn rescue_restores_the_barrier_without_a_price_gap_beyond_dilution() {
        let mut rng = Rng::seed_from_u64(7);
        let mut s = make_with(Persona::parse("HK"), &mut rng);
        s.advance(0.4 * SECS_PER_YEAR, &NO_FACTORS, 1.0, 1.0, &mut rng);
        s.y -= 5.0;
        s.reprice();
        assert!(s.distance_to_default() < 0.0);
        let (before, cap) = (s.log_price, s.market_cap());
        s.rescue();
        assert!((s.distance_to_default() - s.params.barrier).abs() < 1e-9);
        assert!((before - s.log_price - 1.25f64.ln()).abs() < 1e-9);
        assert!((s.market_cap() / cap - 1.0).abs() < 1e-9);
    }

    // ------------------------------------------------------ stacking audit

    /// Every one of the 2^16 perk sets (including sets the game never draws:
    /// contradicting pairs and more than 12 perks) must produce parameters
    /// inside the range where the model is known to behave.
    #[test]
    fn all_65536_perk_sets_stay_in_safe_bounds() {
        let mut rng = Rng::seed_from_u64(8);
        for mask in 0..=u16::MAX {
            for exchange in [0u8, 3] {
                let p = Params::draw(SecurityKind::Stock, Persona(mask), exchange, 0.0, &mut rng);
                let who = Persona(mask).letters();
                // Never flat: the quietest stock still moves ~8 %/yr.
                let vol = p.annual_vol(0.14, 1.0);
                assert!((0.075..1.6).contains(&vol), "{who}: vol {vol}");
                assert!(p.sigma_f >= 0.05 && p.sigma_u >= 0.10 && p.beta() >= 0.15, "{who}");
                // Mean reversion is first order and neither stuck nor instant
                // (half-life between one week and about four months).
                assert!((2.0..=36.0).contains(&p.kappa), "{who}: kappa {}", p.kappa);
                assert!((0.01..0.5).contains(&p.mispricing_sd()), "{who}");
                // No runaway trend in either direction.
                let drift = p.alpha + p.jump_rate * p.jump_mean + p.win_rate * p.win_mean;
                assert!((-0.75..0.35).contains(&drift), "{who}: drift {drift}");
                assert!(p.decline_rate <= 0.45 * p.sigma_f.max(0.09) + 1e-12, "{who}");
                // Shocks are sized against the stock's own noise.
                assert!(p.win_mean <= 0.55 * p.sigma_f + 1e-12, "{who}");
                assert!(p.catalyst_mean <= 0.9 * p.mispricing_sd() + 1e-12, "{who}");
                assert!(p.crash_mean <= 0.40 + 1e-12, "{who}");
                assert!(p.crash_mean == 0.0 || p.crash_mean >= 0.12, "{who}: crash too small to matter");
                let bias = p.mispricing_mean();
                assert!((-MAX_CRASH_BIAS - 1e-9..=MAX_CATALYST_BIAS + 1e-9).contains(&bias), "{who}: u mean {bias}");
                assert!((0.02..=2.0).contains(&p.tick_rate) && (0.5..=4.0).contains(&p.barrier), "{who}");
                assert!(p.exposures.iter().all(|x| x.is_finite()), "{who}");
            }
        }
    }

    /// Simulated daily closes for one stock over `days`, market held still.
    fn daily_returns(persona: Persona, days: usize, rng: &mut Rng) -> Vec<f64> {
        let mut s = make_with(persona, rng);
        let mut last = s.log_price;
        (1..=days)
            .map(|d| {
                s.advance(d as f64 * 86_400.0, &NO_FACTORS, 1.0, 1.0, rng);
                let r = s.log_price - last;
                last = s.log_price;
                r
            })
            .collect()
    }

    /// Behavioural check on simulated paths: singles, every pair, and a few
    /// hundred heavy stacks must all look like a stock, not like a flat
    /// line, a sine wave or a saw-tooth.
    #[test]
    fn stacked_personas_do_not_flatline_oscillate_or_sawtooth() {
        let mut rng = Rng::seed_from_u64(9);
        let mut personas: Vec<Persona> = vec![Persona::EMPTY, Persona(u16::MAX).sanitized()];
        for (i, &a) in ALL_PERKS.iter().enumerate() {
            personas.push(Persona::EMPTY.with(a));
            for &b in &ALL_PERKS[i + 1..] {
                personas.push(Persona::EMPTY.with(a).with(b).sanitized());
            }
        }
        while personas.len() < 500 {
            let p = Persona::random(&mut rng);
            if p.len() >= 7 {
                personas.push(p);
            }
        }
        let days = 1_095;
        for persona in personas {
            let who = persona.letters();
            let raw = daily_returns(persona, days, &mut rng);
            assert!(raw.iter().all(|x| x.is_finite() && x.abs() > 0.0), "{who}: a day with no move");
            // Rare one-off events (a Novel verdict, an Opportunity crash)
            // are meant to be large; two of them landing k days apart would
            // fake a "period" of k in a plain correlation. Clip returns at
            // 5 robust standard deviations so the checks below look at the
            // everyday behaviour, where a periodic pattern would live.
            let mut sorted: Vec<f64> = raw.iter().map(|x| x.abs()).collect();
            sorted.sort_by(|a, b| a.total_cmp(b));
            let clip = 5.0 * 1.4826 * sorted[sorted.len() / 2];
            let r: Vec<f64> = raw.iter().map(|x| x.clamp(-clip, clip)).collect();
            let n = r.len() as f64;
            let mean = r.iter().sum::<f64>() / n;
            let var = r.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
            let sd = var.sqrt();
            // Flatline: a real daily volatility and no "nothing happened" days.
            assert!((0.0025..0.2).contains(&sd), "{who}: daily sd {sd}");
            // Sine / saw-tooth: both are periodic, which shows up as large
            // autocorrelation of returns at some lag. Pure noise at this
            // sample size gives |rho| < ~0.1.
            for lag in 1..=60 {
                let cov = r.windows(lag + 1).map(|w| (w[0] - mean) * (w[lag] - mean)).sum::<f64>() / n;
                let rho = cov / var;
                assert!(rho.abs() < 0.25, "{who}: autocorrelation {rho:.2} at lag {lag}");
            }
            // Ruler-line trend: a deterministic slope dominating the noise
            // would give very long runs of same-signed days.
            let mut longest = 0;
            let mut run = 0;
            let mut prev_sign = 0.0;
            for x in &r {
                let sign = x.signum();
                run = if sign == prev_sign { run + 1 } else { 1 };
                prev_sign = sign;
                longest = longest.max(run);
            }
            assert!(longest < 40, "{who}: {longest} same-signed days in a row");
        }
    }
}
