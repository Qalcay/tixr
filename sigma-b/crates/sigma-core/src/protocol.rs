//! The message types passed between the engine thread and the display
//! thread. This module is the *entire* contract between the two halves:
//! the UI never touches engine state, it only sends `Command`s and renders
//! the latest `Frame`.
//!
//! Frames are plain owned data (no references, no locks), so the engine
//! can build one, send it, and forget about it. The big stock table is
//! wrapped in an `Arc` and only rebuilt a few times a second; other frames
//! reuse the previous `Arc` for free.

use crate::broker::Strategy;
use crate::clock::{ClockParts, SessionMode};
use crate::log::Level;
use crate::market::Sector;
use crate::persona::PerkSet;
use crate::security::{SecurityId, Ticker};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderKind {
    /// Buy at or below / sell at or above the trigger.
    Limit,
    /// Buy at or above / sell at or below the trigger (stop-loss).
    Stop,
}

/// UI to engine.
#[derive(Clone, Debug)]
pub enum Command {
    SetPaused(bool),
    /// Multiplier on the base pace (1.0 = 90 real minutes per sim day).
    SetSpeed(f64),
    SetSession(SessionMode),
    /// Engine work budget per tick, in milliseconds.
    SetBudgetMs(f64),
    SetLogLevel(Level),
    Select(Option<SecurityId>),
    /// Securities currently visible on screen; they get sampled more often.
    Watch(Vec<SecurityId>),
    Buy { id: SecurityId, notional: f64 },
    /// Sell a fraction (0..=1) of the position.
    Sell { id: SecurityId, fraction: f64 },
    PlaceOrder { id: SecurityId, side: Side, kind: OrderKind, trigger: f64, amount: f64 },
    CancelOrder(u64),
    /// Found a company: pay `capital`, keep `stake` of the shares.
    LaunchIpo { sector: Sector, capital: f64, stake: f64 },
    Reset { seed: u64 },
    Shutdown,
}

/// One row of the big stock table. Kept small and `Copy` (40 bytes) so a
/// 32k-row table is ~1.3 MB.
#[derive(Clone, Copy, Debug)]
pub struct Row {
    pub id: SecurityId,
    pub ticker: Ticker,
    pub exchange: u8,
    pub sector: u8,
    pub perks: PerkSet,
    pub price: f32,
    pub change_pct: f32,
    pub market_cap: f32,
    /// Price relative to model value, in percent (+ = rich, - = cheap).
    pub value_gap_pct: f32,
    pub vol: f32,
}

#[derive(Clone, Debug, Default)]
pub struct ClockView {
    pub t: f64,
    pub parts: ClockParts,
    pub open: bool,
    pub session: Option<SessionMode>,
    pub speed: f64,
    pub paused: bool,
}

#[derive(Clone, Debug, Default)]
pub struct EngineStats {
    pub live: usize,
    pub capacity: usize,
    pub band: (usize, usize),
    pub target: usize,
    pub updates_per_sec: f64,
    pub samples_per_tick: usize,
    pub sweep_per_tick: usize,
    pub mean_age_min: f64,
    pub max_age_min: f64,
    pub tick_ms: f64,
    pub non_finite_events: u64,
    pub ipos: u64,
    pub defaults: u64,
    pub acquisitions: u64,
    pub delistings: u64,
    pub births_per_day: f64,
    pub deaths_per_day: f64,
    pub brokers_alive: usize,
    pub brokers_broke: u64,
}

/// Market-wide drivers shown in the bottom dock.
#[derive(Clone, Debug, Default)]
pub struct Drivers {
    pub policy_rate: f64,
    pub taylor_target: f64,
    pub inflation: f64,
    pub output_gap: f64,
    pub y3m: f64,
    pub y2: f64,
    pub y10: f64,
    pub term_spread: f64,
    pub credit_spread: f64,
    pub erp: f64,
    /// VIX-like fear gauge (about 16 when calm).
    pub fear: f64,
    pub risk_appetite: f64,
    pub recession_prob: f64,
    pub curve: [f64; 8],
    /// Forward curve: where the market expects each tenor's... short rate
    /// to be at that horizon. Used as the rate forecast.
    pub forwards: [f64; 8],
    pub curve_month_ago: Option<[f64; 8]>,
    pub cpi_index: f64,
    pub gdp_index: f64,
    pub advancers: usize,
    pub decliners: usize,
    pub market_sentiment: f64,
    pub sector_sentiment: [f64; 11],
}

