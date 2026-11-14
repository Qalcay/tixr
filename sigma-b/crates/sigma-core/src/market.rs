//! Sectors, exchanges and the shared market factors.
//!
//! ## Do we still use different markets?
//! Yes, but they now mean what they mean in the real world:
//!
//! * **Sectors** drive co-movement. Banks move with banks, oil with oil.
//!   Each sector has its own sentiment factor and its own fundamental
//!   profile (growth, payout, cyclicality).
//! * **Exchanges** are listing venues with size requirements, like a main
//!   board and a venture board. A company that grows past a board's ceiling
//!   is promoted at the monthly review; one that shrinks is demoted. The
//!   four board names are kept from tixr alpha.
//!
//! ## Factor model, evaluated lazily
//! A security's log price is
//!
//! ```text
//! ln P = ln V(fundamentals, rates) + beta * S_market + gamma * S_sector + m
//! ```
//!
//! where `S_market` and `S_sector` are global OU factors stepped every
//! engine tick (there are only 12 of them, so this is cheap), and `m` is the
//! security's own mispricing. The global parts are *read at evaluation
//! time*, never stored per stock, so co-movement is exact even for a stock
//! that has not been sampled for an hour: the moment it is sampled, it
//! picks up everything the market did meanwhile.

use crate::macro_econ::ou_step;
use crate::persona::Perk;
use crate::rng::Rng;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Sector {
    Energy = 0,
    Materials,
    Industrials,
    ConsumerDisc,
    Staples,
    Health,
    Financials,
    Tech,
    Comms,
    Utilities,
    RealEstate,
}

pub const SECTORS: [Sector; 11] = [
    Sector::Energy,
    Sector::Materials,
    Sector::Industrials,
    Sector::ConsumerDisc,
    Sector::Staples,
    Sector::Health,
    Sector::Financials,
    Sector::Tech,
    Sector::Comms,
    Sector::Utilities,
    Sector::RealEstate,
];

/// Fundamental profile of a sector.
#[derive(Clone, Copy, Debug)]
pub struct SectorProfile {
    pub beta: f64,
    pub cyclicality: f64,
    pub excess_growth: f64,
    pub payout: f64,
    pub fund_vol: f64,
    pub leverage: f64,
    /// Share of new listings that come from this sector.
    pub share: f64,
}

impl Sector {
    pub fn name(self) -> &'static str {
        match self {
            Sector::Energy => "Energy",
            Sector::Materials => "Materials",
            Sector::Industrials => "Industrials",
            Sector::ConsumerDisc => "Cons. Disc.",
            Sector::Staples => "Staples",
            Sector::Health => "Health",
            Sector::Financials => "Financials",
            Sector::Tech => "Tech",
            Sector::Comms => "Comms",
            Sector::Utilities => "Utilities",
            Sector::RealEstate => "Real Estate",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Sector::Energy => "ENR",
            Sector::Materials => "MAT",
            Sector::Industrials => "IND",
            Sector::ConsumerDisc => "CND",
            Sector::Staples => "STP",
            Sector::Health => "HLT",
            Sector::Financials => "FIN",
            Sector::Tech => "TEC",
            Sector::Comms => "COM",
            Sector::Utilities => "UTL",
            Sector::RealEstate => "REI",
        }
    }

    pub fn from_index(i: u8) -> Sector {
        SECTORS[i as usize % SECTORS.len()]
    }

    pub fn profile(self) -> SectorProfile {
        let p = |beta, cyclicality, excess_growth, payout, fund_vol, leverage, share| SectorProfile {
            beta,
            cyclicality,
            excess_growth,
            payout,
            fund_vol,
            leverage,
            share,
        };
        match self {
            Sector::Energy => p(1.10, 2.5, -0.005, 0.55, 0.30, 0.35, 0.06),
            Sector::Materials => p(1.10, 2.2, 0.000, 0.45, 0.26, 0.30, 0.06),
            Sector::Industrials => p(1.05, 1.8, 0.005, 0.40, 0.20, 0.30, 0.12),
            Sector::ConsumerDisc => p(1.15, 2.0, 0.010, 0.30, 0.24, 0.30, 0.11),
            Sector::Staples => p(0.65, 0.6, 0.000, 0.60, 0.12, 0.30, 0.06),
            Sector::Health => p(0.80, 0.5, 0.015, 0.35, 0.22, 0.20, 0.13),
            Sector::Financials => p(1.10, 1.6, 0.000, 0.45, 0.20, 0.55, 0.12),
            Sector::Tech => p(1.25, 1.2, 0.035, 0.15, 0.30, 0.15, 0.16),
            Sector::Comms => p(1.00, 1.0, 0.015, 0.30, 0.24, 0.30, 0.07),
            Sector::Utilities => p(0.50, 0.4, -0.005, 0.70, 0.10, 0.50, 0.04),
            Sector::RealEstate => p(0.85, 1.2, 0.000, 0.75, 0.16, 0.50, 0.07),
        }
    }

    /// How a sector tilts persona draws.
    pub fn affinity(self) -> &'static [(Perk, f64)] {
        use Perk::*;
        match self {
            Sector::Energy => &[(Cyclical, 2.5), (PricingPower, 1.8), (Dividend, 1.5)],
            Sector::Materials => &[(Cyclical, 2.5), (Overseas, 1.5)],
            Sector::Industrials => &[(Cyclical, 1.8), (Overseas, 1.6), (Bellwether, 1.3)],
            Sector::ConsumerDisc => &[(Hype, 1.8), (Cyclical, 1.5), (Momentum, 1.3)],
            Sector::Staples => &[(Anchor, 2.0), (Dividend, 2.0), (PricingPower, 2.0), (Kontrarian, 1.8)],
            Sector::Health => &[(Innovator, 2.0), (Kontrarian, 1.6), (Gambit, 1.4)],
            Sector::Financials => &[(Bellwether, 2.0), (Junk, 1.6), (Dividend, 1.4)],
            Sector::Tech => &[(Innovator, 2.5), (Network, 2.5), (Emerging, 2.0), (Hype, 1.6)],
            Sector::Comms => &[(Network, 2.0), (Hype, 1.4)],
            Sector::Utilities => &[(Anchor, 2.0), (Dividend, 2.5), (Kontrarian, 2.0), (Fortress, 0.6)],
            Sector::RealEstate => &[(Dividend, 2.5), (Junk, 1.5), (Cyclical, 1.3)],
        }
    }

    pub fn draw(rng: &mut Rng) -> Sector {
        let mut u = rng.uniform();
        for s in SECTORS {
            let share = s.profile().share;
            if u < share {
                return s;
            }
            u -= share;
        }
        Sector::Industrials
    }
}

