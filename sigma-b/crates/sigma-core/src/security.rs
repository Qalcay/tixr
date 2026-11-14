//! A single security: its persona, its lazily advanced state, and the
//! valuation that turns fundamentals into a price.
//!
//! ## Lazy catch-up
//! A security is only touched when the sampler picks it (or the user looks
//! at it). It remembers the calendar time `t_last` and variance time
//! `v_last` of its last update. `advance` then moves its private state over
//! the whole gap in **one exact step**:
//!
//! * mispricing `m` and volatility state `h` are OU processes: exact step,
//! * earnings `e` are Brownian with drift: exact step,
//! * news jumps are Poisson: the number of jumps in the gap is drawn once,
//! * excess growth `g` is OU; its integral uses the trapezoid rule.
//!
//! So a stock updated once an hour has (to within the trapezoid term) the
//! *same distribution* as one updated every second. Update frequency only
//! changes how finely the price path is drawn, never where it goes. That is
//! why path B can afford to update only a random fraction each tick.
//!
//! ## Pricing
//! ```text
//! E   = exp(e + pass_through*ln CPI + ln realGDP + (cyclicality-1)*gap)
//! V   = E * payout_eff * (1+g_lt) / (r_e - g_lt)            (Gordon growth)
//! r_e = y10 + beta*ERP + premium + leverage*credit_spread/2  (CAPM + credit)
//! P   = V * exp(beta*S_mkt + gamma*S_sector + m)
//! ```
//! Interest rates reach every price through `r_e`: when the 10-year yield
//! rises, every multiple compresses at once, as in real markets.

use crate::clock::SECONDS_PER_YEAR;
use crate::macro_econ::{norm_cdf, ou_step};
use crate::market::{SECTORS, Sector, exchange_for_cap};
use crate::persona::{Modifiers, PerkSet};
use crate::rng::Rng;
use std::collections::VecDeque;

/// Daily closes kept per security (about six months of trading days).
pub const HISTORY_DAYS: usize = 128;

/// Generation-tagged id: low 16 bits = slot, high 16 bits = generation.
/// When a slot is reused the generation increments, so a stale id held by a
/// portfolio can never point at the new company in the same slot. (In tixr
/// alpha holdings were keyed by slot, so a respawned ticker silently
/// inherited old positions.)
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

/// Up to five ASCII letters, stored inline (no heap allocation per row).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Ticker {
    pub bytes: [u8; 5],
    pub len: u8,
}

impl Ticker {
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("?")
    }
}

/// What kind of issuer stands behind the security.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Issuer {
    /// An operating company valued on earnings.
    Company,
    /// A listed brokerage (broker index); valued on its book.
    BrokerFirm(u16),
    /// Launched by the player.
    PlayerFounded,
}

/// The shared market state a security reads when it prices itself.
/// Small and `Copy`: the engine keeps one for "now" and one per recent
/// daily close.
#[derive(Clone, Copy, Debug, Default)]
pub struct Globals {
    pub t: f64,
    pub v: f64,
    pub log_cpi: f64,
    pub log_gdp: f64,
    pub gap: f64,
    pub expected_inflation: f64,
    pub potential_growth: f64,
    pub y10: f64,
    pub erp: f64,
    pub credit_spread: f64,
    pub vol_mult: f64,
    pub s_market: f64,
    pub s_sector: [f64; 11],
    /// Population pressure: extra exit hazard when the market is crowded.
    pub crowding_hazard: f64,
}

/// Extra inputs for a listed broker: its book value and recent return.
#[derive(Clone, Copy, Debug)]
pub struct FirmLink {
    pub equity: f64,
    pub roe: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Exit {
    Defaulted,
    Acquired { price: f64 },
    Delisted { price: f64 },
}

#[derive(Clone, Debug)]
pub struct Security {
    pub id: SecurityId,
    pub ticker: Ticker,
    pub sector: Sector,
    pub exchange: u8,
    pub perks: PerkSet,
    pub issuer: Issuer,