#[derive(Clone, Debug, Default)]
pub struct IndexView {
    pub name: String,
    pub level: f64,
    pub change_pct: f64,
    pub members: usize,
    pub history: Vec<f32>,
}

#[derive(Clone, Debug, Default)]
pub struct MacroHistory {
    pub y10: Vec<f32>,
    pub y3m: Vec<f32>,
    pub policy: Vec<f32>,
    pub inflation: Vec<f32>,
    pub live: Vec<f32>,
}

#[derive(Clone, Copy, Debug)]
pub struct CrawlCell {
    pub id: SecurityId,
    pub ticker: Ticker,
    pub price: f32,
    pub change_pct: f32,
}

#[derive(Clone, Debug)]
pub struct Detail {
    pub id: SecurityId,
    pub ticker: Ticker,
    pub sector: Sector,
    pub exchange: u8,
    pub perks: PerkSet,
    pub issuer: String,
    pub price: f64,
    pub prev_close: f64,
    pub day_high: f64,
    pub day_low: f64,
    pub fair: f64,
    pub mispricing: f64,
    pub pe: f64,
    pub div_yield: f64,
    pub beta: f64,
    pub vol: f64,
    pub market_cap: f64,
    pub shares: f64,
    pub leverage: f64,
    pub distance_to_default: f64,
    pub cost_of_equity: f64,
    pub growth: f64,
    pub forecast: (f64, f64, f64),
    pub listed_days: f64,
    pub age_min: f64,
    pub updates: u32,
    pub split_factor: f64,
    pub history: Vec<f32>,
    pub intraday: Vec<f32>,
    pub held_shares: f64,
}

#[derive(Clone, Debug)]
pub struct PositionView {
    pub id: SecurityId,
    pub ticker: Ticker,
    pub sector: Sector,
    pub shares: f64,
    pub price: f64,
    pub avg_cost: f64,
    pub value: f64,
    pub unrealized: f64,
    pub change_pct: f64,
}

#[derive(Clone, Debug)]
pub struct OrderView {
    pub order_id: u64,
    pub id: SecurityId,
    pub ticker: Ticker,
    pub side: Side,
    pub kind: OrderKind,
    pub trigger: f64,
    pub amount: f64,
    pub last_price: f64,
}

#[derive(Clone, Debug)]
pub struct FillView {
    pub t: f64,
    pub text: String,
}

#[derive(Clone, Debug, Default)]
pub struct PlayerView {
    pub cash: f64,
    pub equity: f64,
    pub invested: f64,
    pub unrealized: f64,
    pub realized: f64,
    pub dividends: f64,
    pub interest: f64,
    pub fees: f64,
    pub start_cash: f64,
    pub positions: Vec<PositionView>,
    pub orders: Vec<OrderView>,
    pub fills: Vec<FillView>,
    pub sector_value: [f64; 11],
}

#[derive(Clone, Debug)]
pub struct BrokerRow {
    pub index: u16,
    pub name: String,
    pub strategy: Strategy,
    pub alive: bool,
    pub listed: Option<Ticker>,
    pub cash: f64,
    pub equity: f64,
    pub capital_in: f64,
    pub roe: f64,
    pub positions: usize,
    pub trades: u64,
    pub age_days: f64,
    pub top_holdings: Vec<(Ticker, f64)>,
}

/// Where money came from and went, economy-wide.
#[derive(Clone, Debug, Default)]
pub struct MoneyFlows {
    pub interest_paid: f64,
    pub dividends_paid: f64,
    pub fees_collected: f64,
    pub broker_capital_in: f64,
    pub broker_capital_out: f64,
    pub takeover_cash: f64,
    pub ipo_capital: f64,
    pub cash_total: f64,
    pub market_cap_total: f64,
}

/// Engine to UI: everything one screen needs.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub seed: u64,
    pub clock: ClockView,
    pub stats: EngineStats,
    pub drivers: Drivers,
    pub indices: Vec<IndexView>,
    pub macro_history: MacroHistory,
    pub crawl: Vec<Vec<CrawlCell>>,
    pub selected: Option<Detail>,
    pub player: PlayerView,
    pub brokers: Vec<BrokerRow>,
    pub money: MoneyFlows,
    pub table: Option<Arc<Vec<Row>>>,
    /// Incremented every time `table` is rebuilt.
    pub table_version: u64,
    pub notices: Vec<String>,
}