/// Listing venues, smallest to largest. Board names come from tixr alpha.
pub const EXCHANGES: usize = 4;
pub const EXCHANGE_NAMES: [&str; EXCHANGES] = ["#AST8", "#4MAQ", "#CHA2", "#EX01"];
/// Lower market-cap bound for each board (currency units).
pub const EXCHANGE_MIN_CAP: [f64; EXCHANGES] = [0.0, 3.0e8, 2.0e9, 1.0e10];

/// The board a company of this size belongs on.
pub fn exchange_for_cap(cap: f64) -> u8 {
    let mut ex = 0u8;
    for (i, &min) in EXCHANGE_MIN_CAP.iter().enumerate() {
        if cap >= min {
            ex = i as u8;
        }
    }
    ex
}

/// Global sentiment factors, stepped on variance time.
#[derive(Clone, Debug)]
pub struct Factors {
    pub market: f64,
    pub sector: [f64; 11],
}

impl Factors {
    pub fn new() -> Self {
        Self { market: 0.0, sector: [0.0; 11] }
    }

    /// `dv` is variance-time in years; `vol_mult` the macro fear gauge;
    /// `appetite` tilts where market sentiment settles.
    pub fn step(&mut self, dv: f64, vol_mult: f64, appetite: f64, rng: &mut Rng) {
        if dv <= 0.0 {
            return;
        }
        // Market sentiment: half-life ~2 months, stationary sd ~8%.
        let mean = 0.06 * appetite.tanh();
        self.market = ou_step(self.market, mean, 4.0, 0.22 * vol_mult, dv, rng.normal())
            .clamp(-0.8, 0.8);
        for s in self.sector.iter_mut() {
            *s = ou_step(*s, 0.0, 5.0, 0.18 * vol_mult, dv, rng.normal()).clamp(-0.8, 0.8);
        }
    }
}

impl Default for Factors {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sector_shares_sum_to_one() {
        let s: f64 = SECTORS.iter().map(|s| s.profile().share).sum();
        assert!((s - 1.0).abs() < 1e-9, "{s}");
    }

    #[test]
    fn exchange_tiers() {
        assert_eq!(exchange_for_cap(1.0e8), 0);
        assert_eq!(exchange_for_cap(5.0e8), 1);
        assert_eq!(exchange_for_cap(5.0e9), 2);
        assert_eq!(exchange_for_cap(5.0e11), 3);
    }
}