    // --- Persona-derived parameters, fixed at listing ---
    pub beta: f64,
    pub sector_beta: f64,
    pub cyclicality: f64,
    pub pass_through: f64,
    pub fund_vol: f64,
    pub sent_vol: f64,
    pub reversion: f64,
    pub growth_mean: f64,
    pub growth_fade: f64,
    pub payout: f64,
    pub leverage: f64,
    pub debt_per_share: f64,
    pub jump_rate: f64,
    pub jump_size: f64,
    pub jump_mean: f64,
    pub liquidity: f64,
    pub premium: f64,
    pub takeover_rate: f64,
    pub default_mult: f64,
    pub attention: f64,
    pub shares: f64,

    // --- Lazily advanced state ---
    /// Idiosyncratic log earnings per share.
    pub e: f64,
    /// Current excess growth over the economy (per year).
    pub g: f64,
    /// Log volatility state (clustering).
    pub h: f64,
    /// Mispricing: log distance between price and model value.
    pub m: f64,
    pub t_last: f64,
    pub v_last: f64,

    // --- Observables ---
    pub price: f64,
    pub fair: f64,
    pub prev_close: f64,
    pub day_high: f64,
    pub day_low: f64,
    /// Cumulative dividends paid per *base* share (see `split_factor`).
    pub cum_div_base: f64,
    /// Product of all split ratios. Holders store base shares, so a split
    /// never needs to touch their positions: current shares = base * factor.
    pub split_factor: f64,
    pub closes: VecDeque<f32>,
    pub last_close_t: f64,
    pub low_price_days: u16,
    pub listed_t: f64,
    pub halted_until: f64,
    pub updates: u32,
}

/// Parameters for a new listing.
pub struct ListingSpec {
    pub sector: Sector,
    pub perks: PerkSet,
    pub issuer: Issuer,
    pub price: f64,
    pub market_cap: f64,
}

impl ListingSpec {
    /// A random listing as the engine or a broker would underwrite it.
    pub fn random(rng: &mut Rng) -> Self {
        let sector = Sector::draw(rng);
        let perks = PerkSet::draw(rng, sector.affinity());
        Self {
            sector,
            perks,
            issuer: Issuer::Company,
            price: rng.lognormal(22.0, 0.7).clamp(3.0, 300.0),
            market_cap: rng.lognormal(1.2e9, 1.3).clamp(5.0e7, 2.0e11),
        }
    }
}

impl Security {
    pub fn list(
        id: SecurityId,
        ticker: Ticker,
        spec: ListingSpec,
        g: &Globals,
        link: Option<FirmLink>,
        rng: &mut Rng,
    ) -> Self {
        let prof = spec.sector.profile();
        let mods: Modifiers = spec.perks.modifiers();
        let jitter = |rng: &mut Rng, s: f64| (s * rng.normal()).exp();
        let leverage = (prof.leverage + mods.leverage + 0.08 * rng.normal()).clamp(0.0, 0.85);
        let price = spec.price.max(0.5);
        let shares = (spec.market_cap / price).max(1.0e5);
        let mut s = Self {
            id,
            ticker,
            sector: spec.sector,
            exchange: 0,
            perks: spec.perks,
            issuer: spec.issuer,
            beta: (prof.beta * mods.beta * jitter(rng, 0.15)).clamp(0.1, 3.0),
            sector_beta: (mods.sector_beta * jitter(rng, 0.2)).clamp(0.1, 3.0),
            cyclicality: (prof.cyclicality * mods.cyclicality * jitter(rng, 0.2)).clamp(0.1, 6.0),
            pass_through: (0.7 + mods.pass_through).clamp(0.3, 1.0),
            fund_vol: (prof.fund_vol * mods.fund_vol * jitter(rng, 0.2)).clamp(0.04, 0.9),
            sent_vol: (0.28 * mods.sent_vol * jitter(rng, 0.2)).clamp(0.05, 1.0),
            reversion: (6.0 * mods.reversion * jitter(rng, 0.2)).clamp(1.0, 25.0),
            growth_mean: (prof.excess_growth + mods.growth + 0.02 * rng.normal()).clamp(-0.08, 0.15),
            growth_fade: (0.25 * mods.growth_fade).clamp(0.05, 1.5),
            payout: (prof.payout + mods.payout + 0.08 * rng.normal()).clamp(0.0, 0.95),
            leverage,
            debt_per_share: price * leverage / (1.0 - leverage),
            jump_rate: (3.0 * mods.jump_rate).clamp(0.5, 20.0),
            jump_size: (0.06 * mods.jump_size).clamp(0.01, 0.3),
            jump_mean: -0.005 + mods.jump_mean,
            liquidity: mods.liquidity.clamp(0.1, 5.0),
            premium: mods.premium,
            takeover_rate: (0.035 * mods.takeover).clamp(0.002, 0.2),
            default_mult: mods.default_risk,
            attention: mods.attention,
            shares,
            e: 0.0,
            g: 0.0,
            h: 0.0,
            m: 0.04 + 0.05 * rng.normal(), // a small IPO "pop" that fades
            t_last: g.t,
            v_last: g.v,
            price,
            fair: price,
            prev_close: price,
            day_high: price,
            day_low: price,
            cum_div_base: 0.0,
            split_factor: 1.0,
            closes: VecDeque::with_capacity(HISTORY_DAYS),
            last_close_t: g.t,
            low_price_days: 0,
            listed_t: g.t,
            halted_until: 0.0,
            updates: 0,
        };
        s.g = s.growth_mean;
        // Solve for the earnings level that makes today's model price equal
        // the offer price: price with e = 0, then shift.
        s.e = 0.0;
        let p0 = s.model_log_price(g, None);
        s.e = price.ln() - p0;
        if link.is_some() {
            // A brokerage is priced off its book: choose the share count
            // that makes the offer price consistent with that book.
            let lf = s.log_fair(g, link) + s.shares.ln();
            let lp = lf + s.beta * g.s_market + s.sector_beta * g.s_sector[s.sector as usize] + s.m;
            s.shares = (lp - price.ln()).exp().max(1.0e5);
        }
        s.reprice(g, link);
        s.prev_close = s.price;
        s.exchange = exchange_for_cap(s.market_cap());
        s
    }

