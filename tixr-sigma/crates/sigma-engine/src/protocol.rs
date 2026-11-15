//! Messages between the Display thread and the Engine thread.
//!
//! The two threads share **no** mutable state. The UI sends [`Command`]s; the
//! engine replies with owned [`Snapshot`]s. This is the "share memory by
//! communicating" pattern, and it is the idiomatic default for a two-thread
//! Rust program: the compiler proves there are no data races, and neither
//! side can ever block the other on a lock.

use std::sync::Arc;

use crate::broker::{BrokerId, BrokerKind, Strategy};
use crate::console::Level;
use crate::history::History;
use crate::macroecon::{Regime, TENORS};
use crate::persona::Persona;
use crate::security::{SecurityId, SecurityKind, Ticker};
use crate::time::SimTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OrderSize {
    Shares(f64),
    Notional(f64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct LaunchRequest {
    pub capital: f64,
    pub exchange: u8,
    /// `None` draws a random persona.
    pub persona: Option<Persona>,
    /// `None` draws a random ticker.
    pub ticker: Option<String>,
    pub fund: bool,
}

/// Market-wide series with chart history (the Financials window).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MarketSeries {
    Index0 = 0,
    Index1,
    Index2,
    Index3,
    PolicyRate,
    Yield10Y,
    Inflation,
    CreditSpread,
    BrokerEquity,
    FirmEquity,
    Listings,
}

pub const ALL_MARKET_SERIES: [MarketSeries; 11] = [
    MarketSeries::Index0,
    MarketSeries::Index1,
    MarketSeries::Index2,
    MarketSeries::Index3,
    MarketSeries::PolicyRate,
    MarketSeries::Yield10Y,
    MarketSeries::Inflation,
    MarketSeries::CreditSpread,
    MarketSeries::BrokerEquity,
    MarketSeries::FirmEquity,
    MarketSeries::Listings,
];

/// How a series' values should be printed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeriesUnit {
    Level,
    Percent,
    Money,
    Count,
}

