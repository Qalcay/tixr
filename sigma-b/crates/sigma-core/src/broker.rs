//! Brokers: the simulated market participants.
//!
//! A broker is a portfolio plus a strategy plus a life cycle. Brokers are
//! sampled stochastically just like securities: each tick a few are drawn
//! to act, so 160 brokers cost the same as 16 per tick.
//!
//! **Brokers can be firms.** Roughly a quarter of brokers are listed: they
//! have their own ticker in the Financials sector, valued on their book
//! (equity) and return on equity. If a listed broker goes broke its stock
//! defaults with it, wiping out whoever held it, including other brokers.
//!
//! Life cycle: a broker launches with fresh capital, trades, and is declared
//! broke when its equity falls below a quarter of the capital it started
//! with. Its positions are liquidated and a new broker eventually takes its
//! seat, so the cast keeps turning over (active, broke, new).

use crate::persona::{Perk, PerkSet};
use crate::portfolio::Portfolio;
use crate::rng::Rng;
use crate::security::SecurityId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strategy {
    /// Buys below model value, sells above it. The main arbitrage force
    /// that pulls prices back toward fair value.
    Value,
    /// Chases today's winners, cuts losers quickly.
    Momentum,
    /// Holds the biggest companies and rarely trades.
    Index,
    /// Buys dividend payers.
    Income,
    /// Hunts volatile, hyped names; quick take-profit and stop-loss.
    Speculator,
    /// Leans against intraday moves, providing liquidity.
    MarketMaker,
}

pub const STRATEGIES: [Strategy; 6] = [
    Strategy::Value,
    Strategy::Momentum,
    Strategy::Index,
    Strategy::Income,
    Strategy::Speculator,
    Strategy::MarketMaker,
];

impl Strategy {
    pub fn name(self) -> &'static str {
        match self {
            Strategy::Value => "Value",
            Strategy::Momentum => "Momentum",
            Strategy::Index => "Index",
            Strategy::Income => "Income",
            Strategy::Speculator => "Speculator",
            Strategy::MarketMaker => "Market maker",
        }
    }
}

/// What a broker can see about a security when deciding.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub id: SecurityId,
    pub price_base: f64,
    pub mispricing: f64,
    pub change_pct: f64,
    pub vol: f64,
    pub market_cap: f64,
    pub div_yield: f64,
    pub perks: PerkSet,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Intent {
    Buy { id: SecurityId, notional: f64 },
    Sell { id: SecurityId, base_shares: f64 },
}

#[derive(Clone, Debug)]
pub struct Broker {
    pub index: u16,
    pub name: String,
    pub strategy: Strategy,
    pub port: Portfolio,
    pub capital_in: f64,
    pub alive: bool,
    pub listed: Option<SecurityId>,
    pub born_t: f64,
    pub trades: u64,
    /// Last marked equity and when it was marked.
    pub equity: f64,
    pub equity_t: f64,
    /// Smoothed annualised return on equity.
    pub roe: f64,
    /// Fraction of equity committed per buy.
    pub risk: f64,
    /// Max fraction of equity in one name.
    pub max_position: f64,
}

impl Broker {
    pub fn launch(index: u16, t: f64, rng: &mut Rng) -> Self {
        let strategy = STRATEGIES[rng.below(STRATEGIES.len() as u64) as usize];
        let scale = if strategy == Strategy::Index { 4.0 } else { 1.0 };
        let capital = (rng.lognormal(3.0e8, 1.0) * scale).clamp(1.0e7, 5.0e10);
        let (risk, max_position) = match strategy {
            Strategy::Value => (0.03, 0.08),
            Strategy::Momentum => (0.03, 0.06),
            Strategy::Index => (0.02, 0.05),
            Strategy::Income => (0.025, 0.07),
            Strategy::Speculator => (0.05, 0.12),
            Strategy::MarketMaker => (0.01, 0.03),
        };
        Self {
            index,
            name: broker_name(rng),
            strategy,
            port: Portfolio::new(capital, t),
            capital_in: capital,
            alive: true,
            listed: None,
            born_t: t,
            trades: 0,
            equity: capital,
            equity_t: t,
            roe: 0.06,
            risk: risk * rng.range(0.7, 1.4),
            max_position,
        }
    }

    /// Update marked equity and the smoothed ROE.
    pub fn mark(&mut self, equity: f64, t: f64) {
        let dt = (t - self.equity_t) / crate::clock::SECONDS_PER_YEAR;
        if dt > 1e-6 && self.equity > 0.0 && equity > 0.0 {
            let r = ((equity / self.equity).ln() / dt).clamp(-2.0, 2.0);
            let w = (dt / 0.5).min(1.0); // ~6 month smoothing
            self.roe = (1.0 - w) * self.roe + w * r;
        }
        self.equity = equity.max(0.0);
        self.equity_t = t;
    }

    pub fn is_broke(&self) -> bool {
        self.equity < 0.25 * self.capital_in
    }