    pub fn market_cap(&self) -> f64 {
        self.price * self.shares
    }

    /// Total equity volatility (annual), used for display, risk and weights.
    pub fn equity_vol(&self, vol_mult: f64) -> f64 {
        let sent = self.sent_vol * self.h.exp() * vol_mult;
        let jumps = self.jump_rate * self.jump_size * self.jump_size;
        (self.fund_vol * self.fund_vol + sent * sent + jumps).sqrt()
    }

    /// Cost of equity: CAPM plus persona premium plus a credit component.
    pub fn cost_of_equity(&self, g: &Globals) -> f64 {
        (g.y10 + self.beta * g.erp + self.premium + 0.5 * self.leverage * g.credit_spread)
            .clamp(0.03, 0.30)
    }

    /// Long-run nominal growth the market capitalises.
    fn long_growth(&self, g: &Globals, r_e: f64) -> f64 {
        (g.potential_growth + self.pass_through * g.expected_inflation + 0.3 * self.g)
            .clamp(-0.02, r_e - 0.025)
    }

    /// Price-to-earnings multiple from the Gordon growth model. Bounded so
    /// a quirk of rates can never produce an infinite valuation.
    pub fn multiple(&self, g: &Globals) -> f64 {
        let r_e = self.cost_of_equity(g);
        let gl = self.long_growth(g, r_e);
        let payout_eff = 0.4 + 0.6 * self.payout;
        (payout_eff * (1.0 + gl) / (r_e - gl)).clamp(3.0, 45.0)
    }