impl MarketSeries {
    pub fn label(self) -> &'static str {
        match self {
            MarketSeries::Index0 => "EX01 index",
            MarketSeries::Index1 => "CHA2 index",
            MarketSeries::Index2 => "4MAQ index",
            MarketSeries::Index3 => "AST8 index",
            MarketSeries::PolicyRate => "Policy rate",
            MarketSeries::Yield10Y => "10Y yield",
            MarketSeries::Inflation => "Inflation",
            MarketSeries::CreditSpread => "Credit spread",
            MarketSeries::BrokerEquity => "Brokers: total equity",
            MarketSeries::FirmEquity => "Firms: total equity",
            MarketSeries::Listings => "Listed securities",
        }
    }

    pub fn unit(self) -> SeriesUnit {
        match self {
            MarketSeries::Index0 | MarketSeries::Index1 | MarketSeries::Index2 | MarketSeries::Index3 => {
                SeriesUnit::Level
            }
            MarketSeries::PolicyRate
            | MarketSeries::Yield10Y
            | MarketSeries::Inflation
            | MarketSeries::CreditSpread => SeriesUnit::Percent,
            MarketSeries::BrokerEquity | MarketSeries::FirmEquity => SeriesUnit::Money,
            MarketSeries::Listings => SeriesUnit::Count,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Multiplier on the base pace (1.0 = 90 real minutes per sim day).
    /// Honoured only in builds with time controls (see
    /// [`crate::runner::TIME_CONTROLS`]); a release game is time-locked.
    SetSpeed(f64),
    SetPaused(bool),
    /// Run 8x faster while every exchange is closed (nights, weekends).
    /// Also a time control, so also dev-only.
    SetClosedBoost(bool),
    /// Which market-wide series the snapshot should carry history for.
    SelectSeries(Option<MarketSeries>),
    /// Write the game to the named slot under the save directory.
    Save {
        slot: String,
    },
    /// Replace the running game with the one in the named slot.
    Load {
        slot: String,
    },
    SetLogLevel(Level),
    Select(Option<SecurityId>),
    FocusBroker(Option<BrokerId>),
    Order {
        id: SecurityId,
        side: Side,
        size: OrderSize,
    },
    /// Sell the player's whole position.
    Liquidate(SecurityId),
    Launch(LaunchRequest),
    Restart {
        seed: u64,
    },
    Shutdown,
}

// ---------------------------------------------------------------- snapshot

#[derive(Clone, Copy, Debug)]
pub struct Row {
    pub id: SecurityId,
    pub ticker: Ticker,
    pub exchange: u8,
    pub kind: SecurityKind,
    pub persona: Persona,
    pub price: f64,
    pub change_pct: f32,
    pub market_cap: f64,
}

#[derive(Clone, Debug)]
pub struct MacroView {
    pub policy_rate: f64,
    pub inflation: f64,
    pub output_gap: f64,
    pub term_premium: f64,
    pub erp: f64,
    pub credit_spread: f64,
    pub money_supply: f64,
    pub fair_pe: f64,
    pub taylor_target: f64,
    pub stress: f64,
    pub curve: [f64; TENORS.len()],
    /// Curve-implied short rate 0..5 years ahead (forward curve).
    pub forward_path: Vec<(f64, f64)>,
    pub spread_10y_3m: f64,
}

#[derive(Clone, Debug)]
pub struct ExchangeView {
    pub name: &'static str,
    pub is_open: bool,
    pub index: f64,
    pub change_pct: f64,
    pub pe: f64,
    pub members: usize,
    pub session: (f64, f64),
}

#[derive(Clone, Copy, Debug)]
pub struct ForecastPoint {
    pub days_ahead: f64,
    pub mean: f64,
    pub lo1: f64,
    pub hi1: f64,
    pub lo2: f64,
    pub hi2: f64,
}

#[derive(Clone, Debug)]
pub struct SecurityDetail {
    pub id: SecurityId,
    pub ticker: Ticker,
    pub kind: SecurityKind,
    pub exchange: u8,
    pub persona: Persona,
    pub sponsor: String,
    pub listed_day: i64,
    pub price: f64,
    pub prev_close: f64,
    pub day_open: f64,
    pub day_high: f64,
    pub day_low: f64,
    pub volume_today: f64,
    pub fair_value: f64,
    pub mispricing: f64,
    pub shares: f64,
    pub market_cap: f64,
    pub beta: f64,
    pub duration: f64,
    pub half_life_days: f64,
    pub annual_vol: f64,
    pub distance_to_default: f64,
    pub age_years: f64,
    pub rescues: u16,
    /// All chart tiers; the UI picks a range without a round trip.
    pub history: History,
    pub forecast: Vec<ForecastPoint>,
    pub player_shares: f64,
    pub player_avg_cost: f64,
}

#[derive(Clone, Debug)]
pub struct BrokerRow {
    pub id: BrokerId,
    pub name: String,
    pub kind: BrokerKind,
    pub strategy: Strategy,
    pub cash: f64,
    pub equity: f64,
    pub initial_capital: f64,
    pub positions: usize,
    pub trades: u64,
    pub realized_pnl: f64,
    pub born_day: i64,
    pub listings_sponsored: u32,
}

#[derive(Clone, Debug)]
pub struct HoldingRow {
    pub id: SecurityId,
    pub ticker: Ticker,
    pub shares: f64,
    pub avg_cost: f64,
    pub price: f64,
    pub value: f64,
    pub unrealized: f64,
}

#[derive(Clone, Debug)]
pub struct BrokerDetail {
    pub row: BrokerRow,
    pub holdings: Vec<HoldingRow>,
    pub fees_paid: f64,
    pub interest_earned: f64,
    /// Daily equity history.
    pub history: History,
}

#[derive(Clone, Debug)]
pub struct FillRecord {
    pub time: SimTime,
    pub ticker: Ticker,
    pub side: Side,
    pub shares: f64,
    pub price: f64,
    pub fee: f64,
    pub realized: f64,
}

#[derive(Clone, Debug, Default)]
pub struct PlayerView {
    pub cash: f64,
    pub equity: f64,
    pub starting_cash: f64,
    pub realized_pnl: f64,
    pub unrealized_pnl: f64,
    pub fees_paid: f64,
    pub interest_earned: f64,
    pub holdings: Vec<HoldingRow>,
    pub fills: Vec<FillRecord>,
    pub companies: Vec<(SecurityId, Ticker)>,
}

#[derive(Clone, Debug, Default)]
pub struct PopulationView {
    pub live: usize,
    pub stocks: usize,
    pub funds: usize,
    pub carrying_capacity: f64,
    pub ipos: u64,
    pub bankruptcies: u64,
    pub penny_delistings: u64,
    pub acquisitions: u64,
    pub fund_closures: u64,
    pub splits: u64,
    pub brokers_alive: usize,
    pub firms_alive: usize,
    pub brokers_broke: u64,
    pub rescues: u64,
    pub repairs: u64,
}

#[derive(Clone, Debug)]
pub struct FeedItem {
    pub time: SimTime,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
    Info(String),
    Error(String),
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub time: SimTime,
    /// Day number shown to the user (1 = first day after warm-up).
    pub day: i64,
    /// Sim days before day 1 (to turn any sim time into a display day).
    pub epoch_days: u64,
    pub weekday: &'static str,
    pub clock: String,
    pub seed: u64,
    pub speed: f64,
    pub paused: bool,
    pub closed_boost_active: bool,
    /// Whether this build accepts speed changes at all.
    pub time_controls: bool,
    /// True once this game has ever run at a speed other than 1x.
    pub dev_time_used: bool,
    /// History of the series chosen with [`Command::SelectSeries`].
    pub series: Option<(MarketSeries, History)>,
    pub lag_secs: f64,
    pub events_per_real_sec: f64,
    pub tick_density: f64,
    pub regime: Regime,
    pub regime_days: u32,
    pub macro_view: MacroView,
    pub exchanges: Vec<ExchangeView>,
    pub rows: Vec<Row>,
    pub detail: Option<SecurityDetail>,
    /// Shared, refreshed every couple of real seconds (thousands of rows).
    pub brokers: Arc<Vec<BrokerRow>>,
    pub broker_detail: Option<BrokerDetail>,
    pub player: PlayerView,
    pub population: PopulationView,
    pub feed: Vec<FeedItem>,
    pub notices: Vec<Notice>,
}

crate::tag_enum!(Side { Buy = 0, Sell = 1 });
crate::record!(FillRecord { time, ticker, side, shares, price, fee, realized });
crate::record!(FeedItem { time, text });