    /// Decide what to do given held positions and a handful of candidates.
    /// Pure function of its inputs (plus the rng), so it is easy to test.
    pub fn decide(&self, held: &[(View, f64, f64)], candidates: &[View], rng: &mut Rng) -> Vec<Intent> {
        let mut out = Vec::new();
        let equity = self.equity.max(1.0);
        let cash = self.port.cash;

        // 1. Exits. `held` = (view, base_shares, average cost per base share).
        for &(v, shares, avg_cost) in held {
            let gain = if avg_cost > 0.0 { v.price_base / avg_cost - 1.0 } else { 0.0 };
            let sell_frac = match self.strategy {
                Strategy::Value if v.mispricing > 0.03 || gain > 0.4 => 1.0,
                Strategy::Momentum if v.change_pct < -2.0 || gain < -0.08 => 1.0,
                Strategy::Index if cash < 0.02 * equity => 0.1,
                Strategy::Income if v.div_yield < 0.015 => 1.0,
                Strategy::Speculator if !(-0.10..=0.15).contains(&gain) => 1.0,
                Strategy::MarketMaker if v.change_pct > 1.5 || gain > 0.03 => 0.5,
                _ => 0.0,
            };
            if sell_frac > 0.0 {
                out.push(Intent::Sell { id: v.id, base_shares: shares * sell_frac });
            }
        }

        // 2. One entry: score candidates, take the best if it passes.
        let score = |v: &View| -> Option<f64> {
            match self.strategy {
                Strategy::Value => (v.mispricing < -0.04).then_some(-v.mispricing),
                Strategy::Momentum => (v.change_pct > 1.0).then_some(v.change_pct),
                Strategy::Index => Some(v.market_cap.ln()),
                Strategy::Income => (v.div_yield > 0.03).then_some(v.div_yield),
                Strategy::Speculator => {
                    let hype = v.perks.has(Perk::Gambit) || v.perks.has(Perk::Hype);
                    Some(v.vol * if hype { 2.0 } else { 1.0 })
                }
                Strategy::MarketMaker => (v.change_pct < -1.5).then_some(-v.change_pct),
            }
        };
        let best = candidates
            .iter()
            .filter_map(|v| score(v).map(|s| (s, v)))
            .max_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, v)) = best {
            let already = held.iter().find(|h| h.0.id == v.id).map_or(0.0, |h| h.1 * v.price_base);
            let room = (self.max_position * equity - already).max(0.0);
            let spendable = (cash - 0.05 * equity).max(0.0);
            let size = (self.risk * equity * rng.range(0.5, 1.5)).min(room).min(spendable);
            if size > 1_000.0 {
                out.push(Intent::Buy { id: v.id, notional: size });
            }
        }
        out
    }
}

fn broker_name(rng: &mut Rng) -> String {
    const FIRST: [&str; 24] = [
        "Halvorsen", "Pike", "Northgate", "Ashby", "Corvid", "Marlow", "Quill", "Ferro", "Brandt",
        "Okafor", "Lindqvist", "Sato", "Varga", "Delacroix", "Imre", "Kestrel", "Osei", "Rourke",
        "Tamsin", "Vale", "Wexley", "Yarrow", "Zeller", "Arden",
    ];
    const SUFFIX: [&str; 12] = [
        "Capital", "Partners", "& Co", "Securities", "Asset Mgmt", "Holdings", "Trading", "Group",
        "Advisors", "Markets", "Investments", "Brothers",
    ];
    let a = FIRST[rng.below(FIRST.len() as u64) as usize];
    let s = SUFFIX[rng.below(SUFFIX.len() as u64) as usize];
    format!("{a} {s}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(id: u16, mispricing: f64) -> View {
        View {
            id: SecurityId::new(id, 0),
            price_base: 10.0,
            mispricing,
            change_pct: 0.0,
            vol: 0.3,
            market_cap: 1e9,
            div_yield: 0.02,
            perks: PerkSet(0),
        }
    }

    #[test]
    fn value_broker_buys_the_cheapest() {
        let mut rng = Rng::seeded(1);
        let mut b = Broker::launch(0, 0.0, &mut rng);
        b.strategy = Strategy::Value;
        let c = [view(1, -0.02), view(2, -0.20), view(3, -0.08)];
        let intents = b.decide(&[], &c, &mut rng);
        assert!(matches!(intents[..], [Intent::Buy { id, .. }] if id == SecurityId::new(2, 0)));
    }

    #[test]
    fn value_broker_sells_overpriced() {
        let mut rng = Rng::seeded(2);
        let mut b = Broker::launch(0, 0.0, &mut rng);
        b.strategy = Strategy::Value;
        let held = [(view(5, 0.1), 100.0, 10.0)];
        let intents = b.decide(&held, &[], &mut rng);
        assert_eq!(intents, vec![Intent::Sell { id: SecurityId::new(5, 0), base_shares: 100.0 }]);
    }

    #[test]
    fn broke_threshold() {
        let mut rng = Rng::seeded(3);
        let mut b = Broker::launch(0, 0.0, &mut rng);
        b.mark(b.capital_in * 0.2, 1000.0);
        assert!(b.is_broke());
    }
}