    pub fn log_earnings(&self, g: &Globals) -> f64 {
        self.e + self.pass_through * g.log_cpi + g.log_gdp + (self.cyclicality - 1.0) * g.gap
    }

    /// Earnings per share (annual).
    pub fn eps(&self, g: &Globals) -> f64 {
        self.log_earnings(g).exp()
    }

    fn log_fair(&self, g: &Globals, link: Option<FirmLink>) -> f64 {
        match (self.issuer, link) {
            (Issuer::BrokerFirm(_), Some(l)) => {
                // Brokerages trade on book value. P/B rises when the broker
                // earns more than its cost of equity (residual income).
                let r_e = self.cost_of_equity(g);
                let gl = self.long_growth(g, r_e);
                let pb = (1.0 + (l.roe - r_e) / (r_e - gl)).clamp(0.3, 4.0);
                let book = (l.equity / self.shares).max(1e-6);
                (book * pb).ln()
            }
            _ => self.log_earnings(g) + self.multiple(g).ln(),
        }
    }

    fn model_log_price(&self, g: &Globals, link: Option<FirmLink>) -> f64 {
        self.log_fair(g, link)
            + self.beta * g.s_market
            + self.sector_beta * g.s_sector[self.sector as usize]
            + self.m
    }

    /// Recompute price and fair value from state. No randomness.
    pub fn reprice(&mut self, g: &Globals, link: Option<FirmLink>) {
        let lf = self.log_fair(g, link);
        let lp = self.model_log_price(g, link);
        // Final guard: a non-finite value is a bug upstream, so keep the
        // last good price and let the engine's watchdog report it.
        if lf.is_finite() && lp.is_finite() {
            self.fair = lf.exp().clamp(1e-6, 1e9);
            self.price = lp.exp().clamp(1e-6, 1e9);
        }
        self.day_high = self.day_high.max(self.price);
        self.day_low = self.day_low.min(self.price);
    }

    /// Advance state from `t_last` to `g.t`. Returns an exit if the company
    /// fails or is acquired during the gap.
    pub fn advance(&mut self, g: &Globals, link: Option<FirmLink>, rng: &mut Rng) -> Option<Exit> {
        let dt = (g.t - self.t_last) / SECONDS_PER_YEAR; // calendar years
        let dv = (g.v - self.v_last).max(0.0); // variance years
        if dt <= 0.0 {
            return None;
        }
        self.updates = self.updates.wrapping_add(1);

        // Excess growth fades toward its long-run mean (competition).
        let g_old = self.g;
        self.g = ou_step(self.g, self.growth_mean, self.growth_fade, 0.02, dt, rng.normal())
            .clamp(-0.15, 0.25);
        // Earnings: drift on the calendar, diffusion on variance time.
        // The -0.5*sigma^2 (Ito) term makes *expected* earnings grow at g.
        let fv = self.fund_vol;
        self.e += 0.5 * (g_old + self.g) * dt - 0.5 * fv * fv * dv + fv * dv.sqrt() * rng.normal();
        // News: Poisson number of jumps in the gap, summed in one draw.
        let n = rng.poisson(self.jump_rate * dv);
        if n > 0 {
            let nf = n as f64;
            self.e += nf * self.jump_mean + self.jump_size * nf.sqrt() * rng.normal_clipped(4.0);
        }
        // Volatility clustering and mispricing, both exact OU.
        self.h = ou_step(self.h, 0.0, 8.0, 1.0, dv, rng.normal()).clamp(-1.5, 1.5);
        let sent = self.sent_vol * self.h.exp() * g.vol_mult;
        self.m = ou_step(self.m, 0.0, self.reversion, sent, dv, rng.normal()).clamp(-1.5, 1.5);

        // Dividends accrue continuously on calendar time.
        if !matches!(self.issuer, Issuer::BrokerFirm(_)) {
            let dps = self.payout * self.eps(g) * dt;
            if dps.is_finite() && dps > 0.0 {
                self.cum_div_base += dps * self.split_factor;
            }
        }

        self.t_last = g.t;
        self.v_last = g.v;
        self.reprice(g, link);
        self.hazards(g, dt, dv, rng)
    }

