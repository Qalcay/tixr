//! The event-driven engine (path A).
//!
//! # The core idea
//! Nothing in this engine loops over every security on a timer. Instead,
//! everything that happens is an *event* in one [`CalendarQueue`]:
//!
//! | event            | cadence                                  |
//! |------------------|------------------------------------------|
//! | `SecTick(id)`    | Poisson process per security (its liquidity) |
//! | `BrokerDecide`   | Poisson process per broker               |
//! | `ExchangeStep`   | every sim minute (4 market factors)      |
//! | `MacroStep`      | every sim hour (rates, inflation, curve) |
//! | `Open`/`Close`   | session boundaries per exchange          |
//! | `DailyRoll`      | midnight (regime, IPOs, acquisitions)    |
//!
//! A security is touched only when its own exponential clock rings, when a
//! broker or the player trades it, or at its exchange's open/close. Between
//! touches it is *not stale in any way that matters*: its price model has an
//! exact closed-form transition (see [`crate::security`]), so catching up
//! over 3 seconds or 3 days is the same O(1) computation. The update rate is
//! therefore a pure presentation choice (how often a ticker prints), and the
//! engine scales it down automatically when the CPU budget is tight
//! (`tick_density`) without changing a single statistic of the market.

use std::collections::{HashSet, VecDeque};

use crate::broker::{
    BROKE_FRACTION, Broker, BrokerId, BrokerKind, FEE_RATE, MAX_BROKERS, Signals, Strategy, random_name, score,
};
use crate::console;
use crate::console::Level;
use crate::exchange::{EXCHANGE_COUNT, EXCHANGE_SPECS, Exchange};
use crate::macroecon::{ERP_NEUTRAL, MacroState, Regime, TENORS};
use crate::persona::{Perk, Persona};
use crate::protocol::*;
use crate::queue::CalendarQueue;
use crate::rng::Rng;
use crate::security::{
    Exit, Factors, ListingSpec, MAX_MISPRICING, Security, SecurityId, SecurityKind, Sponsor, Ticker,
};
use crate::time::*;

/// Hard cap on simultaneously listed securities (2^16).
pub const MAX_SECURITIES: usize = 1 << 16;
/// Band the population is steered into (2^13 ..= 2^15).
pub const TARGET_MIN: f64 = 8_192.0;
pub const TARGET_MAX: f64 = 32_768.0;
pub const WARMUP_DAYS: u64 = 28;
const EXCHANGE_STEP_SECS: f64 = 60.0;
const MACRO_STEP_SECS: f64 = 3_600.0;
const FEED_LEN: usize = 200;
const PLAYER_FILLS: usize = 200;

