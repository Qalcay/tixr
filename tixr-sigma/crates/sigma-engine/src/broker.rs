//! Brokers, firms, portfolios and trading strategies.
//!
//! Every market participant, including the player, owns a [`Portfolio`]:
//! cash plus positions keyed by [`SecurityId`]. Brokers are simulated traders
//! with a strategy. A *firm* is a broker with a balance sheet big enough to
//! also act as an issuer: it can underwrite IPOs and launch index funds.
//!
//! Brokers follow the same life cycle as securities: they are founded, trade,
//! and if their equity collapses they go broke, are liquidated, and a new
//! broker is founded in their place later.

use std::collections::BTreeMap;

use crate::rng::Rng;
use crate::security::SecurityId;
use crate::time::SimTime;

pub const MAX_BROKERS: usize = 256;
/// Fee charged on every trade, as a fraction of notional (5 bps).
pub const FEE_RATE: f64 = 0.0005;
/// Broker is declared broke below this fraction of its starting capital.
pub const BROKE_FRACTION: f64 = 0.50;

/// Generational broker id (slot in low 16 bits, generation in high 16).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BrokerId(pub u32);

impl BrokerId {
    pub fn new(slot: u16, generation: u16) -> Self {
        Self(((generation as u32) << 16) | slot as u32)
    }
    pub fn slot(self) -> usize {
        (self.0 & 0xFFFF) as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrokerKind {
    Broker,
    Firm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strategy {
    /// Buys below estimated fair value, sells above. Arbitrages `u` away.
    Value,
    /// Buys recent winners, dumps losers.
    Momentum,
    /// Holds funds and the largest names, rebalances slowly.
    Index,
    /// Buys sharp drops, sells sharp rallies.
    Contrarian,
    /// Uninformed retail-style flow.
    Noise,
    /// Very large value investor that trades rarely.
    Whale,
}

pub const ALL_STRATEGIES: [Strategy; 6] =
    [Strategy::Value, Strategy::Momentum, Strategy::Index, Strategy::Contrarian, Strategy::Noise, Strategy::Whale];

impl Strategy {
    pub fn label(self) -> &'static str {
        match self {
            Strategy::Value => "Value",
            Strategy::Momentum => "Momentum",
            Strategy::Index => "Index",
            Strategy::Contrarian => "Contrarian",
            Strategy::Noise => "Noise",
            Strategy::Whale => "Whale",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Position {
    pub shares: f64,
    /// Total cost basis (cash paid, including fees).
    pub cost: f64,
}

impl Position {
    pub fn avg_cost(&self) -> f64 {
        if self.shares > 0.0 { self.cost / self.shares } else { 0.0 }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Portfolio {
    pub cash: f64,
    /// `BTreeMap`, not `HashMap`: std's HashMap randomises iteration order per
    /// instance, which made two runs with the same seed diverge.
    pub positions: BTreeMap<SecurityId, Position>,
    pub realized_pnl: f64,
    pub fees_paid: f64,
    pub interest_earned: f64,
}

impl Portfolio {
    pub fn new(cash: f64) -> Self {
        Self { cash, ..Default::default() }
    }

    pub fn shares_of(&self, id: SecurityId) -> f64 {
        self.positions.get(&id).map_or(0.0, |p| p.shares)
    }

    pub fn record_buy(&mut self, id: SecurityId, shares: f64, gross: f64, fee: f64) {
        self.cash -= gross + fee;
        self.fees_paid += fee;
        let pos = self.positions.entry(id).or_default();
        pos.shares += shares;
        pos.cost += gross + fee;
    }

    /// Returns the realised P&L of the sale.
    pub fn record_sell(&mut self, id: SecurityId, shares: f64, gross: f64, fee: f64) -> f64 {
        self.cash += gross - fee;
        self.fees_paid += fee;
        let Some(pos) = self.positions.get_mut(&id) else { return 0.0 };
        let frac = (shares / pos.shares).clamp(0.0, 1.0);
        let basis = pos.cost * frac;
        pos.shares -= shares;
        pos.cost -= basis;
        if pos.shares <= 1e-9 {
            self.positions.remove(&id);
        }
        let pnl = gross - fee - basis;
        self.realized_pnl += pnl;
        pnl
    }

    /// Security left the market: convert the position to cash.
    pub fn settle_exit(&mut self, id: SecurityId, cash_per_share: f64) -> Option<(f64, f64)> {
        let pos = self.positions.remove(&id)?;
        let proceeds = pos.shares * cash_per_share;
        self.cash += proceeds;
        let pnl = proceeds - pos.cost;
        self.realized_pnl += pnl;
        Some((proceeds, pnl))
    }

    pub fn apply_split(&mut self, id: SecurityId, ratio: f64) {
        if let Some(pos) = self.positions.get_mut(&id) {
            pos.shares *= ratio;
        }
    }

    pub fn accrue_interest(&mut self, rate: f64, dt_years: f64) {
        if self.cash > 0.0 {
            let i = self.cash * rate * dt_years;
            self.cash += i;
            self.interest_earned += i;
        }
    }
}

#[derive(Clone, Debug)]
pub struct Broker {
    pub id: BrokerId,
    pub name: String,
    pub kind: BrokerKind,
    pub strategy: Strategy,
    pub port: Portfolio,
    pub initial_capital: f64,
    pub born_at: SimTime,
    pub trades: u64,
    /// Decisions per sim second.
    pub decision_rate: f64,
    /// Fraction of equity committed per trade.
    pub trade_frac: f64,
    /// Max fraction of equity in one name.
    pub max_position_frac: f64,
    /// Max number of distinct names held (keeps books realistic and cheap
    /// to value; without it brokers accumulated hundreds of crumbs).
    pub max_positions: usize,
    /// Sell a position that has lost this fraction of its cost.
    pub stop_loss: f64,
    /// Sell a position that has gained this fraction over its cost.
    pub take_profit: f64,
    /// Annual operating cost as a fraction of starting capital.
    pub opex_rate: f64,
    pub peak_equity: f64,
    pub last_equity: f64,
    pub listings_sponsored: u32,
}

impl Broker {
    pub fn found(id: BrokerId, name: String, kind: BrokerKind, now: SimTime, rng: &mut Rng) -> Self {
        let strategy = ALL_STRATEGIES[rng.below(ALL_STRATEGIES.len())];
        let base_capital = rng.range(5e6, 2e8).max(1.0);
        let capital = match (kind, strategy) {
            (BrokerKind::Firm, _) => base_capital * rng.range(5.0, 25.0),
            (_, Strategy::Whale) => base_capital * 10.0,
            _ => base_capital,
        };
        let minutes_between = match strategy {
            Strategy::Whale => rng.range(60.0, 240.0),
            Strategy::Index => rng.range(30.0, 120.0),
            Strategy::Noise => rng.range(3.0, 15.0),
            _ => rng.range(5.0, 30.0),
        };
        let trade_frac = match strategy {
            Strategy::Whale => rng.range(0.02, 0.05),
            Strategy::Noise => rng.range(0.005, 0.02),
            _ => rng.range(0.01, 0.04),
        };
        Self {
            id,
            name,
            kind,
            strategy,
            port: Portfolio::new(capital),
            initial_capital: capital,
            born_at: now,
            trades: 0,
            decision_rate: 1.0 / (minutes_between * 60.0),
            trade_frac,
            max_position_frac: if strategy == Strategy::Index { 0.25 } else { 0.10 },
            max_positions: match strategy {
                Strategy::Index => 60,
                Strategy::Value => 40,
                Strategy::Whale => 25,
                Strategy::Noise => 20,
                Strategy::Momentum | Strategy::Contrarian => 30,
            },
            stop_loss: match strategy {
                Strategy::Momentum => 0.10,
                Strategy::Noise => 0.25,
                Strategy::Index => 0.50,
                _ => 0.30,
            },
            take_profit: match strategy {
                Strategy::Contrarian => 0.12,
                Strategy::Value | Strategy::Whale => 0.35,
                Strategy::Noise => 0.30,
                _ => 0.60,
            },
            opex_rate: match kind {
                BrokerKind::Firm => rng.range(0.02, 0.05),
                BrokerKind::Broker => rng.range(0.04, 0.12),
            },
            peak_equity: capital,
            last_equity: capital,
            listings_sponsored: 0,
        }
    }

    pub fn is_firm(&self) -> bool {
        self.kind == BrokerKind::Firm
    }
}

/// What a broker can observe about one security when deciding.
#[derive(Clone, Copy, Debug)]
pub struct Signals {
    /// Noisy estimate of ln(price / fair value).
    pub mispricing: f64,
    pub momentum_5d: f64,
    pub momentum_1d: f64,
    pub is_fund: bool,
    pub momentum_persona: bool,
    /// ln(market cap) relative to the market median, ~N(0, 1.5).
    pub size: f64,
}

/// Score > 0 means "want to own more", < 0 "want to own less".
/// Kept as a pure function so it is trivially unit-testable.
pub fn score(strategy: Strategy, s: &Signals, rng: &mut Rng) -> f64 {
    match strategy {
        Strategy::Value | Strategy::Whale => {
            let threshold = if strategy == Strategy::Whale { 0.06 } else { 0.03 };
            // Buy cheap, sell rich (sell threshold is half the buy one, so
            // positions are closed before fair value is fully restored).
            if s.mispricing < -threshold || s.mispricing > threshold * 0.5 { -s.mispricing } else { 0.0 }
        }
        Strategy::Momentum => {
            let boost = if s.momentum_persona { 1.5 } else { 1.0 };
            let m = s.momentum_5d * boost;
            if m.abs() > 0.03 { m } else { 0.0 }
        }
        Strategy::Contrarian => {
            if s.momentum_1d.abs() > 0.04 {
                -s.momentum_1d
            } else {
                0.0
            }
        }
        Strategy::Index => {
            if s.is_fund {
                0.05
            } else if s.size > 1.0 {
                0.02
            } else {
                -0.01
            }
        }
        Strategy::Noise => 0.05 * rng.normal(),
    }
}

const NAME_HEADS: [&str; 24] = [
    "Aster",
    "Bramble",
    "Cobalt",
    "Delta",
    "Ember",
    "Granite",
    "Harbor",
    "Iron",
    "Juniper",
    "Kestrel",
    "Lumen",
    "Meridian",
    "Northgate",
    "Onyx",
    "Pike",
    "Quill",
    "Raven",
    "Summit",
    "Tidal",
    "Umber",
    "Vale",
    "Willow",
    "Yarrow",
    "Zenith",
];
const BROKER_TAILS: [&str; 5] = ["Brokerage", "Securities", "Trading", "Markets", "Partners"];
const FIRM_TAILS: [&str; 5] = ["Capital", "Holdings", "Group", "Ventures", "& Co"];

pub fn random_name(kind: BrokerKind, rng: &mut Rng, taken: &dyn Fn(&str) -> bool) -> String {
    let tails: &[&str] = if kind == BrokerKind::Firm { &FIRM_TAILS } else { &BROKER_TAILS };
    for _ in 0..32 {
        let name = format!("{} {}", NAME_HEADS[rng.below(NAME_HEADS.len())], tails[rng.below(tails.len())]);
        if !taken(&name) {
            return name;
        }
    }
    let mut n = 2;
    loop {
        let name = format!("{} {} {}", NAME_HEADS[rng.below(NAME_HEADS.len())], tails[rng.below(tails.len())], n);
        if !taken(&name) {
            return name;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buy_then_sell_realises_pnl() {
        let mut p = Portfolio::new(1_000.0);
        let id = SecurityId::new(1, 0);
        p.record_buy(id, 10.0, 100.0, 0.0);
        assert_eq!(p.cash, 900.0);
        let pnl = p.record_sell(id, 5.0, 75.0, 0.0);
        assert!((pnl - 25.0).abs() < 1e-9);
        assert!((p.shares_of(id) - 5.0).abs() < 1e-9);
        let pnl2 = p.record_sell(id, 5.0, 40.0, 0.0);
        assert!((pnl2 + 10.0).abs() < 1e-9);
        assert!(p.positions.is_empty());
    }

    #[test]
    fn exit_settles_to_cash() {
        let mut p = Portfolio::new(0.0);
        let id = SecurityId::new(2, 3);
        p.record_buy(id, 100.0, 1_000.0, 0.0);
        let (cash, pnl) = p.settle_exit(id, 13.0).unwrap();
        assert_eq!(cash, 1_300.0);
        assert!((pnl - 300.0).abs() < 1e-9);
        assert_eq!(p.shares_of(id), 0.0);
    }

    #[test]
    fn value_buys_cheap_and_sells_rich() {
        let mut rng = Rng::seed_from_u64(1);
        let mut s = Signals {
            mispricing: -0.1,
            momentum_5d: 0.0,
            momentum_1d: 0.0,
            is_fund: false,
            momentum_persona: false,
            size: 0.0,
        };
        assert!(score(Strategy::Value, &s, &mut rng) > 0.0);
        s.mispricing = 0.1;
        assert!(score(Strategy::Value, &s, &mut rng) < 0.0);
    }
}
