//! Portfolios: shared by the player and every broker.
//!
//! Money rules (see `docs/personas-brokers-money.md`):
//! * Cash is only created or destroyed at clearly named places: interest,
//!   dividends, fees, IPO proceeds, takeover payouts, new broker capital.
//! * Positions are counted in *base shares* (shares before any split), so a
//!   10-for-1 split never needs to touch a portfolio.
//! * Dividends are settled lazily with the same anchor trick the price
//!   engine uses: each position remembers the security's cumulative dividend
//!   per base share when it was last settled, and is owed the difference.
//! * No shorting and no margin: cash can never go negative, so equity can
//!   never go negative, so nothing downstream ever divides by a negative or
//!   zero equity. (One of the NaN sources in tixr alpha.)

use crate::security::SecurityId;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Position {
    pub base_shares: f64,
    /// Total cash paid for the shares still held (for average cost).
    pub cost: f64,
    /// `cum_div_base` of the security when dividends were last settled.
    pub div_anchor: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Portfolio {
    pub cash: f64,
    /// A `BTreeMap`, not a `HashMap`: std's `HashMap` iterates in a random
    /// order per process, which would make runs with the same seed diverge.
    pub positions: BTreeMap<SecurityId, Position>,
    pub realized_pnl: f64,
    pub dividends: f64,
    pub interest: f64,
    pub fees: f64,
    /// Calendar time interest was last accrued.
    pub interest_t: f64,
}

/// Commission on every trade, as a fraction of notional (5 basis points).
pub const FEE_RATE: f64 = 0.0005;

impl Portfolio {
    pub fn new(cash: f64, t: f64) -> Self {
        Self { cash, interest_t: t, ..Default::default() }
    }

    /// Pay interest on cash at the policy rate (continuous compounding).
    pub fn accrue_interest(&mut self, t: f64, rate: f64) {
        let dt = (t - self.interest_t) / crate::clock::SECONDS_PER_YEAR;
        self.interest_t = t;
        if dt > 0.0 && self.cash > 0.0 && rate.is_finite() {
            let earned = self.cash * ((rate * dt).exp() - 1.0);
            self.cash += earned;
            self.interest += earned;
        }
    }

    /// Settle dividends owed on one position. Returns the amount paid.
    pub fn settle_dividends(&mut self, id: SecurityId, cum_div_base: f64) -> f64 {
        let Some(pos) = self.positions.get_mut(&id) else { return 0.0 };
        let owed = (cum_div_base - pos.div_anchor).max(0.0) * pos.base_shares;
        pos.div_anchor = cum_div_base;
        if owed.is_finite() && owed > 0.0 {
            self.cash += owed;
            self.dividends += owed;
            owed
        } else {
            0.0
        }
    }

    /// Buy `notional` worth at `price_per_base`. Returns base shares bought.
    pub fn buy(&mut self, id: SecurityId, notional: f64, price_per_base: f64, cum_div_base: f64) -> f64 {
        if !(notional > 0.0 && price_per_base > 0.0) {
            return 0.0;
        }
        let fee = notional * FEE_RATE;
        if notional + fee > self.cash {
            return 0.0;
        }
        self.settle_dividends(id, cum_div_base);
        let shares = notional / price_per_base;
        self.cash -= notional + fee;
        self.fees += fee;
        let pos = self.positions.entry(id).or_insert(Position { div_anchor: cum_div_base, ..Default::default() });
        pos.base_shares += shares;
        pos.cost += notional;
        shares
    }

    /// Sell up to `base_shares`. Returns cash received (after fee).
    pub fn sell(&mut self, id: SecurityId, base_shares: f64, price_per_base: f64, cum_div_base: f64) -> f64 {
        if !(base_shares > 0.0 && price_per_base > 0.0) {
            return 0.0;
        }
        self.settle_dividends(id, cum_div_base);
        let Some(pos) = self.positions.get_mut(&id) else { return 0.0 };
        let qty = base_shares.min(pos.base_shares);
        if qty <= 0.0 {
            return 0.0;
        }
        let gross = qty * price_per_base;
        let fee = gross * FEE_RATE;
        let cost_part = pos.cost * qty / pos.base_shares;
        pos.base_shares -= qty;
        pos.cost -= cost_part;
        if pos.base_shares <= 1e-9 {
            self.positions.remove(&id);
        }
        self.cash += gross - fee;
        self.fees += fee;
        self.realized_pnl += gross - fee - cost_part;
        gross - fee
    }

    /// Close a position because the security left the market (default,
    /// takeover, delisting). Pays `cash_per_base` per base share.
    pub fn resolve_exit(&mut self, id: SecurityId, cash_per_base: f64, cum_div_base: f64) -> f64 {
        self.settle_dividends(id, cum_div_base);
        let Some(pos) = self.positions.remove(&id) else { return 0.0 };
        let paid = (pos.base_shares * cash_per_base).max(0.0);
        self.cash += paid;
        self.realized_pnl += paid - pos.cost;
        paid
    }

    /// Cash plus marked-to-market positions.
    pub fn equity(&self, mut price_per_base: impl FnMut(SecurityId) -> Option<f64>) -> f64 {
        self.cash
            + self
                .positions
                .iter()
                .map(|(id, p)| price_per_base(*id).map_or(0.0, |px| px * p.base_shares))
                .sum::<f64>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> SecurityId {
        SecurityId::new(3, 1)
    }

    #[test]
    fn buy_then_sell_round_trip_costs_two_fees() {
        let mut p = Portfolio::new(10_000.0, 0.0);
        let sh = p.buy(id(), 1_000.0, 10.0, 0.0);
        assert_eq!(sh, 100.0);
        let back = p.sell(id(), 100.0, 10.0, 0.0);
        assert!((back - 999.5).abs() < 1e-9);
        assert!((p.cash - (10_000.0 - 1_000.5 + 999.5)).abs() < 1e-9);
        assert!(p.positions.is_empty());
        assert!((p.realized_pnl - (-0.5)).abs() < 1e-9);
    }

    #[test]
    fn cannot_overspend() {
        let mut p = Portfolio::new(100.0, 0.0);
        assert_eq!(p.buy(id(), 100.0, 1.0, 0.0), 0.0); // fee pushes over
        assert_eq!(p.cash, 100.0);
    }

    #[test]
    fn dividends_settle_once() {
        let mut p = Portfolio::new(1_000.0, 0.0);
        p.buy(id(), 500.0, 5.0, 0.0); // 100 shares
        assert!((p.settle_dividends(id(), 0.25) - 25.0).abs() < 1e-9);
        assert_eq!(p.settle_dividends(id(), 0.25), 0.0);
    }

    #[test]
    fn interest_accrues() {
        let mut p = Portfolio::new(1_000.0, 0.0);
        p.accrue_interest(crate::clock::SECONDS_PER_YEAR, 0.05);
        assert!((p.cash - 1_000.0 * 0.05f64.exp()).abs() < 1e-6);
    }
}