#[derive(Clone, Debug)]
pub struct EngineConfig {
    pub seed: u64,
    pub initial_securities: usize,
    pub brokers: usize,
    pub firm_share: f64,
    pub player_cash: f64,
    /// Run the warm-up month so charts have history on the first frame.
    pub warmup: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            seed: 0x5167_4D41_7469_7872, // "SigMAtixr"
            initial_securities: 14_000,
            brokers: MAX_BROKERS,
            firm_share: 0.25,
            player_cash: 1_000_000.0,
            warmup: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ev {
    /// Price print for a security; the epoch lets a whole schedule be
    /// invalidated at once (see `reset_tick_schedule`).
    SecTick(SecurityId, u32),
    BrokerDecide(BrokerId),
    BrokerFound(u16),
    ExchangeStep,
    MacroStep,
    DailyRoll,
    Open(u8),
    Close(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trader {
    Player,
    Broker(BrokerId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TradeError {
    NoSuchSecurity,
    MarketClosed,
    InsufficientCash,
    NothingToSell,
    TooSmall,
}

impl TradeError {
    pub fn message(self) -> &'static str {
        match self {
            TradeError::NoSuchSecurity => "that security is no longer listed",
            TradeError::MarketClosed => "its exchange is closed",
            TradeError::InsufficientCash => "not enough cash",
            TradeError::NothingToSell => "no shares to sell",
            TradeError::TooSmall => "order too small",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Fill {
    pub id: SecurityId,
    pub ticker: Ticker,
    pub side: Side,
    pub shares: f64,
    pub price: f64,
    pub fee: f64,
    pub realized: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub events: u64,
    pub sec_ticks: u64,
    /// Tick events that fired while their exchange was closed (re-queued).
    pub closed_ticks: u64,
    pub broker_decisions: u64,
    pub ipos: u64,
    pub bankruptcies: u64,
    pub penny_delistings: u64,
    pub acquisitions: u64,
    pub fund_closures: u64,
    pub splits: u64,
    pub brokers_broke: u64,
    pub brokers_founded: u64,
    pub repairs: u64,
    pub trades: u64,
}

pub struct Player {
    pub port: crate::broker::Portfolio,
    pub starting_cash: f64,
    pub fills: VecDeque<FillRecord>,
    pub companies: Vec<SecurityId>,
}

/// Result of [`Engine::advance_to`].
#[derive(Clone, Copy, Debug)]
pub struct Advance {
    pub events: u64,
    /// `false` if the time budget ran out before reaching the target.
    pub reached: bool,
}

pub struct Engine {
    pub now: SimTime,
    seed: u64,
    rng: Rng,
    name_rng: Rng,
    queue: CalendarQueue<Ev>,
    pub macro_state: MacroState,
    pub exchanges: Vec<Exchange>,
    factors: [Factors; EXCHANGE_COUNT],

    slots: Vec<Option<Security>>,
    generations: Vec<u16>,
    free_slots: Vec<u16>,
    live: Vec<u16>,
    live_pos: Vec<u32>,
    tickers: HashSet<Ticker>,

    brokers: Vec<Option<Broker>>,
    broker_generations: Vec<u16>,
    pub player: Player,

    pub carrying_capacity: f64,
    /// Multiplier on every security's print rate (0, 1]. Presentation only.
    pub tick_density: f64,
    tick_epoch: u32,
    median_log_cap: f64,
    pub stats: Stats,
    feed: VecDeque<FeedItem>,
    notices: Vec<Notice>,
    pub selected: Option<SecurityId>,
    pub focused_broker: Option<BrokerId>,
}

impl Engine {
    pub fn new(config: EngineConfig) -> Self {
        let mut rng = Rng::seed_from_u64(config.seed);
        let name_rng = rng.fork(0x4E41_4D45);
        let macro_state = MacroState::new();
        let exchanges: Vec<Exchange> =
            (0..EXCHANGE_COUNT as u8).map(|i| Exchange::new(i, &macro_state, &mut rng)).collect();

        let mut eng = Self {
            now: 0.0,
            seed: config.seed,
            rng,
            name_rng,
            queue: CalendarQueue::new(4_096, 0.25),
            macro_state,
            exchanges,
            factors: [[0.0; 4]; EXCHANGE_COUNT],
            slots: Vec::new(),
            generations: Vec::new(),
            free_slots: Vec::new(),
            live: Vec::new(),
            live_pos: Vec::new(),
            tickers: HashSet::new(),
            brokers: (0..MAX_BROKERS).map(|_| None).collect(),
            broker_generations: vec![0; MAX_BROKERS],
            player: Player {
                port: crate::broker::Portfolio::new(config.player_cash),
                starting_cash: config.player_cash,
                fills: VecDeque::new(),
                companies: Vec::new(),
            },
            carrying_capacity: 16_384.0,
            tick_density: 1.0,
            tick_epoch: 0,
            median_log_cap: (2e9f64).ln(),
            stats: Stats::default(),
            feed: VecDeque::new(),
            notices: Vec::new(),
            selected: None,
            focused_broker: None,
        };
        eng.refresh_factors();
        eng.carrying_capacity = eng.compute_carrying_capacity();
        // During warm-up prints are sparse (see below), so schedule the very
        // first ticks sparsely too.
        let warm_density = 0.002;
        if config.warmup {
            eng.tick_density = warm_density;
        }

        // Initial listings, spread over exchanges and pre-aged.
        let weights = [30u32, 25, 25, 20];
        for _ in 0..config.initial_securities.min(MAX_SECURITIES) {
            let ex = eng.rng.weighted_index(&weights) as u8;
            let kind = if eng.rng.chance(0.02) { SecurityKind::Fund } else { SecurityKind::Stock };
            let initial_y = (0.35 * eng.rng.normal()).clamp(-1.0, 1.0);
            eng.list_new(kind, ex, None, None, Sponsor::Engine, initial_y);
        }
        let firms = ((config.brokers as f64) * config.firm_share).round() as usize;
        for slot in 0..config.brokers.min(MAX_BROKERS) {
            let kind = if slot < firms { BrokerKind::Firm } else { BrokerKind::Broker };
            eng.found_broker(slot as u16, kind);
        }

        // Recurring events.
        for ex in 0..EXCHANGE_COUNT as u8 {
            eng.schedule_session(ex);
        }
        eng.queue.push(EXCHANGE_STEP_SECS, Ev::ExchangeStep);
        eng.queue.push(MACRO_STEP_SECS, Ev::MacroStep);
        eng.queue.push(SECS_PER_DAY, Ev::DailyRoll);

        if config.warmup {
            // A month of history at a low print rate: cheap, statistically
            // identical (closed-form catch-up), and the chart is not empty.
            let warm_end = WARMUP_DAYS as f64 * SECS_PER_DAY + 8.0 * SECS_PER_HOUR;
            eng.advance_to(warm_end, None);
            eng.tick_density = 1.0;
            eng.reset_tick_schedule();
            // The player arrives on day 1: no warm-up interest or history.
            eng.player.port = crate::broker::Portfolio::new(config.player_cash);
            eng.feed.clear();
        }
        console!(
            Level::Info,
            eng.now,
            "engine ready: seed={:#x} live={} brokers={} K={:.0}",
            eng.seed,
            eng.live.len(),
            eng.brokers_alive(),
            eng.carrying_capacity
        );
        eng
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn live_count(&self) -> usize {
        self.live.len()
    }

    pub fn brokers_alive(&self) -> usize {
        self.brokers.iter().filter(|b| b.is_some()).count()
    }

    pub fn security(&self, id: SecurityId) -> Option<&Security> {
        self.slots.get(id.slot())?.as_ref().filter(|s| s.id == id)
    }

    fn security_mut(&mut self, id: SecurityId) -> Option<&mut Security> {
        self.slots.get_mut(id.slot())?.as_mut().filter(|s| s.id == id)
    }

    pub fn broker(&self, id: BrokerId) -> Option<&Broker> {
        self.brokers.get(id.slot())?.as_ref().filter(|b| b.id == id)
    }

    pub fn live_ids(&self) -> impl Iterator<Item = SecurityId> + '_ {
        self.live.iter().filter_map(|&s| self.slots[s as usize].as_ref().map(|x| x.id))
    }

    pub fn any_exchange_open(&self) -> bool {
        self.exchanges.iter().any(|e| e.is_open)
    }

    pub fn drain_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    // ------------------------------------------------------------ factors

    fn refresh_factors(&mut self) {
        let m = &self.macro_state;
        let (y10, pi, cs) = (m.y10(), m.inflation, m.credit_spread);
        for (i, ex) in self.exchanges.iter().enumerate() {
            self.factors[i] = [ex.factor, -y10, pi, -cs];
        }
    }

    fn compute_carrying_capacity(&self) -> f64 {
        let m = &self.macro_state;
        let k = 16_384.0 * (6.0 * m.output_gap + 5.0 * (ERP_NEUTRAL - m.erp)).exp();
        k.clamp(9_000.0, 30_000.0)
    }

    // ------------------------------------------------------------ main loop

    /// Process every event with time `<= target`, stopping early if the
    /// optional real-time `deadline` passes. Deterministic: the same seed and
    /// the same commands give the same market regardless of how the run is
    /// chopped into `advance_to` calls.
    pub fn advance_to(&mut self, target: SimTime, deadline: Option<std::time::Instant>) -> Advance {
        let mut events = 0u64;
        while let Some((t, ev)) = self.queue.pop_due(target) {
            self.now = t.max(self.now);
            self.handle(ev);
            events += 1;
            if events.is_multiple_of(2_048)
                && let Some(d) = deadline
                && std::time::Instant::now() >= d
            {
                self.stats.events += events;
                return Advance { events, reached: false };
            }
        }
        self.now = self.now.max(target);
        self.stats.events += events;
        Advance { events, reached: true }
    }

    fn handle(&mut self, ev: Ev) {
        match ev {
            Ev::SecTick(id, epoch) => {
                if epoch == self.tick_epoch {
                    self.on_sec_tick(id);
                }
            }
            Ev::BrokerDecide(id) => self.on_broker_decide(id),
            Ev::BrokerFound(slot) => {
                let kind = if self.rng.chance(0.25) { BrokerKind::Firm } else { BrokerKind::Broker };
                self.found_broker(slot, kind);
            }
            Ev::ExchangeStep => {
                let dt = years(EXCHANGE_STEP_SECS);
                let global_z = self.rng.normal();
                for ex in self.exchanges.iter_mut() {
                    ex.step(&self.macro_state, dt, global_z, &mut self.rng);
                }
                self.refresh_factors();
                self.queue.push(self.now + EXCHANGE_STEP_SECS, Ev::ExchangeStep);
            }
            Ev::MacroStep => {
                let dt = years(MACRO_STEP_SECS);
                self.macro_state.step(dt, &mut self.rng);
                let rate = self.macro_state.policy_rate;
                for b in self.brokers.iter_mut().flatten() {
                    b.port.accrue_interest(rate, dt);
                    // Operating costs (staff, data, rent): the steady bleed
                    // that eventually sinks brokers without an edge.
                    let opex = b.initial_capital * b.opex_rate * dt;
                    b.port.cash -= opex;
                    b.port.realized_pnl -= opex;
                }
                self.player.port.accrue_interest(rate, dt);
                self.refresh_factors();
                self.queue.push(self.now + MACRO_STEP_SECS, Ev::MacroStep);
            }
            Ev::DailyRoll => self.on_daily_roll(),
            Ev::Open(ex) => self.on_open(ex),
            Ev::Close(ex) => self.on_close(ex),
        }
    }

    fn schedule_session(&mut self, ex: u8) {
        let session = EXCHANGE_SPECS[ex as usize].session;
        if session.is_open(self.now) {
            self.exchanges[ex as usize].is_open = true;
            self.queue.push(session.next_close_after(self.now), Ev::Close(ex));
        } else {
            self.exchanges[ex as usize].is_open = false;
            self.queue.push(session.next_open_at_or_after(self.now), Ev::Open(ex));
        }
    }

    // ------------------------------------------------------------ securities

    fn schedule_tick(&mut self, id: SecurityId, rate: f64, exchange: u8) {
        let ex = &self.exchanges[exchange as usize];
        let from = if ex.is_open {
            self.now
        } else {
            EXCHANGE_SPECS[exchange as usize].session.next_open_at_or_after(self.now)
        };
        let dt = self.rng.exponential((rate * self.tick_density).max(1e-9));
        self.queue.push(from + dt, Ev::SecTick(id, self.tick_epoch));
    }

    /// Drop every pending print and draw a fresh one per security at the
    /// current density. O(N), used when the density jumps (end of warm-up).
    pub fn reset_tick_schedule(&mut self) {
        self.tick_epoch = self.tick_epoch.wrapping_add(1);
        let ids: Vec<(SecurityId, f64, u8)> = self
            .live
            .iter()
            .filter_map(|&s| self.slots[s as usize].as_ref())
            .map(|s| (s.id, s.params.tick_rate, s.exchange))
            .collect();
        for (id, rate, ex) in ids {
            self.schedule_tick(id, rate, ex);
        }
    }

    /// Exact catch-up of one security to `now`. Returns false if missing.
    fn catch_up(&mut self, id: SecurityId) -> bool {
        let now = self.now;
        let (vol, jump) = (self.macro_state.regime.vol_mult(), self.macro_state.regime.jump_mult());
        let slot = id.slot();
        let Some(sec) = self.slots.get_mut(slot).and_then(|s| s.as_mut()).filter(|s| s.id == id) else {
            return false;
        };
        let f = self.factors[sec.exchange as usize];
        sec.advance(now, &f, vol, jump, &mut self.rng);
        if !sec.price.is_finite() || !sec.reprice() {
            self.stats.repairs += 1;
            console!(Level::Warn, now, "repaired non-finite state on {}", sec.ticker);
        }
        true
    }

    fn on_sec_tick(&mut self, id: SecurityId) {
        let Some(sec) = self.security(id) else { return }; // stale event: delisted
        let (exchange, rate) = (sec.exchange, sec.params.tick_rate);
        if !self.exchanges[exchange as usize].is_open {
            self.stats.closed_ticks += 1;
            self.schedule_tick(id, rate, exchange);
            return;
        }
        self.catch_up(id);
        self.stats.sec_ticks += 1;
        let sec = self.security_mut(id).expect("checked above");
        sec.record_tick();
        if sec.distance_to_default() < 0.0 {
            self.delist(id, Exit::Bankrupt);
            return;
        }
        self.schedule_tick(id, rate, exchange);
    }

    fn members_of(&self, ex: u8) -> Vec<SecurityId> {
        self.live
            .iter()
            .filter_map(|&s| self.slots[s as usize].as_ref())
            .filter(|s| s.exchange == ex)
            .map(|s| s.id)
            .collect()
    }

    fn on_open(&mut self, ex: u8) {
        self.exchanges[ex as usize].is_open = true;
        let session = EXCHANGE_SPECS[ex as usize].session;
        self.queue.push(session.next_close_after(self.now), Ev::Close(ex));
        // Opening auction: every member reprices once, gaps included.
        for id in self.members_of(ex) {
            self.catch_up(id);
            if let Some(s) = self.security_mut(id) {
                s.record_tick();
            }
        }
        console!(
            Level::Debug,
            self.now,
            "{} open, index {:.1}",
            EXCHANGE_SPECS[ex as usize].name,
            self.exchanges[ex as usize].index_level()
        );
    }

    fn on_close(&mut self, ex: u8) {
        let mut exits = Vec::new();
        let mut splits = Vec::new();
        for id in self.members_of(ex) {
            self.catch_up(id);
            let Some(s) = self.security_mut(id) else { continue };
            if let Some(exit) = s.close_session() {
                exits.push((id, exit));
            } else if s.distance_to_default() < 0.0 {
                exits.push((id, Exit::Bankrupt));
            } else if let Some(r) = s.maybe_split() {
                splits.push((id, r));
            }
        }
        for (id, r) in splits {
            self.stats.splits += 1;
            for b in self.brokers.iter_mut().flatten() {
                b.port.apply_split(id, r);
            }
            self.player.port.apply_split(id, r);
            if let Some(t) = self.security(id).map(|s| s.ticker) {
                self.push_feed(format!("{t} splits {r:.0}-for-1"));
            }
        }
        for (id, exit) in exits {
            self.delist(id, exit);
        }
        let e = &mut self.exchanges[ex as usize];
        e.is_open = false;
        e.prev_close_index = e.index_level();
        let session = EXCHANGE_SPECS[ex as usize].session;
        self.queue.push(session.next_open_at_or_after(self.now + 1.0), Ev::Open(ex));
        console!(
            Level::Debug,
            self.now,
            "{} close, index {:.1}",
            EXCHANGE_SPECS[ex as usize].name,
            self.exchanges[ex as usize].index_level()
        );
    }

    fn unique_ticker(&mut self, kind: SecurityKind, ex: u8, wanted: Option<&str>) -> Option<Ticker> {
        if let Some(w) = wanted {
            let w = w.trim().to_ascii_uppercase();
            if (2..=5).contains(&w.len()) && w.bytes().all(|b| b.is_ascii_uppercase()) {
                let t = Ticker::new(&w);
                if !self.tickers.contains(&t) {
                    self.tickers.insert(t);
                    return Some(t);
                }
            }
            return None;
        }
        let (len, suffix) = match kind {
            SecurityKind::Fund => (5, Some(b'X')),
            SecurityKind::Stock => (EXCHANGE_SPECS[ex as usize].ticker_len, None),
        };
        for attempt in 0..256 {
            // If short names are crowded, fall back to one letter longer.
            let l = if attempt < 64 { len } else { (len + 1).min(5) };
            let t = Ticker::random(&mut self.name_rng, l, suffix);
            if !self.tickers.contains(&t) {
                self.tickers.insert(t);
                return Some(t);
            }
        }
        None
    }

    /// List a new security. Returns its id, or `None` at the hard cap.
    fn list_new(
        &mut self,
        kind: SecurityKind,
        exchange: u8,
        persona: Option<Persona>,
        ticker: Option<Ticker>,
        sponsor: Sponsor,
        initial_y: f64,
    ) -> Option<SecurityId> {
        if self.live.len() >= MAX_SECURITIES {
            return None;
        }
        let ticker = match ticker {
            Some(t) => t,
            None => self.unique_ticker(kind, exchange, None)?,
        };
        let slot = match self.free_slots.pop() {
            Some(s) => s,
            None => {
                let s = self.slots.len();
                if s >= MAX_SECURITIES {
                    return None;
                }
                self.slots.push(None);
                self.generations.push(0);
                self.live_pos.push(u32::MAX);
                s as u16
            }
        };
        let generation = self.generations[slot as usize];
        let id = SecurityId::new(slot, generation);
        let persona = persona.unwrap_or_else(|| match kind {
            SecurityKind::Fund => Persona::EMPTY.with(Perk::Liquid),
            SecurityKind::Stock => Persona::random(&mut self.rng),
        });
        let ipo_price = match kind {
            SecurityKind::Fund => self.rng.range(20.0, 120.0),
            SecurityKind::Stock => (self.rng.range(2.0f64.ln(), 300.0f64.ln())).exp(),
        };
        let market_cap = (self.median_log_cap + 1.4 * self.rng.normal()).exp().clamp(5e7, 4e12);
        let spec = ListingSpec { kind, exchange, persona, ticker, ipo_price, market_cap, sponsor, initial_y };
        let factors = self.factors[exchange as usize];
        let sec = Security::list(id, spec, &factors, self.now, &mut self.rng);
        let rate = sec.params.tick_rate;
        self.slots[slot as usize] = Some(sec);
        self.live_pos[slot as usize] = self.live.len() as u32;
        self.live.push(slot);
        self.schedule_tick(id, rate, exchange);
        Some(id)
    }

    fn delist(&mut self, id: SecurityId, exit: Exit) {
        let slot = id.slot();
        let Some(sec) = self.slots[slot].take().filter(|s| s.id == id) else { return };
        // Swap-remove from the dense live list in O(1).
        let pos = self.live_pos[slot] as usize;
        let last = *self.live.last().expect("non-empty");
        self.live.swap_remove(pos);
        if pos < self.live.len() {
            self.live_pos[last as usize] = pos as u32;
        }
        self.live_pos[slot] = u32::MAX;
        self.generations[slot] = self.generations[slot].wrapping_add(1);
        self.free_slots.push(slot as u16);
        self.tickers.remove(&sec.ticker);

        let per_share = sec.price * exit.payout_multiple();
        for b in self.brokers.iter_mut().flatten() {
            b.port.settle_exit(id, per_share);
        }
        if let Some((cash, pnl)) = self.player.port.settle_exit(id, per_share) {
            self.notices.push(Notice::Info(format!(
                "{} {}: your position paid out {} (P&L {:+.0})",
                sec.ticker,
                exit.label(),
                fmt_money(cash),
                pnl
            )));
        }
        match exit {
            Exit::Bankrupt => self.stats.bankruptcies += 1,
            Exit::PennyDelisted => self.stats.penny_delistings += 1,
            Exit::Acquired => self.stats.acquisitions += 1,
            Exit::FundClosed => self.stats.fund_closures += 1,
        }
        if self.selected == Some(id) {
            self.selected = None;
        }
        self.push_feed(format!("{} {} at {:.2}", sec.ticker, exit.label(), sec.price));
        console!(
            Level::Debug,
            self.now,
            "{} {} (price {:.2}, persona {})",
            sec.ticker,
            exit.label(),
            sec.price,
            sec.persona.letters()
        );
    }

    fn push_feed(&mut self, text: String) {
        if self.feed.len() == FEED_LEN {
            self.feed.pop_front();
        }
        self.feed.push_back(FeedItem { time: self.now, text });
    }

    // ------------------------------------------------------------ daily roll

    fn on_daily_roll(&mut self) {
        self.queue.push(self.now + SECS_PER_DAY, Ev::DailyRoll);
        if let Some(r) = self.macro_state.step_regime_daily(&mut self.rng) {
            self.push_feed(format!("Regime shift: {}", r.label()));
            console!(Level::Info, self.now, "regime -> {}", r.label());
        }
        self.carrying_capacity = self.compute_carrying_capacity();
        self.update_median_cap();
        self.population_step();
        self.firm_actions();
        for b in self.brokers.iter_mut().flatten() {
            b.peak_equity = b.peak_equity.max(b.last_equity);
        }
        let m = &self.macro_state;
        console!(
            Level::Info,
            self.now,
            "live={} K={:.0} ipo={} bk={} acq={} | {} | 3M {:.2}% 10Y {:.2}% pi {:.2}% | idx {:.0}/{:.0}/{:.0}/{:.0} | repairs={}",
            self.live.len(),
            self.carrying_capacity,
            self.stats.ipos,
            self.stats.bankruptcies,
            self.stats.acquisitions,
            m.regime.label(),
            m.curve.yield_at(0.25) * 100.0,
            m.y10() * 100.0,
            m.inflation * 100.0,
            self.exchanges[0].index_level(),
            self.exchanges[1].index_level(),
            self.exchanges[2].index_level(),
            self.exchanges[3].index_level(),
            self.stats.repairs
        );
    }

    fn update_median_cap(&mut self) {
        // Median of a sample is plenty for "is this a big company?".
        let n = self.live.len();
        if n == 0 {
            return;
        }
        let mut caps: Vec<f64> = (0..256.min(n))
            .filter_map(|_| {
                let slot = self.live[self.rng.below(n)];
                self.slots[slot as usize].as_ref().map(|s| s.market_cap().max(1.0).ln())
            })
            .collect();
        caps.sort_by(|a, b| a.total_cmp(b));
        if let Some(&m) = caps.get(caps.len() / 2) {
            // Keep new listings from inheriting runaway inflation of caps.
            self.median_log_cap = m.clamp((2e8f64).ln(), (5e10f64).ln());
        }
    }

    /// Births (IPOs) and acquisitions, steering the population toward the
    /// carrying capacity K. Logistic-style: births fall and acquisitions
    /// rise as N approaches and passes K, and K itself follows the economy.
    fn population_step(&mut self) {
        let n = self.live.len().max(1) as f64;
        let k = self.carrying_capacity;
        let m = &self.macro_state;
        let hot = (8.0 * (ERP_NEUTRAL - m.erp)).exp().clamp(0.3, 2.5);
        let crowd = (k / n).powi(2).clamp(0.05, 8.0);
        // Hard band: never below 2^13, never above 2^15 in normal operation.
        let floor_push = if n < TARGET_MIN { 4.0 } else { 1.0 };
        let ceiling_cut = if n > TARGET_MAX { 0.0 } else { 1.0 };
        let birth_rate_year = 0.05 * n * crowd * hot * floor_push * ceiling_cut;
        let births = self.rng.poisson(birth_rate_year / 365.0);

        let acq_hazard = 0.04 * (n / k).powi(2) * if n > TARGET_MAX { 4.0 } else { 1.0 };
        let acquisitions = self.rng.poisson(n * acq_hazard / 365.0);

        for _ in 0..births {
            let ex = self.rng.below(EXCHANGE_COUNT) as u8;
            let fund = self.rng.chance(0.03);
            let firm = if self.rng.chance(0.4) { self.random_firm() } else { None };
            let sponsor = firm.map_or(Sponsor::Engine, Sponsor::Firm);
            let kind = if fund { SecurityKind::Fund } else { SecurityKind::Stock };
            if let Some(id) = self.list_new(kind, ex, None, None, sponsor, 0.0) {
                self.stats.ipos += 1;
                if let Some(fid) = firm {
                    self.underwrite(fid, id);
                }
                let (t, p) = self.security(id).map(|s| (s.ticker, s.price)).unwrap_or((Ticker::new("?"), 0.0));
                let who = firm.and_then(|f| self.broker(f)).map_or("market".to_string(), |b| b.name.clone());
                self.push_feed(format!("IPO {t} on {} at {p:.2} ({who})", EXCHANGE_SPECS[ex as usize].name));
            }
        }
        for _ in 0..acquisitions {
            if self.live.is_empty() {
                break;
            }
            let slot = self.live[self.rng.below(self.live.len())];
            let Some(sec) = self.slots[slot as usize].as_ref() else { continue };
            if sec.kind == SecurityKind::Stock {
                let id = sec.id;
                self.catch_up(id);
                self.delist(id, Exit::Acquired);
            }
        }
    }

    fn random_firm(&mut self) -> Option<BrokerId> {
        let firms: Vec<BrokerId> = self.brokers.iter().flatten().filter(|b| b.is_firm()).map(|b| b.id).collect();
        if firms.is_empty() { None } else { Some(firms[self.rng.below(firms.len())]) }
    }

    /// Firm underwrites an IPO: earns a 2% fee on a 25% float and buys 5% of
    /// the company at the offer price.
    fn underwrite(&mut self, firm: BrokerId, id: SecurityId) {
        let Some((cap, price)) = self.security(id).map(|s| (s.market_cap(), s.price)) else { return };
        let Some(b) = self.brokers[firm.slot()].as_mut().filter(|b| b.id == firm) else { return };
        let fee = cap * 0.25 * 0.02;
        b.port.cash += fee;
        b.port.realized_pnl += fee;
        let stake = (cap * 0.05).min(b.port.cash * 0.2);
        if stake > 0.0 {
            b.port.record_buy(id, stake / price, stake, 0.0);
        }
        b.listings_sponsored += 1;
    }

    /// Wealthy firms occasionally launch index funds.
    fn firm_actions(&mut self) {
        let funds = self
            .live
            .iter()
            .filter(|&&s| matches!(self.slots[s as usize].as_ref(), Some(x) if x.kind == SecurityKind::Fund))
            .count();
        let fund_cap = (self.live.len() / 40).max(20);
        if funds >= fund_cap {
            return;
        }
        let candidates: Vec<BrokerId> = self
            .brokers
            .iter()
            .flatten()
            .filter(|b| b.is_firm() && b.last_equity > 1.5 * b.initial_capital)
            .map(|b| b.id)
            .collect();
        for fid in candidates {
            if !self.rng.chance(0.01) {
                continue;
            }
            let ex = self.rng.below(EXCHANGE_COUNT) as u8;
            if let Some(id) = self.list_new(SecurityKind::Fund, ex, None, None, Sponsor::Firm(fid), 0.0) {
                self.stats.ipos += 1;
                self.underwrite(fid, id);
                let t = self.security(id).map(|s| s.ticker).unwrap_or(Ticker::new("?"));
                let name = self.broker(fid).map(|b| b.name.clone()).unwrap_or_default();
                self.push_feed(format!("{name} launches fund {t}"));
            }
        }
    }

    // ------------------------------------------------------------ brokers

    fn found_broker(&mut self, slot: u16, kind: BrokerKind) {
        let s = slot as usize;
        if self.brokers[s].is_some() {
            return;
        }
        let id = BrokerId::new(slot, self.broker_generations[s]);
        let names: HashSet<String> = self.brokers.iter().flatten().map(|b| b.name.clone()).collect();
        let name = random_name(kind, &mut self.name_rng, &|n: &str| names.contains(n));
        let b = Broker::found(id, name, kind, self.now, &mut self.rng);
        let rate = b.decision_rate;
        if self.now > 0.0 {
            self.push_feed(format!(
                "{} founded ({}, {})",
                b.name,
                if b.is_firm() { "firm" } else { "broker" },
                b.strategy.label()
            ));
        }
        self.brokers[s] = Some(b);
        self.stats.brokers_founded += 1;
        let dt = self.rng.exponential(rate);
        self.queue.push(self.now + dt, Ev::BrokerDecide(id));
    }

    fn portfolio_value(&self, port: &crate::broker::Portfolio) -> f64 {
        port.cash
            + port.positions.iter().map(|(id, p)| self.security(*id).map_or(0.0, |s| s.price * p.shares)).sum::<f64>()
    }

    fn on_broker_decide(&mut self, id: BrokerId) {
        self.stats.broker_decisions += 1;
        let Some(b) = self.broker(id) else { return };
        let equity = self.portfolio_value(&b.port);
        let (initial, rate, strategy) = (b.initial_capital, b.decision_rate, b.strategy);
        if let Some(b) = self.brokers[id.slot()].as_mut() {
            b.last_equity = equity;
        }
        let peak = self.broker(id).map_or(equity, |b| b.peak_equity);
        // Broke: equity below a floor of starting capital, or clients pull
        // out after a deep drawdown from the peak.
        if equity < BROKE_FRACTION * initial || equity < 0.3 * peak {
            self.broker_goes_broke(id);
            return;
        }
        if self.any_exchange_open() && !self.live.is_empty() {
            self.broker_trade(id, strategy, equity);
        }
        let dt = self.rng.exponential(rate);
        self.queue.push(self.now + dt, Ev::BrokerDecide(id));
    }

    fn signals_for(&mut self, sid: SecurityId, estimate_noise: f64) -> Option<Signals> {
        if !self.catch_up(sid) {
            return None;
        }
        let s = self.security(sid)?;
        Some(Signals {
            mispricing: s.u + estimate_noise,
            momentum_5d: s.momentum(5),
            momentum_1d: if s.prev_close > 0.0 { (s.price / s.prev_close).ln() } else { 0.0 },
            is_fund: s.kind == SecurityKind::Fund,
            momentum_persona: s.persona.has(Perk::Momentum),
            size: s.market_cap().max(1.0).ln() - self.median_log_cap,
        })
    }

    fn broker_trade(&mut self, id: BrokerId, strategy: Strategy, equity: f64) {
        let Some(b) = self.broker(id) else { return };
        let (max_positions, stop_loss, take_profit) = (b.max_positions, b.stop_loss, b.take_profit);
        let held: Vec<(SecurityId, f64)> = b.port.positions.iter().map(|(k, p)| (*k, p.avg_cost())).collect();
        let book_full = held.len() >= max_positions;

        // 1) Manage the book: review a few holdings; sell on a negative
        //    signal, a stop-loss or a take-profit.
        let reviews = if book_full { 4 } else { 2 };
        let mut sold_one = false;
        for _ in 0..reviews.min(held.len()) {
            let (sid, avg_cost) = held[self.rng.below(held.len())];
            let noise = 0.02 * self.rng.normal();
            let Some(sig) = self.signals_for(sid, noise) else { continue };
            let price = self.security(sid).map_or(0.0, |s| s.price);
            let ret = if avg_cost > 0.0 { price / avg_cost - 1.0 } else { 0.0 };
            let sc = score(strategy, &sig, &mut self.rng);
            let exit_all = ret <= -stop_loss || ret >= take_profit;
            if exit_all || sc < 0.0 {
                let shares = self.broker(id).map_or(0.0, |b| b.port.shares_of(sid));
                let frac = if exit_all || book_full { 1.0 } else { self.rng.range(0.3, 1.0) };
                if self.execute(Trader::Broker(id), sid, Side::Sell, OrderSize::Shares(shares * frac)).is_ok() {
                    sold_one = true;
                    break;
                }
            }
        }

        // 2) Look for something to buy among a few random open listings
        //    (or add to an existing holding when the book is full).
        let mut best_buy: Option<(SecurityId, f64)> = None;
        for k in 0..6 {
            let sid = if book_full {
                if held.is_empty() || k > 1 {
                    break;
                }
                held[self.rng.below(held.len())].0
            } else {
                let slot = self.live[self.rng.below(self.live.len())];
                match self.slots[slot as usize].as_ref() {
                    Some(s) if self.exchanges[s.exchange as usize].is_open => s.id,
                    _ => continue,
                }
            };
            let noise = 0.02 * self.rng.normal();
            let Some(sig) = self.signals_for(sid, noise) else { continue };
            let sc = score(strategy, &sig, &mut self.rng);
            if sc > 0.0 && best_buy.is_none_or(|(_, b)| sc > b) {
                best_buy = Some((sid, sc));
            }
        }
        if let Some((sid, sc)) = best_buy {
            let Some(b) = self.broker(id) else { return };
            let conviction = (sc / 0.08).clamp(0.25, 1.5);
            let current = self.security(sid).map_or(0.0, |s| s.price * b.port.shares_of(sid));
            let room = (b.max_position_frac * equity - current).max(0.0);
            let adv_cap = self.security(sid).map_or(0.0, |s| 0.05 * s.daily_dollar_volume());
            // Selling first and buying in the same breath would churn fees.
            let cash_frac = if sold_one { 0.5 } else { 0.95 };
            let notional = (equity * b.trade_frac * conviction).min(room).min(adv_cap).min(b.port.cash * cash_frac);
            if notional > 1_000.0 {
                let _ = self.execute(Trader::Broker(id), sid, Side::Buy, OrderSize::Notional(notional));
            }
        }
    }

    fn broker_goes_broke(&mut self, id: BrokerId) {
        let Some(b) = self.brokers[id.slot()].take() else { return };
        self.stats.brokers_broke += 1;
        self.broker_generations[id.slot()] = self.broker_generations[id.slot()].wrapping_add(1);
        // Funds sponsored by a failed firm are wound up at NAV.
        if b.is_firm() {
            let orphaned: Vec<SecurityId> = self
                .live_ids()
                .filter(|&sid| {
                    self.security(sid).is_some_and(|s| s.kind == SecurityKind::Fund && s.sponsor == Sponsor::Firm(id))
                })
                .collect();
            for sid in orphaned {
                self.delist(sid, Exit::FundClosed);
            }
        }
        if self.focused_broker == Some(id) {
            self.focused_broker = None;
        }
        self.push_feed(format!("{} went broke after {:.0} days", b.name, (self.now - b.born_at) / SECS_PER_DAY));
        console!(Level::Info, self.now, "broker {} ({}) broke", b.name, b.strategy.label());
        // A replacement is founded a few days later: active -> broke -> new.
        let delay = self.rng.exponential(1.0 / (3.0 * SECS_PER_DAY));
        self.queue.push(self.now + delay, Ev::BrokerFound(id.slot() as u16));
    }

    // ------------------------------------------------------------ trading

    /// Execute a market order. The security is caught up first, impact
    /// follows the square-root law and lands in the mispricing term, so it
    /// fades over the security's reversion half-life.
    pub fn execute(&mut self, who: Trader, id: SecurityId, side: Side, size: OrderSize) -> Result<Fill, TradeError> {
        let sec = self.security(id).ok_or(TradeError::NoSuchSecurity)?;
        if !self.exchanges[sec.exchange as usize].is_open {
            return Err(TradeError::MarketClosed);
        }
        self.catch_up(id);
        let sec = self.security(id).ok_or(TradeError::NoSuchSecurity)?;
        let (price, adv, impact_coef, ticker) = (sec.price, sec.daily_dollar_volume(), sec.params.impact, sec.ticker);
        let port = match who {
            Trader::Player => &self.player.port,
            Trader::Broker(b) => &self.broker(b).ok_or(TradeError::NoSuchSecurity)?.port,
        };
        let mut shares = match size {
            OrderSize::Shares(n) => n,
            OrderSize::Notional(x) => x / price,
        }
        .max(0.0);
        if side == Side::Sell {
            let held = port.shares_of(id);
            if held <= 0.0 {
                return Err(TradeError::NothingToSell);
            }
            shares = shares.min(held);
        }
        let sign = if side == Side::Buy { 1.0 } else { -1.0 };
        let impact_of = |shares: f64| (impact_coef * 0.02 * (shares * price / adv).sqrt()).min(0.10);
        let mut impact = impact_of(shares);
        let mut exec = price * (sign * impact * 0.5).exp();
        if side == Side::Buy {
            let affordable = port.cash / (exec * (1.0 + FEE_RATE));
            if affordable <= 0.0 {
                return Err(TradeError::InsufficientCash);
            }
            if shares > affordable {
                shares = affordable * 0.999;
                impact = impact_of(shares);
                exec = price * (impact * 0.5).exp();
            }
        }
        if shares * price < 1.0 {
            return Err(TradeError::TooSmall);
        }
        let gross = shares * exec;
        let fee = gross * FEE_RATE;
        let port = match who {
            Trader::Player => &mut self.player.port,
            Trader::Broker(b) => &mut self.brokers[b.slot()].as_mut().ok_or(TradeError::NoSuchSecurity)?.port,
        };
        let realized = match side {
            Side::Buy => {
                port.record_buy(id, shares, gross, fee);
                0.0
            }
            Side::Sell => port.record_sell(id, shares, gross, fee),
        };
        if let Trader::Broker(b) = who
            && let Some(b) = self.brokers[b.slot()].as_mut()
        {
            b.trades += 1;
        }
        let sec = self.security_mut(id).expect("checked");
        sec.u = (sec.u + sign * impact).clamp(-MAX_MISPRICING, MAX_MISPRICING);
        sec.reprice();
        sec.volume_today += gross;
        sec.record_tick();
        self.stats.trades += 1;
        Ok(Fill { id, ticker, side, shares, price: exec, fee, realized })
    }

    // ------------------------------------------------------------ commands

    /// Apply a UI command that targets the market itself. Pacing commands
    /// (speed, pause) belong to the runner and are ignored here.
    pub fn apply(&mut self, cmd: &Command) {
        match cmd {
            Command::Select(id) => self.selected = *id,
            Command::FocusBroker(id) => self.focused_broker = *id,
            Command::Order { id, side, size } => self.player_order(*id, *side, *size),
            Command::Liquidate(id) => {
                let held = self.player.port.shares_of(*id);
                self.player_order(*id, Side::Sell, OrderSize::Shares(held));
            }
            Command::Launch(req) => self.player_launch(req),
            _ => {}
        }
    }

    fn player_order(&mut self, id: SecurityId, side: Side, size: OrderSize) {
        match self.execute(Trader::Player, id, side, size) {
            Ok(f) => {
                if self.player.fills.len() == PLAYER_FILLS {
                    self.player.fills.pop_front();
                }
                self.player.fills.push_back(FillRecord {
                    time: self.now,
                    ticker: f.ticker,
                    side: f.side,
                    shares: f.shares,
                    price: f.price,
                    fee: f.fee,
                    realized: f.realized,
                });
                let verb = if side == Side::Buy { "Bought" } else { "Sold" };
                self.notices.push(Notice::Info(format!("{verb} {:.0} {} @ {:.2}", f.shares, f.ticker, f.price)));
                console!(Level::Debug, self.now, "player {verb} {:.2} {} @ {:.4}", f.shares, f.ticker, f.price);
            }
            Err(e) => {
                let t = self.security(id).map(|s| s.ticker.to_string()).unwrap_or_default();
                self.notices.push(Notice::Error(format!("Order for {t} rejected: {}", e.message())));
            }
        }
    }

    fn player_launch(&mut self, req: &LaunchRequest) {
        const MIN_CAPITAL: f64 = 50_000.0;
        let fail = |e: &mut Self, msg: &str| e.notices.push(Notice::Error(format!("Launch failed: {msg}")));
        if !(req.capital.is_finite() && req.capital >= MIN_CAPITAL) {
            return fail(self, "minimum capital is $50k");
        }
        let fee = req.capital * 0.01;
        if req.capital + fee > self.player.port.cash {
            return fail(self, "not enough cash (capital + 1% listing fee)");
        }
        if req.exchange as usize >= EXCHANGE_COUNT {
            return fail(self, "unknown exchange");
        }
        let kind = if req.fund { SecurityKind::Fund } else { SecurityKind::Stock };
        let ticker =
            match self.unique_ticker(kind, req.exchange, req.ticker.as_deref().filter(|s| !s.trim().is_empty())) {
                Some(t) => t,
                None => return fail(self, "ticker taken or invalid (2-5 letters)"),
            };
        let Some(id) = self.list_new(kind, req.exchange, req.persona, Some(ticker), Sponsor::Player, 0.0) else {
            self.tickers.remove(&ticker);
            return fail(self, "market is at its 65,536 listing cap");
        };
        // The player keeps a 40% founder stake bought at the offer price.
        let sec = self.slots[id.slot()].as_mut().expect("just listed");
        let price = sec.price;
        let stake_shares = req.capital / price;
        sec.shares = stake_shares / 0.4;
        self.player.port.cash -= fee;
        self.player.port.fees_paid += fee;
        self.player.port.record_buy(id, stake_shares, req.capital, 0.0);
        self.player.companies.push(id);
        self.stats.ipos += 1;
        let persona = sec.persona.letters();
        self.push_feed(format!("IPO {ticker} by you on {} at {price:.2}", EXCHANGE_SPECS[req.exchange as usize].name));
        self.notices.push(Notice::Info(format!("Listed {ticker} (persona {persona}); you own 40%")));
        console!(Level::Info, self.now, "player launched {ticker} capital={:.0}", req.capital);
    }

    // ------------------------------------------------------------ snapshot

    fn sponsor_name(&self, s: Sponsor) -> String {
        match s {
            Sponsor::Engine => "Public market".into(),
            Sponsor::Player => "You".into(),
            Sponsor::Firm(f) => self.broker(f).map_or_else(|| "Defunct firm".into(), |b| b.name.clone()),
        }
    }

    fn display_day(&self) -> i64 {
        day_index(self.now) as i64 - WARMUP_DAYS as i64 + 1
    }

    fn holdings_of(&self, port: &crate::broker::Portfolio) -> Vec<HoldingRow> {
        let mut rows: Vec<HoldingRow> = port
            .positions
            .iter()
            .filter_map(|(id, p)| {
                let s = self.security(*id)?;
                let value = s.price * p.shares;
                Some(HoldingRow {
                    id: *id,
                    ticker: s.ticker,
                    shares: p.shares,
                    avg_cost: p.avg_cost(),
                    price: s.price,
                    value,
                    unrealized: value - p.cost,
                })
            })
            .collect();
        rows.sort_by(|a, b| b.value.total_cmp(&a.value));
        rows
    }

    fn broker_row(&self, b: &Broker) -> BrokerRow {
        BrokerRow {
            id: b.id,
            name: b.name.clone(),
            kind: b.kind,
            strategy: b.strategy,
            cash: b.port.cash,
            equity: self.portfolio_value(&b.port),
            initial_capital: b.initial_capital,
            positions: b.port.positions.len(),
            trades: b.trades,
            realized_pnl: b.port.realized_pnl,
            born_day: day_index(b.born_at) as i64 - WARMUP_DAYS as i64 + 1,
            listings_sponsored: b.listings_sponsored,
        }
    }

    fn detail_for(&self, id: SecurityId) -> Option<SecurityDetail> {
        let s = self.security(id)?;
        let ex = &self.exchanges[s.exchange as usize];
        let m = &self.macro_state;
        let vol = m.regime.vol_mult();
        let fvol = ex.factor_vol(m);
        let forecast = (0..=60)
            .map(|d| {
                let h = d as f64 / 365.0;
                let (mu, sd) = s.forecast_log(h, ex.expected_factor_drift(m, h), fvol, vol);
                ForecastPoint {
                    days_ahead: d as f64,
                    mean: mu.exp(),
                    lo1: (mu - sd).exp(),
                    hi1: (mu + sd).exp(),
                    lo2: (mu - 2.0 * sd).exp(),
                    hi2: (mu + 2.0 * sd).exp(),
                }
            })
            .collect();
        let p = &s.params;
        let annual_vol =
            (p.beta().powi(2) * fvol.powi(2) + (p.sigma_f * vol).powi(2) + (p.sigma_u * vol).powi(2)).sqrt();
        let pos = self.player.port.positions.get(&id);
        Some(SecurityDetail {
            id,
            ticker: s.ticker,
            kind: s.kind,
            exchange: s.exchange,
            persona: s.persona,
            sponsor: self.sponsor_name(s.sponsor),
            listed_day: day_index(s.listed_at) as i64 - WARMUP_DAYS as i64 + 1,
            price: s.price,
            prev_close: s.prev_close,
            day_open: s.day_open,
            day_high: s.day_high,
            day_low: s.day_low,
            volume_today: s.volume_today,
            fair_value: s.fair_value(),
            mispricing: s.u,
            shares: s.shares,
            market_cap: s.market_cap(),
            beta: p.beta(),
            duration: p.exposures[1],
            half_life_days: (2f64.ln() / p.kappa) * 365.0,
            annual_vol,
            distance_to_default: s.distance_to_default(),
            closes: s.closes.iter().copied().collect(),
            intraday: s.intraday.iter().copied().collect(),
            forecast,
            player_shares: pos.map_or(0.0, |p| p.shares),
            player_avg_cost: pos.map_or(0.0, |p| p.avg_cost()),
        })
    }

    /// Build an owned snapshot for the UI. Runner-owned fields (speed, lag,
    /// throughput) are filled with defaults and overwritten by the runner.
    pub fn snapshot(&mut self) -> Snapshot {
        let rows: Vec<Row> = self
            .live
            .iter()
            .filter_map(|&slot| self.slots[slot as usize].as_ref())
            .map(|s| Row {
                id: s.id,
                ticker: s.ticker,
                exchange: s.exchange,
                kind: s.kind,
                persona: s.persona,
                price: s.price,
                change_pct: if s.prev_close > 0.0 { ((s.price / s.prev_close - 1.0) * 100.0) as f32 } else { 0.0 },
                market_cap: s.market_cap(),
            })
            .collect();

        let m = &self.macro_state;
        let macro_view = MacroView {
            policy_rate: m.policy_rate,
            inflation: m.inflation,
            output_gap: m.output_gap,
            term_premium: m.term_premium,
            erp: m.erp,
            credit_spread: m.credit_spread,
            money_supply: m.money_supply(),
            fair_pe: m.fair_pe(),
            taylor_target: m.taylor_target(),
            stress: m.stress(),
            curve: m.curve.points(),
            forward_path: (0..=20).map(|q| (q as f64 * 0.25, m.curve.forward_at(q as f64 * 0.25))).collect(),
            spread_10y_3m: m.curve.spread_10y_3m(),
        };
        let mut members = [0usize; EXCHANGE_COUNT];
        for r in &rows {
            members[r.exchange as usize] += 1;
        }
        let exchanges = self
            .exchanges
            .iter()
            .map(|e| ExchangeView {
                name: e.spec.name,
                is_open: e.is_open,
                index: e.index_level(),
                change_pct: (e.index_level() / e.prev_close_index - 1.0) * 100.0,
                pe: e.pe(),
                members: members[e.id as usize],
                session: (e.spec.session.open / SECS_PER_HOUR, e.spec.session.close / SECS_PER_HOUR),
            })
            .collect();

        let brokers: Vec<BrokerRow> = self.brokers.iter().flatten().map(|b| self.broker_row(b)).collect();
        let broker_detail = self.focused_broker.and_then(|id| self.broker(id)).map(|b| BrokerDetail {
            row: self.broker_row(b),
            holdings: self.holdings_of(&b.port),
            fees_paid: b.port.fees_paid,
            interest_earned: b.port.interest_earned,
        });

        let holdings = self.holdings_of(&self.player.port);
        let unrealized = holdings.iter().map(|h| h.unrealized).sum();
        let pp = &self.player.port;
        let player = PlayerView {
            cash: pp.cash,
            equity: self.portfolio_value(pp),
            starting_cash: self.player.starting_cash,
            realized_pnl: pp.realized_pnl,
            unrealized_pnl: unrealized,
            fees_paid: pp.fees_paid,
            interest_earned: pp.interest_earned,
            holdings,
            fills: self.player.fills.iter().rev().cloned().collect(),
            companies: self
                .player
                .companies
                .iter()
                .filter_map(|&id| self.security(id).map(|s| (id, s.ticker)))
                .collect(),
        };
        let funds = rows.iter().filter(|r| r.kind == SecurityKind::Fund).count();
        let population = PopulationView {
            live: rows.len(),
            stocks: rows.len() - funds,
            funds,
            carrying_capacity: self.carrying_capacity,
            ipos: self.stats.ipos,
            bankruptcies: self.stats.bankruptcies,
            penny_delistings: self.stats.penny_delistings,
            acquisitions: self.stats.acquisitions,
            fund_closures: self.stats.fund_closures,
            splits: self.stats.splits,
            brokers_alive: self.brokers_alive(),
            brokers_broke: self.stats.brokers_broke,
            repairs: self.stats.repairs,
        };
        Snapshot {
            time: self.now,
            day: self.display_day(),
            weekday: weekday_name(self.now),
            clock: clock_string(self.now),
            seed: self.seed,
            speed: 1.0,
            paused: false,
            closed_boost_active: false,
            lag_secs: 0.0,
            events_per_real_sec: 0.0,
            tick_density: self.tick_density,
            regime: self.macro_state.regime,
            regime_days: self.macro_state.regime_days,
            macro_view,
            exchanges,
            detail: self.selected.and_then(|id| self.detail_for(id)),
            rows,
            brokers,
            broker_detail,
            player,
            population,
            feed: self.feed.iter().rev().take(60).cloned().collect(),
            notices: self.drain_notices(),
        }
    }

    /// Check every invariant a long run must keep; used by tests and by the
    /// runner's periodic self-check. Returns a description of the first
    /// violation, if any.
    pub fn health_check(&self) -> Result<(), String> {
        for &slot in &self.live {
            let s = self.slots[slot as usize].as_ref().ok_or("live slot is empty")?;
            if !(s.price.is_finite() && s.price > 0.0 && s.log_price.is_finite() && s.u.is_finite() && s.y.is_finite())
            {
                return Err(format!("{} has non-finite state (price {})", s.ticker, s.price));
            }
        }
        for b in self.brokers.iter().flatten() {
            if !b.port.cash.is_finite() {
                return Err(format!("broker {} has non-finite cash", b.name));
            }
            if b.port.positions.values().any(|p| !p.shares.is_finite() || p.shares < 0.0) {
                return Err(format!("broker {} has a bad position", b.name));
            }
        }
        let m = &self.macro_state;
        for v in [m.policy_rate, m.inflation, m.output_gap, m.y10(), m.erp] {
            if !v.is_finite() {
                return Err("macro state non-finite".into());
            }
        }
        for e in &self.exchanges {
            if !e.index_level().is_finite() {
                return Err(format!("{} index non-finite", e.spec.name));
            }
        }
        if self.live.len() > MAX_SECURITIES {
            return Err("security cap exceeded".into());
        }
        Ok(())
    }

    /// Regime currently in force (shortcut for the runner/tests).
    pub fn regime(&self) -> Regime {
        self.macro_state.regime
    }

    /// Number of tenors on the curve (re-exported for UIs).
    pub const TENOR_COUNT: usize = TENORS.len();
}

impl Side {
    pub fn opposite(self) -> Side {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }
}

/// Compact money formatting shared by engine messages ($1.23M etc).
pub fn fmt_money(v: f64) -> String {
    let a = v.abs();
    let (n, suffix) = if a >= 1e15 {
        (v / 1e15, "Q")
    } else if a >= 1e12 {
        (v / 1e12, "T")
    } else if a >= 1e9 {
        (v / 1e9, "B")
    } else if a >= 1e6 {
        (v / 1e6, "M")
    } else if a >= 1e4 {
        (v / 1e3, "k")
    } else {
        (v, "")
    };
    format!("${n:.2}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small(seed: u64) -> Engine {
        Engine::new(EngineConfig { seed, initial_securities: 1_500, brokers: 64, warmup: false, ..Default::default() })
    }

    #[test]
    fn same_seed_same_market_regardless_of_chunking() {
        let mut a = small(9);
        let mut b = small(9);
        a.advance_to(3.0 * SECS_PER_DAY, None);
        // Same horizon in many uneven chunks.
        let mut t = 0.0;
        while t < 3.0 * SECS_PER_DAY {
            t += 137.0;
            b.advance_to(t.min(3.0 * SECS_PER_DAY), None);
        }
        let pa: Vec<f64> = a.live_ids().map(|id| a.security(id).unwrap().price).collect();
        let pb: Vec<f64> = b.live_ids().map(|id| b.security(id).unwrap().price).collect();
        assert_eq!(pa, pb);
    }

    #[test]
    fn stale_ids_do_not_resolve_after_delisting() {
        let mut e = small(3);
        let id = e.live_ids().next().unwrap();
        e.delist(id, Exit::Acquired);
        assert!(e.security(id).is_none());
        let new = e.list_new(SecurityKind::Stock, 0, None, None, Sponsor::Engine, 0.0).unwrap();
        // Slot is reused, generation differs, old id stays dead.
        assert_eq!(new.slot(), id.slot());
        assert_ne!(new, id);
        assert!(e.security(id).is_none());
    }

    #[test]
    fn player_can_trade_when_open_and_not_when_closed() {
        let mut e = small(4);
        // Monday 14:00: EX01 is open (13:30-20:00).
        e.advance_to(14.0 * SECS_PER_HOUR, None);
        let id = e.live_ids().find(|&id| e.security(id).unwrap().exchange == 0).unwrap();
        let cash0 = e.player.port.cash;
        let fill = e.execute(Trader::Player, id, Side::Buy, OrderSize::Notional(10_000.0)).unwrap();
        assert!(fill.shares > 0.0);
        assert!(e.player.port.cash < cash0);
        // Saturday: everything closed.
        e.advance_to(5.5 * SECS_PER_DAY, None);
        if e.security(id).is_some() {
            assert_eq!(
                e.execute(Trader::Player, id, Side::Sell, OrderSize::Shares(1.0)).unwrap_err(),
                TradeError::MarketClosed
            );
        }
    }

    #[test]
    fn delisting_pays_holders_and_removes_position() {
        let mut e = small(5);
        e.advance_to(14.0 * SECS_PER_HOUR, None);
        let id = e.live_ids().find(|&id| e.security(id).unwrap().exchange == 0).unwrap();
        e.execute(Trader::Player, id, Side::Buy, OrderSize::Notional(50_000.0)).unwrap();
        let cash_before = e.player.port.cash;
        e.delist(id, Exit::Acquired);
        assert_eq!(e.player.port.shares_of(id), 0.0);
        assert!(e.player.port.cash > cash_before + 50_000.0, "acquisition premium paid");
    }

    #[test]
    fn launch_company_creates_listing_owned_by_player() {
        let mut e = small(6);
        e.apply(&Command::Launch(LaunchRequest {
            capital: 200_000.0,
            exchange: 2,
            persona: Some(Persona::parse("BLO")),
            ticker: Some("ZZTOP".into()),
            fund: false,
        }));
        let (id, t) = (e.player.companies[0], Ticker::new("ZZTOP"));
        let s = e.security(id).unwrap();
        assert_eq!(s.ticker, t);
        assert_eq!(s.persona.letters(), "BLO");
        assert!((e.player.port.shares_of(id) / s.shares - 0.4).abs() < 1e-9);
    }
}