    /// Exit hazards over the gap just simulated.
    fn hazards(&mut self, g: &Globals, dt: f64, dv: f64, rng: &mut Rng) -> Option<Exit> {
        // Merton-style distance to default: how many standard deviations
        // asset value sits above debt.
        let default_rate = if self.debt_per_share > 0.0 {
            let assets = self.price + self.debt_per_share;
            let asset_vol = (self.equity_vol(g.vol_mult) * self.price / assets).max(0.02);
            let dd = (assets / self.debt_per_share).ln() / asset_vol;
            (0.008 + 3.0 * norm_cdf(-dd)) * self.default_mult
        } else {
            0.0
        };
        let hazard = default_rate * dv + g.crowding_hazard * dt;
        if rng.chance(1.0 - (-hazard).exp()) {
            return Some(Exit::Defaulted);
        }
        if rng.chance(1.0 - (-self.takeover_rate * dt).exp()) {
            let premium = rng.range(0.15, 0.45);
            return Some(Exit::Acquired { price: self.price * (1.0 + premium) });
        }
        None
    }

    /// Called once per security for each daily close it crosses. Handles
    /// history, the exchange's low-price rule and stock splits.
    pub fn record_close(&mut self, close_t: f64) -> Option<Exit> {
        self.last_close_t = close_t;
        if self.closes.len() == HISTORY_DAYS {
            self.closes.pop_front();
        }
        self.closes.push_back(self.price as f32);
        self.prev_close = self.price;
        self.day_high = self.price;
        self.day_low = self.price;

        // Listing rule (like the NYSE $1 rule): 30 closes under 1.00 = out.
        if self.price < 1.0 {
            self.low_price_days += 1;
            if self.low_price_days >= 30 {
                return Some(Exit::Delisted { price: self.price });
            }
        } else {
            self.low_price_days = 0;
        }

        // Split 10-for-1 when the price passes 1,000. Keeps numbers readable
        // (and the side panel narrow) without changing anyone's wealth.
        if self.price >= 1000.0 {
            self.apply_split(10.0);
        }
        None
    }

    pub fn apply_split(&mut self, ratio: f64) {
        self.split_factor *= ratio;
        self.shares *= ratio;
        self.e -= ratio.ln();
        self.debt_per_share /= ratio;
        self.price /= ratio;
        self.fair /= ratio;
        self.prev_close /= ratio;
        self.day_high /= ratio;
        self.day_low /= ratio;
        for c in self.closes.iter_mut() {
            *c /= ratio as f32;
        }
    }

    /// Sampling weight: calm, ignored stocks are drawn less often.
    pub fn base_weight(&self, vol_mult: f64) -> f64 {
        self.attention * (0.5 + self.equity_vol(vol_mult) / 0.3)
    }

    pub fn change_pct(&self) -> f64 {
        if self.prev_close > 0.0 { (self.price / self.prev_close - 1.0) * 100.0 } else { 0.0 }
    }

    pub fn is_finite(&self) -> bool {
        [self.e, self.g, self.h, self.m, self.price, self.fair, self.cum_div_base]
            .iter()
            .all(|v| v.is_finite())
    }

    /// One-year model forecast: (expected price, low, high) at ~90%.
    pub fn forecast_1y(&self, g: &Globals) -> (f64, f64, f64) {
        let r_e = self.cost_of_equity(g);
        let div_yield = if self.fair > 0.0 { self.payout * self.eps(g) / self.price } else { 0.0 };
        // Expected log return: price appreciation (cost of equity minus the
        // dividend paid out) plus the mispricing expected to close.
        let decay = (-self.reversion).exp();
        let mu = (r_e - div_yield).clamp(-0.2, 0.4) - self.m * (1.0 - decay);
        let sd = self.equity_vol(g.vol_mult);
        let center = self.price * mu.exp();
        (center, center * (-1.645 * sd).exp(), center * (1.645 * sd).exp())
    }
}

/// Number of sectors, re-exported for fixed-size arrays elsewhere.
pub const SECTOR_COUNT: usize = SECTORS.len();

#[cfg(test)]
mod tests {
    use super::*;

    fn globals() -> Globals {
        Globals {
            t: 0.0,
            v: 0.0,
            y10: 0.04,
            erp: 0.045,
            credit_spread: 0.04,
            vol_mult: 1.0,
            potential_growth: 0.02,
            expected_inflation: 0.02,
            ..Default::default()
        }
    }

    fn listing(rng: &mut Rng) -> Security {
        let g = globals();
        let spec = ListingSpec {
            sector: Sector::Industrials,
            perks: PerkSet(0),
            issuer: Issuer::Company,
            price: 50.0,
            market_cap: 1.0e9,
        };
        let t = Ticker { bytes: *b"TEST\0", len: 4 };
        Security::list(SecurityId::new(1, 0), t, spec, &g, None, rng)
    }

    #[test]
    fn listing_prices_at_offer() {
        let mut rng = Rng::seeded(1);
        let s = listing(&mut rng);
        assert!((s.price - 50.0).abs() < 1e-6, "{}", s.price);
        assert!(s.multiple(&globals()) > 3.0);
    }

    #[test]
    fn id_round_trip() {
        let id = SecurityId::new(65_535, 7);
        assert_eq!(id.slot(), 65_535);
        assert_eq!(id.generation(), 7);
    }

    #[test]
    fn update_cadence_does_not_change_distribution() {
        // The core claim of path B: one big catch-up step and many small
        // steps give the same distribution of log returns.
        let horizon_days = 20.0;
        let mut rng = Rng::seeded(77);
        let n = 4000;
        let (mut coarse, mut fine) = (Vec::new(), Vec::new());
        for _ in 0..n {
            for (steps, out) in [(1usize, &mut coarse), (40usize, &mut fine)] {
                let mut s = listing(&mut rng);
                s.takeover_rate = 0.0;
                s.debt_per_share = 0.0;
                let start = s.price.ln();
                let mut g = globals();
                for k in 1..=steps {
                    let frac = k as f64 / steps as f64;
                    g.t = horizon_days * 86_400.0 * frac;
                    g.v = horizon_days / 365.25 * frac;
                    let _ = s.advance(&g, None, &mut rng);
                }
                out.push(s.price.ln() - start);
            }
        }
        let sd = |v: &[f64]| {
            let m = v.iter().sum::<f64>() / v.len() as f64;
            (v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / v.len() as f64).sqrt()
        };
        let (a, b) = (sd(&coarse), sd(&fine));
        assert!((a - b).abs() / b < 0.08, "coarse sd {a} vs fine sd {b}");
    }

    #[test]
    fn split_preserves_value() {
        let mut rng = Rng::seeded(3);
        let mut s = listing(&mut rng);
        let cap = s.market_cap();
        let per_base = s.price * s.split_factor;
        s.apply_split(10.0);
        assert!((s.market_cap() - cap).abs() < 1e-3 * cap);
        assert!((s.price * s.split_factor - per_base).abs() < 1e-9 * per_base);
        // The model price after a split matches the split-adjusted price.
        let g = globals();
        let before = s.price;
        s.reprice(&g, None);
        assert!((s.price - before).abs() < 1e-6 * before);
    }

    #[test]
    fn long_run_state_is_bounded() {
        let mut rng = Rng::seeded(5);
        let mut s = listing(&mut rng);
        s.takeover_rate = 0.0;
        s.debt_per_share = 0.0;
        let mut g = globals();
        for day in 1..=3650 {
            g.t = day as f64 * 86_400.0;
            g.v = day as f64 / 365.25;
            let _ = s.advance(&g, None, &mut rng);
            assert!(s.is_finite());
        }
        assert!(s.m.abs() <= 1.5 && s.h.abs() <= 1.5);
    }
}
