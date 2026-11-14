//! The path-B engine: budgeted random sampling with exact lazy catch-up.
//!
//! ## One tick, in order
//! 1. **Global step.** Advance the clock, the macro economy and the 12
//!    sentiment factors. These are a few dozen numbers, so they are stepped
//!    every tick in sub-steps that never cross a session boundary.
//! 2. **Forced updates.** The selected stock, the player's holdings and
//!    stocks with resting orders are always caught up (a handful).
//! 3. **Sweep.** A round-robin cursor touches just enough slots that no
//!    security is ever older than `max_staleness` simulated seconds. This
//!    bounds the age of the oldest quote and guarantees daily closes are
//!    recorded.
//! 4. **Random sample.** Draw `budget` slots from the Fenwick sampler,
//!    weighted by volatility and attention, and catch each one up.
//! 5. **Brokers.** A few brokers are drawn to act.
//! 6. **Life cycle.** IPOs arrive at a rate set by a population controller.
//!
//! The cost of a tick is O(budget * log n), *independent of how many
//! securities exist*. 8,192 or 65,536 live securities cost the same per tick;
//! more securities just means each is visited less often, which (thanks to
//! exact catch-up) changes nothing about where prices go.

use crate::broker::{Broker, Intent, View};
use crate::clock::{
    ClockParts, SECONDS_PER_DAY, SessionMode, START_T, variance_rate, variance_seconds_per_year,
};
use crate::log::Level;
use crate::macro_econ::{Macro, MacroParams};
use crate::market::{EXCHANGE_NAMES, EXCHANGES, Factors, SECTORS, Sector, exchange_for_cap};
use crate::names::draw_ticker;
use crate::persona::PerkSet;
use crate::portfolio::Portfolio;
use crate::protocol::*;
use crate::rng::Rng;
use crate::sampler::{CAPACITY, WeightedSampler};
use crate::security::{Exit, FirmLink, Globals, Issuer, ListingSpec, Security, SecurityId, Ticker};
use crate::slog;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;

/// Tunables. `Config::default()` is the real game; `Config::small()` is a
/// scaled-down market for fast tests.
#[derive(Clone, Debug)]
pub struct Config {
    pub seed: u64,
    pub target_population: usize,
    pub band: (usize, usize),
    pub initial_population: usize,
    pub brokers: usize,
    pub listed_broker_share: f64,
    pub session: SessionMode,
    /// No security is left un-updated longer than this (sim seconds).
    pub max_staleness: f64,
    /// Macro sub-step (sim seconds).
    pub macro_step: f64,
    /// Average sim seconds between a broker's decisions while open.
    pub broker_interval: f64,
    pub player_cash: f64,
    /// Simulated days to run before the first frame, so charts have history.
    pub warmup_days: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            seed: 0x5167_4D41,
            target_population: 1 << 14, // 16,384
            band: (1 << 13, 1 << 15),   // 8,192 .. 32,768
            initial_population: 15_000,
            brokers: 160,
            listed_broker_share: 0.25,
            session: SessionMode::Regular,
            max_staleness: 1_800.0,
            macro_step: 60.0,
            broker_interval: 600.0,
            player_cash: 1_000_000.0,
            warmup_days: 8,
        }
    }
}

impl Config {
    pub fn small(seed: u64) -> Self {
        Self {
            seed,
            target_population: 1024,
            band: (512, 2048),
            initial_population: 900,
            brokers: 24,
            warmup_days: 1,
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ExitRecord {
    cash_per_base: f64,
    cum_div_base: f64,
}

#[derive(Clone, Debug)]
struct Order {
    order_id: u64,
    id: SecurityId,
    side: Side,
    kind: OrderKind,
    trigger: f64,
    /// Buy: notional. Sell: fraction of position.
    amount: f64,
}

#[derive(Clone, Copy, Debug, Default)]
struct IndexBook {
    cap_sum: f64,
    divisor: f64,
    members: usize,
}

impl IndexBook {
    fn level(&self) -> f64 {
        if self.divisor > 0.0 { self.cap_sum / self.divisor } else { 1000.0 }
    }

    /// Add or remove a member without moving the index level: the divisor
    /// absorbs the change, exactly as real index providers do.
    fn rebalance(&mut self, cap_delta: f64, member_delta: isize) {
        let level = self.level();
        self.cap_sum = (self.cap_sum + cap_delta).max(0.0);
        self.members = (self.members as isize + member_delta).max(0) as usize;
        if self.cap_sum > 0.0 && level > 0.0 {
            self.divisor = self.cap_sum / level;
        }
    }

    /// A price move changes the level, not the divisor.
    fn mark(&mut self, cap_delta: f64) {
        self.cap_sum = (self.cap_sum + cap_delta).max(0.0);
    }
}

#[derive(Clone, Debug, Default)]
struct Counters {
    updates: u64,
    window_updates: u64,
    window_age_sum: f64,
    window_age_max: f64,
    non_finite: u64,
    ipos: u64,
    defaults: u64,
    acquisitions: u64,
    delistings: u64,
    brokers_broke: u64,
    deaths_today: u32,
    births_today: u32,
    death_ema: f64,
    birth_ema: f64,
    last_samples: usize,
    last_sweep: usize,
    last_mean_age: f64,
    last_max_age: f64,
}

#[derive(Clone, Debug, Default)]
struct Money {
    interest_paid: f64,
    dividends_paid: f64,
    fees_collected: f64,
    broker_capital_in: f64,
    broker_capital_out: f64,
    takeover_cash: f64,
    ipo_capital: f64,
}

pub struct Engine {
    pub cfg: Config,
    rng: Rng,
    life_rng: Rng,
    broker_rng: Rng,
    t: f64,
    v: f64,
    var_secs_per_year: f64,
    pub econ: Macro,
    factors: Factors,
    globals: Globals,
    close_ring: VecDeque<(f64, Globals)>,
    next_close: f64,
    day_index: u64,

    slots: Vec<Option<Security>>,
    generations: Vec<u16>,
    free: Vec<u16>,
    live: usize,
    sampler: WeightedSampler,
    sweep_cursor: usize,
    tickers: HashSet<Ticker>,
    exits: HashMap<SecurityId, ExitRecord>,

    brokers: Vec<Broker>,
    broker_respawn: Vec<f64>,

    player: Portfolio,
    orders: Vec<Order>,
    next_order_id: u64,
    fills: VecDeque<FillView>,
    // Ordered sets: iteration order feeds the RNG, so it must be stable.
    focus: BTreeSet<usize>,
    watch: BTreeSet<usize>,
    selected: Option<SecurityId>,
    intraday: VecDeque<f32>,
    intraday_t: f64,

    indices: [IndexBook; EXCHANGES + 1],
    index_open: [f64; EXCHANGES + 1],
    index_history: [VecDeque<f32>; EXCHANGES + 1],
    macro_history: [VecDeque<f32>; 4],
    composite_live: VecDeque<f32>,
    composite_live_t: f64,
    curve_history: VecDeque<[f64; 8]>,

    ipo_accum: f64,
    counters: Counters,
    money: Money,
    notices: Vec<String>,
    table_version: u64,
    table: Option<Arc<Vec<Row>>>,
    crawl: Vec<Vec<CrawlCell>>,
    breadth: (usize, usize),
}

const HISTORY_CAP: usize = 256;
const INTRADAY_CAP: usize = 800;
const FILLS_CAP: usize = 64;

impl Engine {
    pub fn new(cfg: Config) -> Self {
        let mut root = Rng::seeded(cfg.seed);
        let rng = root.fork(1);
        let mut life_rng = root.fork(2);
        let broker_rng = root.fork(3);
        let mut econ = Macro::new(MacroParams::default());
        // Give the economy a lived-in starting point: run it for two years.
        for _ in 0..730 {
            econ.step(SECONDS_PER_DAY, &mut life_rng);
        }
        econ.years = 0.0;
        econ.log_cpi = 0.0;
        let var_secs_per_year = variance_seconds_per_year(cfg.session);
        let mut e = Self {
            rng,
            life_rng,
            broker_rng,
            t: START_T,
            v: 0.0,
            var_secs_per_year,
            econ,
            factors: Factors::new(),
            globals: Globals::default(),
            close_ring: VecDeque::new(),
            next_close: cfg.session.next_close(START_T),
            day_index: 0,
            slots: Vec::new(),
            generations: vec![0; CAPACITY],
            free: Vec::new(),
            live: 0,
            sampler: WeightedSampler::new(),
            sweep_cursor: 0,
            tickers: HashSet::new(),
            exits: HashMap::new(),
            brokers: Vec::new(),
            broker_respawn: Vec::new(),
            player: Portfolio::new(cfg.player_cash, START_T),
            orders: Vec::new(),
            next_order_id: 1,
            fills: VecDeque::new(),
            focus: BTreeSet::new(),
            watch: BTreeSet::new(),
            selected: None,
            intraday: VecDeque::new(),
            intraday_t: 0.0,
            indices: [IndexBook::default(); EXCHANGES + 1],
            index_open: [1000.0; EXCHANGES + 1],
            index_history: Default::default(),
            macro_history: Default::default(),
            composite_live: VecDeque::new(),
            composite_live_t: 0.0,
            curve_history: VecDeque::new(),
            ipo_accum: 0.0,
            counters: Counters::default(),
            money: Money::default(),
            notices: Vec::new(),
            table_version: 0,
            table: None,
            crawl: vec![Vec::new(); EXCHANGES],
            breadth: (0, 0),
            cfg,
        };
        e.refresh_globals();
        for _ in 0..e.cfg.initial_population {
            let spec = ListingSpec::random(&mut e.life_rng);
            e.list(spec, None);
        }
        for i in 0..e.indices.len() {
            let b = &mut e.indices[i];
            b.divisor = b.cap_sum / 1000.0;
        }
        for i in 0..e.cfg.brokers {
            e.launch_broker(i);
        }
        e.counters.ipos = 0;
        e.counters.death_ema = e.baseline_deaths_per_day();
        e.counters.birth_ema = e.counters.death_ema;
        // Warm up: run a few sim days in coarse steps.
        let steps = (e.cfg.warmup_days as f64 * SECONDS_PER_DAY / 300.0) as usize;
        for _ in 0..steps {
            e.advance(300.0, 0);
        }
        e.notices.clear();
        e.fills.clear();
        e.take_window(); // do not report warm-up work as live throughput
        slog!(
            Level::Info,
            "engine",
            "seed {:#x}: {} securities, {} brokers, warm-up {} days",
            e.cfg.seed,
            e.live,
            e.brokers.len(),
            e.cfg.warmup_days
        );
        e
    }

    // ------------------------------------------------------------------
    // Accessors
    // ------------------------------------------------------------------

    pub fn time(&self) -> f64 {
        self.t
    }

    pub fn live(&self) -> usize {
        self.live
    }

    pub fn non_finite_events(&self) -> u64 {
        self.counters.non_finite
    }

    pub fn is_open(&self) -> bool {
        self.cfg.session.is_open(self.t)
    }

    pub fn security(&self, id: SecurityId) -> Option<&Security> {
        self.slots.get(id.slot())?.as_ref().filter(|s| s.id == id)
    }

    pub fn securities(&self) -> impl Iterator<Item = &Security> {
        self.slots.iter().flatten()
    }

    pub fn brokers(&self) -> &[Broker] {
        &self.brokers
    }

    pub fn player(&self) -> &Portfolio {
        &self.player
    }

    /// Total cash held by the player and every live broker.
    pub fn cash_total(&self) -> f64 {
        self.player.cash + self.brokers.iter().filter(|b| b.alive).map(|b| b.port.cash).sum::<f64>()
    }

    fn baseline_deaths_per_day(&self) -> f64 {
        // ~8% of the target population leaves per year.
        self.cfg.target_population as f64 * 0.08 / 365.25
    }

    fn refresh_globals(&mut self) {
        let m = &self.econ;
        let n = self.live as f64;
        let target = self.cfg.target_population as f64;
        let crowd = (n / target - 1.0).max(0.0);
        let over_band = if self.live > self.cfg.band.1 { 0.4 } else { 0.0 };
        self.globals = Globals {
            t: self.t,
            v: self.v,
            log_cpi: m.log_cpi,
            log_gdp: m.log_real_gdp(),
            gap: m.output_gap,
            expected_inflation: m.expected_inflation(),
            potential_growth: m.params.potential_growth,
            y10: m.y10(),
            erp: m.equity_risk_premium(),
            credit_spread: m.credit_spread,
            vol_mult: m.vol_multiplier(),
            s_market: self.factors.market,
            s_sector: self.factors.sector,
            crowding_hazard: 0.12 * crowd * crowd + over_band,
        };
    }

    // ------------------------------------------------------------------
    // The tick
    // ------------------------------------------------------------------

    /// Advance by `dt` simulated seconds with a budget of `samples` random
    /// updates. Returns the number of security updates performed.
    pub fn advance(&mut self, dt: f64, samples: usize) -> usize {
        if dt <= 0.0 {
            return 0;
        }
        // 1. Global step, split at session boundaries and daily closes.
        let mut remaining = dt;
        while remaining > 1e-9 {
            let boundary = self.cfg.session.next_boundary(self.t);
            let step = remaining
                .min(self.cfg.macro_step)
                .min(boundary - self.t)
                .min(self.next_close - self.t)
                .max(1e-6);
            let rate = variance_rate(self.cfg.session, self.t + 0.5 * step);
            let dv = rate * step / self.var_secs_per_year;
            self.econ.step(step, &mut self.life_rng);
            self.factors.step(dv, self.econ.vol_multiplier(), self.econ.risk_appetite, &mut self.life_rng);
            self.t += step;
            self.v += dv;
            remaining -= step;
            if self.t >= self.next_close - 1e-6 {
                self.t = self.t.max(self.next_close);
                self.refresh_globals();
                self.on_close();
                self.next_close = self.cfg.session.next_close(self.t);
            }
        }
        self.refresh_globals();
        let mut updated = 0;

        // 2. Forced updates.
        let mut forced: Vec<usize> = self.focus.iter().copied().collect();
        if let Some(id) = self.selected {
            forced.push(id.slot());
        }
        for slot in forced {
            updated += self.update_slot(slot) as usize;
        }

        // 3. Sweep: enough slots that each is visited every max_staleness.
        let span = self.slots.len();
        let sweep = if span == 0 {
            0
        } else {
            ((span as f64 * dt / self.cfg.max_staleness).ceil() as usize).min(span)
        };
        for _ in 0..sweep {
            self.sweep_cursor = (self.sweep_cursor + 1) % span;
            updated += self.update_slot(self.sweep_cursor) as usize;
        }

        // 4. Weighted random sample.
        let mut drawn = 0;
        for _ in 0..samples {
            let u = self.rng.uniform();
            if let Some(slot) = self.sampler.sample(u) {
                updated += self.update_slot(slot) as usize;
                drawn += 1;
            }
        }
        self.counters.last_samples = drawn;
        self.counters.last_sweep = sweep;

        // 5. Brokers act while the market is open.
        if self.is_open() {
            let expected = self.brokers.len() as f64 * dt / self.cfg.broker_interval;
            let n = self.broker_rng.poisson(expected) as usize;
            for _ in 0..n {
                let b = self.broker_rng.below(self.brokers.len().max(1) as u64) as usize;
                self.broker_act(b);
            }
        }
        self.respawn_brokers();

        // 6. IPOs.
        self.lifecycle(dt);

        self.counters.updates += updated as u64;
        self.counters.window_updates += updated as u64;
        updated
    }

    /// Bring one slot up to date. Returns true if something was updated.
    fn update_slot(&mut self, slot: usize) -> bool {
        let Some(mut sec) = self.slots.get_mut(slot).and_then(Option::take) else {
            return false;
        };
        let age = self.t - sec.t_last;
        if age <= 0.0 {
            self.slots[slot] = Some(sec);
            return false;
        }
        let link = self.firm_link(&sec);
        let old_cap = sec.market_cap();
        let old_exchange = sec.exchange;

        // Cross any daily closes in the gap, using the market as it stood
        // at each close, so the recorded close is the true close.
        let mut exit = None;
        for i in 0..self.close_ring.len() {
            let (close_t, g) = self.close_ring[i];
            if close_t > sec.t_last && close_t > sec.last_close_t && exit.is_none() {
                exit = sec.advance(&g, link, &mut self.rng);
                if exit.is_none() {
                    exit = sec.record_close(close_t);
                    if self.day_index % 21 == 0 {
                        // Monthly listing review, with 20% hysteresis.
                        let proper = exchange_for_cap(sec.market_cap());
                        let cap = sec.market_cap();
                        let ex = sec.exchange as usize;
                        let lo = crate::market::EXCHANGE_MIN_CAP[ex] * 0.8;
                        let hi = crate::market::EXCHANGE_MIN_CAP.get(ex + 1).map_or(f64::MAX, |c| c * 1.2);
                        if cap < lo || cap > hi {
                            sec.exchange = proper;
                        }
                    }
                }
            }
        }
        if exit.is_none() {
            exit = sec.advance(&self.globals, link, &mut self.rng);
        }

        self.counters.window_age_sum += age;
        self.counters.window_age_max = self.counters.window_age_max.max(age);

        if !sec.is_finite() {
            self.counters.non_finite += 1;
            slog!(Level::Warn, "engine", "non-finite state in {} (slot {slot}); delisting", sec.ticker.as_str());
            exit = Some(Exit::Delisted { price: sec.price.max(0.0) });
        }

        // Index bookkeeping.
        let new_cap = sec.market_cap();
        if sec.exchange != old_exchange {
            self.indices[old_exchange as usize].rebalance(-old_cap, -1);
            self.indices[sec.exchange as usize].rebalance(new_cap, 1);
        } else {
            self.indices[sec.exchange as usize].mark(new_cap - old_cap);
        }
        self.indices[EXCHANGES].mark(new_cap - old_cap);

        if slot_is_selected(self.selected, &sec) && self.t - self.intraday_t >= 30.0 {
            self.intraday_t = self.t;
            if self.intraday.len() == INTRADAY_CAP {
                self.intraday.pop_front();
            }
            self.intraday.push_back(sec.price as f32);
        }

        let weight = self.weight_for(slot, &sec);
        let id = sec.id;
        self.slots[slot] = Some(sec);
        if let Some(exit) = exit {
            self.retire(slot, exit);
        } else {
            self.sampler.set(slot, weight);
            self.check_orders(id);
        }
        true
    }

    fn weight_for(&self, slot: usize, sec: &Security) -> f64 {
        let mut w = sec.base_weight(self.globals.vol_mult);
        if self.focus.contains(&slot) {
            w *= 20.0;
        } else if self.watch.contains(&slot) {
            w *= 8.0;
        }
        w
    }

    fn firm_link(&self, sec: &Security) -> Option<FirmLink> {
        match sec.issuer {
            Issuer::BrokerFirm(b) => self.brokers.get(b as usize).filter(|b| b.alive).map(|b| FirmLink {
                equity: b.equity.max(1.0),
                roe: b.roe,
            }),
            _ => None,
        }
    }

    /// Daily close bookkeeping (runs once per trading day at the close).
    fn on_close(&mut self) {
        self.day_index += 1;
        self.close_ring.push_back((self.t, self.globals));
        while self.close_ring.len() > 4 {
            self.close_ring.pop_front();
        }
        // Population controller memory.
        let d = self.counters.deaths_today as f64;
        let b = self.counters.births_today as f64;
        self.counters.death_ema = 0.93 * self.counters.death_ema + 0.07 * d;
        self.counters.birth_ema = 0.93 * self.counters.birth_ema + 0.07 * b;
        self.counters.deaths_today = 0;
        self.counters.births_today = 0;

        // Settle every broker against securities that left the market,
        // then forget the exits.
        for b in 0..self.brokers.len() {
            self.settle_broker_exits(b);
        }
        self.exits.clear();

        // Clean accumulated floating-point drift.
        self.sampler.rebuild();
        self.rebuild_index_sums();

        // Daily histories.
        for (i, book) in self.indices.iter().enumerate() {
            push_capped(&mut self.index_history[i], book.level() as f32, HISTORY_CAP);
            self.index_open[i] = book.level();
        }
        let m = &self.econ;
        let vals = [m.y10(), m.y3m(), m.policy_rate, m.inflation];
        for (h, v) in self.macro_history.iter_mut().zip(vals) {
            push_capped(h, (v * 100.0) as f32, HISTORY_CAP);
        }
        if self.curve_history.len() == 32 {
            self.curve_history.pop_front();
        }
        self.curve_history.push_back(self.econ.curve.points());

        let c = ClockParts::from_t(self.t);
        slog!(
            Level::Debug,
            "engine",
            "close day {} ({}): live {} ipo/day {:.1} exits/day {:.1} composite {:.1}",
            c.day,
            c.weekday_name(),
            self.live,
            self.counters.birth_ema,
            self.counters.death_ema,
            self.indices[EXCHANGES].level()
        );
    }

    fn rebuild_index_sums(&mut self) {
        let mut sums = [(0.0f64, 0usize); EXCHANGES + 1];
        for s in self.slots.iter().flatten() {
            let cap = s.market_cap();
            sums[s.exchange as usize].0 += cap;
            sums[s.exchange as usize].1 += 1;
            sums[EXCHANGES].0 += cap;
            sums[EXCHANGES].1 += 1;
        }
        for (book, (sum, n)) in self.indices.iter_mut().zip(sums) {
            let level = book.level();
            book.cap_sum = sum;
            book.members = n;
            if sum > 0.0 && level > 0.0 {
                book.divisor = sum / level;
            }
        }
    }

    // ------------------------------------------------------------------
    // Life cycle
    // ------------------------------------------------------------------

    /// Births per day from the population controller:
    /// `exits_per_day * exp(2 * (1 - N/target)) * ipo_window`.
    /// At the target, births replace exits. Below it IPOs boom, above it
    /// they dry up, and sentiment opens and shuts the IPO window so the
    /// population wanders inside the band rather than sitting still.
    pub fn births_per_day(&self) -> f64 {
        let n = self.live as f64;
        let target = self.cfg.target_population as f64;
        let base = self.counters.death_ema.max(0.25 * self.baseline_deaths_per_day());
        let pressure = (2.0 * (1.0 - n / target)).exp();
        let window = 0.5 + 1.0 / (1.0 + (-(self.econ.risk_appetite + 8.0 * self.factors.market)).exp());
        // Below the band, a rescue term refills the gap within ~30 days.
        let rescue = (self.cfg.band.0 as f64 - n).max(0.0) / 30.0;
        if self.live >= CAPACITY || self.live >= self.cfg.band.1 * 2 { 0.0 } else { base * pressure * window + rescue }
    }

    fn lifecycle(&mut self, dt: f64) {
        self.ipo_accum += self.births_per_day() * dt / SECONDS_PER_DAY;
        if !self.is_open() {
            return; // IPOs price during market hours; demand carries over.
        }
        let n = self.ipo_accum.floor() as usize;
        self.ipo_accum -= n as f64;
        for _ in 0..n.min(64) {
            let spec = ListingSpec::random(&mut self.life_rng);
            if let Some(id) = self.list(spec, None) {
                self.counters.ipos += 1;
                self.counters.births_today += 1;
                if let Some(s) = self.security(id) {
                    slog!(
                        Level::Trace,
                        "life",
                        "IPO {} {} at {:.2} [{}]",
                        s.ticker.as_str(),
                        s.sector.name(),
                        s.price,
                        s.perks.letters()
                    );
                }
            }
        }
    }

    fn list(&mut self, spec: ListingSpec, link: Option<FirmLink>) -> Option<SecurityId> {
        let slot = match self.free.pop() {
            Some(s) => s as usize,
            None if self.slots.len() < CAPACITY => {
                self.slots.push(None);
                self.slots.len() - 1
            }
            None => return None,
        };
        let id = SecurityId::new(slot as u16, self.generations[slot]);
        let ticker = draw_ticker(&mut self.life_rng, &mut self.tickers);
        let sec = Security::list(id, ticker, spec, &self.globals, link, &mut self.life_rng);
        let cap = sec.market_cap();
        self.indices[sec.exchange as usize].rebalance(cap, 1);
        self.indices[EXCHANGES].rebalance(cap, 1);
        let w = sec.base_weight(self.globals.vol_mult);
        self.slots[slot] = Some(sec);
        self.sampler.set(slot, w);
        self.live += 1;
        Some(id)
    }

    fn retire(&mut self, slot: usize, exit: Exit) {
        let Some(sec) = self.slots[slot].take() else { return };
        let cap = sec.market_cap();
        self.indices[sec.exchange as usize].rebalance(-cap, -1);
        self.indices[EXCHANGES].rebalance(-cap, -1);
        self.sampler.set(slot, 0.0);
        self.tickers.remove(&sec.ticker);
        self.generations[slot] = self.generations[slot].wrapping_add(1);
        self.free.push(slot as u16);
        self.live -= 1;
        self.counters.deaths_today += 1;

        let (cash_per_base, what) = match exit {
            Exit::Defaulted => {
                self.counters.defaults += 1;
                (0.0, "defaulted")
            }
            Exit::Acquired { price } => {
                self.counters.acquisitions += 1;
                (price * sec.split_factor, "was acquired")
            }
            // Delisted shares move to the over-the-counter market, where
            // they trade at a deep discount. We cash holders out at half.
            Exit::Delisted { price } => {
                self.counters.delistings += 1;
                (0.5 * price * sec.split_factor, "was delisted")
            }
        };
        slog!(Level::Trace, "life", "{} {what} at {:.2}", sec.ticker.as_str(), sec.price);
        let rec = ExitRecord { cash_per_base, cum_div_base: sec.cum_div_base };
        self.exits.insert(sec.id, rec);
        if let Exit::Acquired { .. } = exit {
            self.money.takeover_cash += cash_per_base * sec.shares / sec.split_factor;
        }
        if self.player.positions.contains_key(&sec.id) {
            let paid = self.player.resolve_exit(sec.id, rec.cash_per_base, rec.cum_div_base);
            self.notice(format!("{} {what}: you received {}", sec.ticker.as_str(), money_str(paid)));
        }
        self.orders.retain(|o| o.id != sec.id);
        self.focus.remove(&slot);
        if self.selected == Some(sec.id) {
            self.selected = None;
        }
        if let Issuer::BrokerFirm(b) = sec.issuer
            && let Some(br) = self.brokers.get_mut(b as usize)
            && br.listed == Some(sec.id)
        {
            br.listed = None;
        }
        self.refresh_focus();
    }

    // ------------------------------------------------------------------
    // Brokers
    // ------------------------------------------------------------------

    fn launch_broker(&mut self, seat: usize) {
        let mut b = Broker::launch(seat as u16, self.t, &mut self.broker_rng);
        self.money.broker_capital_in += b.capital_in;
        let listed = self.broker_rng.chance(self.cfg.listed_broker_share);
        if seat < self.brokers.len() {
            self.brokers[seat] = b.clone();
        } else {
            self.brokers.push(b.clone());
            self.broker_respawn.push(f64::INFINITY);
        }
        if listed {
            let perks = PerkSet::draw(&mut self.broker_rng, Sector::Financials.affinity());
            let spec = ListingSpec {
                sector: Sector::Financials,
                perks,
                issuer: Issuer::BrokerFirm(seat as u16),
                price: self.broker_rng.lognormal(30.0, 0.5).clamp(5.0, 200.0),
                market_cap: b.capital_in,
            };
            let link = FirmLink { equity: b.capital_in, roe: b.roe };
            b.listed = self.list(spec, Some(link));
            self.brokers[seat].listed = b.listed;
        }
    }

    fn respawn_brokers(&mut self) {
        for seat in 0..self.brokers.len() {
            if !self.brokers[seat].alive && self.t >= self.broker_respawn[seat] {
                self.broker_respawn[seat] = f64::INFINITY;
                self.launch_broker(seat);
                let b = &self.brokers[seat];
                slog!(Level::Debug, "broker", "new broker {} ({}) seat {seat}", b.name, b.strategy.name());
            }
        }
    }

    fn price_base(&self, id: SecurityId) -> Option<f64> {
        self.security(id).map(|s| s.price * s.split_factor)
    }

    fn view(&self, s: &Security) -> View {
        let div_yield = if s.price > 0.0 { s.payout * s.eps(&self.globals) / s.price } else { 0.0 };
        View {
            id: s.id,
            price_base: s.price * s.split_factor,
            mispricing: (s.price / s.fair).ln(),
            change_pct: s.change_pct(),
            vol: s.equity_vol(self.globals.vol_mult),
            market_cap: s.market_cap(),
            div_yield: if s.issuer == Issuer::Company { div_yield } else { 0.0 },
            perks: s.perks,
        }
    }

    fn settle_broker_exits(&mut self, b: usize) {
        let dead: Vec<SecurityId> = self.brokers[b]
            .port
            .positions
            .keys()
            .copied()
            .filter(|id| self.security(*id).is_none())
            .collect();
        for id in dead {
            let rec = self.exits.get(&id).copied().unwrap_or(ExitRecord { cash_per_base: 0.0, cum_div_base: 0.0 });
            let port = &mut self.brokers[b].port;
            let before = port.dividends;
            port.resolve_exit(id, rec.cash_per_base, rec.cum_div_base);
            self.money.dividends_paid += port.dividends - before;
        }
    }

    fn broker_act(&mut self, b: usize) {
        if !self.brokers[b].alive {
            return;
        }
        self.settle_broker_exits(b);
        // Interest and dividends.
        let rate = self.econ.policy_rate;
        {
            let t = self.t;
            let port = &mut self.brokers[b].port;
            let before = port.interest;
            port.accrue_interest(t, rate);
            self.money.interest_paid += port.interest - before;
        }
        let ids: Vec<SecurityId> = self.brokers[b].port.positions.keys().copied().collect();
        for id in &ids {
            if let Some(cd) = self.security(*id).map(|s| s.cum_div_base) {
                let paid = self.brokers[b].port.settle_dividends(*id, cd);
                self.money.dividends_paid += paid;
            }
        }
        // Mark to market and check for bankruptcy.
        let equity = self.brokers[b].port.equity(|id| self.price_base(id));
        let t = self.t;
        self.brokers[b].mark(equity, t);
        if self.brokers[b].is_broke() {
            self.bankrupt(b);
            return;
        }
        // Decide.
        let held: Vec<(View, f64, f64)> = ids
            .iter()
            .filter_map(|id| {
                let s = self.security(*id)?;
                let p = self.brokers[b].port.positions.get(id)?;
                Some((self.view(s), p.base_shares, p.cost / p.base_shares.max(1e-12)))
            })
            .collect();
        let mut candidates = Vec::with_capacity(8);
        for _ in 0..8 {
            let u = self.broker_rng.uniform();
            if let Some(slot) = self.sampler.sample(u)
                && let Some(s) = self.slots[slot].as_ref()
                && s.issuer != Issuer::BrokerFirm(b as u16)
            {
                candidates.push(self.view(s));
            }
        }
        let intents = self.brokers[b].decide(&held, &candidates, &mut self.broker_rng);
        for intent in intents {
            match intent {
                Intent::Buy { id, notional } => {
                    self.trade(id, Side::Buy, notional, Some(b));
                }
                Intent::Sell { id, base_shares } => {
                    let px = self.price_base(id).unwrap_or(0.0);
                    self.trade(id, Side::Sell, base_shares * px, Some(b));
                }
            }
        }
    }

    fn bankrupt(&mut self, b: usize) {
        let ids: Vec<SecurityId> = self.brokers[b].port.positions.keys().copied().collect();
        for id in ids {
            let shares = self.brokers[b].port.positions.get(&id).map_or(0.0, |p| p.base_shares);
            let px = self.price_base(id).unwrap_or(0.0);
            if self.is_open() {
                self.trade(id, Side::Sell, shares * px, Some(b));
            } else {
                let cd = self.security(id).map_or(0.0, |s| s.cum_div_base);
                self.brokers[b].port.sell(id, shares, px, cd);
            }
        }
        let br = &mut self.brokers[b];
        br.alive = false;
        // What is left goes to creditors and leaves the market.
        self.money.broker_capital_out += br.port.cash;
        br.port.cash = 0.0;
        br.port.positions.clear();
        self.counters.brokers_broke += 1;
        let name = br.name.clone();
        let listed = br.listed.take();
        slog!(Level::Info, "broker", "{name} went broke (seat {b})");
        self.notice(format!("Broker {name} went broke"));
        if let Some(id) = listed {
            if let Some(slot) = self.security(id).map(|s| s.id.slot()) {
                self.retire(slot, Exit::Defaulted);
            }
        }
        self.broker_respawn[b] = self.t + self.broker_rng.range(1.0, 5.0) * SECONDS_PER_DAY;
    }

    /// Execute a market order for the player (`who = None`) or a broker.
    /// Price impact follows the square-root law: impact grows with the
    /// square root of order size relative to daily volume.
    fn trade(&mut self, id: SecurityId, side: Side, notional: f64, who: Option<usize>) -> f64 {
        if !self.is_open() || !(notional > 0.0) {
            return 0.0;
        }
        let slot = id.slot();
        // Make sure we trade on a fresh price.
        self.update_slot(slot);
        let Some(sec) = self.security(id) else { return 0.0 };
        let vol = sec.equity_vol(self.globals.vol_mult);
        let daily_volume = (sec.market_cap() * 0.004 * sec.liquidity).max(1.0e4);
        let impact = (0.8 * vol / 252f64.sqrt() * (notional / daily_volume).sqrt()).min(0.05);
        let signed = if side == Side::Buy { impact } else { -impact };
        // Fill at the midpoint of the pre- and post-trade price.
        let px_base = sec.price * sec.split_factor * (0.5 * signed).exp();
        let cd = sec.cum_div_base;
        let port = match who {
            None => &mut self.player,
            Some(b) => &mut self.brokers[b].port,
        };
        let fees_before = port.fees;
        let done = match side {
            Side::Buy => port.buy(id, notional, px_base, cd) * px_base,
            Side::Sell => {
                let shares = notional / px_base;
                port.sell(id, shares, px_base, cd)
            }
        };
        self.money.fees_collected += port.fees - fees_before;
        if done <= 0.0 {
            return 0.0;
        }
        if let Some(b) = who {
            self.brokers[b].trades += 1;
        }
        let g = self.globals;
        let link = self.slots[slot].as_ref().and_then(|s| self.firm_link(s));
        if let Some(sec) = self.slots[slot].as_mut() {
            let old_cap = sec.market_cap();
            sec.m = (sec.m + signed).clamp(-1.5, 1.5);
            sec.reprice(&g, link);
            let new_cap = sec.market_cap();
            self.indices[sec.exchange as usize].mark(new_cap - old_cap);
            self.indices[EXCHANGES].mark(new_cap - old_cap);
        }
        done
    }

    // ------------------------------------------------------------------
    // Player
    // ------------------------------------------------------------------

    fn notice(&mut self, s: String) {
        if self.notices.len() < 32 {
            self.notices.push(s);
        }
    }

    fn fill_log(&mut self, text: String) {
        if self.fills.len() == FILLS_CAP {
            self.fills.pop_back();
        }
        self.fills.push_front(FillView { t: self.t, text });
    }

    fn refresh_focus(&mut self) {
        self.focus = self.player.positions.keys().map(|id| id.slot()).collect();
        for o in &self.orders {
            self.focus.insert(o.id.slot());
        }
    }

    fn player_buy(&mut self, id: SecurityId, notional: f64) -> bool {
        let Some(t) = self.security(id).map(|s| s.ticker) else { return false };
        if !self.is_open() {
            self.notice("Market is closed: place a limit order instead".into());
            return false;
        }
        let done = self.trade(id, Side::Buy, notional, None);
        if done > 0.0 {
            self.fill_log(format!("BUY  {:<5} {}", t.as_str(), money_str(done)));
        } else {
            self.notice("Buy rejected (not enough cash?)".into());
        }
        self.refresh_focus();
        done > 0.0
    }

    fn player_sell(&mut self, id: SecurityId, fraction: f64) -> bool {
        let Some(t) = self.security(id).map(|s| s.ticker) else { return false };
        if !self.is_open() {
            self.notice("Market is closed: place a limit order instead".into());
            return false;
        }
        let shares = self.player.positions.get(&id).map_or(0.0, |p| p.base_shares) * fraction.clamp(0.0, 1.0);
        let px = self.price_base(id).unwrap_or(0.0);
        let done = self.trade(id, Side::Sell, shares * px, None);
        if done > 0.0 {
            self.fill_log(format!("SELL {:<5} {}", t.as_str(), money_str(done)));
        }
        self.refresh_focus();
        done > 0.0
    }

    /// Fire any resting player orders whose trigger the price crossed.
    fn check_orders(&mut self, id: SecurityId) {
        if !self.orders.iter().any(|o| o.id == id) || !self.is_open() {
            return;
        }
        let Some(price) = self.security(id).map(|s| s.price) else { return };
        let fired: Vec<Order> = self
            .orders
            .iter()
            .filter(|o| o.id == id)
            .filter(|o| match (o.side, o.kind) {
                (Side::Buy, OrderKind::Limit) | (Side::Sell, OrderKind::Stop) => price <= o.trigger,
                (Side::Sell, OrderKind::Limit) | (Side::Buy, OrderKind::Stop) => price >= o.trigger,
            })
            .cloned()
            .collect();
        for o in fired {
            self.orders.retain(|x| x.order_id != o.order_id);
            let ok = match o.side {
                Side::Buy => self.player_buy(o.id, o.amount),
                Side::Sell => self.player_sell(o.id, o.amount),
            };
            let t = self.security(id).map(|s| s.ticker).unwrap_or_default();
            self.notice(format!(
                "Order #{} {:?} {} {} at {:.2}",
                o.order_id,
                o.kind,
                t.as_str(),
                if ok { "filled" } else { "failed" },
                price
            ));
        }
        self.refresh_focus();
    }

    fn launch_player_ipo(&mut self, sector: Sector, capital: f64, stake: f64) {
        let stake = stake.clamp(0.05, 0.9);
        let fee = capital * 0.01;
        if !(capital >= 100_000.0) || capital + fee > self.player.cash {
            self.notice("IPO needs at least 100,000 and enough cash for the 1% fee".into());
            return;
        }
        if !self.is_open() {
            self.notice("IPOs price during market hours".into());
            return;
        }
        let perks = PerkSet::draw(&mut self.life_rng, sector.affinity());
        let price = 10.0;
        let spec = ListingSpec { sector, perks, issuer: Issuer::PlayerFounded, price, market_cap: capital / stake };
        let Some(id) = self.list(spec, None) else {
            self.notice("No free listing slots (65,536 cap reached)".into());
            return;
        };
        self.counters.ipos += 1;
        // The founder's capital buys `stake` of the shares at the offer.
        let Some((cd, shares, ticker)) = self.security(id).map(|s| (s.cum_div_base, s.shares / s.split_factor, s.ticker))
        else {
            return;
        };
        self.player.cash -= capital + fee;
        self.player.fees += fee;
        self.money.fees_collected += fee;
        self.money.ipo_capital += capital;
        let pos = self.player.positions.entry(id).or_default();
        pos.base_shares += shares * stake;
        pos.cost += capital;
        pos.div_anchor = cd;
        self.fill_log(format!("IPO  {:<5} founded, {:.0}% stake", ticker.as_str(), stake * 100.0));
        self.notice(format!("{} listed [{}]", ticker.as_str(), perks.letters()));
        self.selected = Some(id);
        self.intraday.clear();
        self.refresh_focus();
    }

    /// Apply a UI command. Returns false for `Shutdown`.
    pub fn apply(&mut self, cmd: Command) {
        match cmd {
            Command::Select(id) => {
                if self.selected != id {
                    self.intraday.clear();
                    self.intraday_t = 0.0;
                }
                self.selected = id;
                if let Some(id) = id {
                    self.update_slot(id.slot());
                }
            }
            Command::Watch(ids) => {
                self.watch = ids.into_iter().take(512).map(|i| i.slot()).collect();
            }
            Command::Buy { id, notional } => {
                self.player_buy(id, notional);
            }
            Command::Sell { id, fraction } => {
                self.player_sell(id, fraction);
            }
            Command::PlaceOrder { id, side, kind, trigger, amount } => {
                if self.security(id).is_some() && trigger > 0.0 && amount > 0.0 {
                    let order_id = self.next_order_id;
                    self.next_order_id += 1;
                    self.orders.push(Order { order_id, id, side, kind, trigger, amount });
                    self.refresh_focus();
                }
            }
            Command::CancelOrder(order_id) => {
                self.orders.retain(|o| o.order_id != order_id);
                self.refresh_focus();
            }
            Command::LaunchIpo { sector, capital, stake } => self.launch_player_ipo(sector, capital, stake),
            Command::SetSession(mode) => {
                self.cfg.session = mode;
                self.var_secs_per_year = variance_seconds_per_year(mode);
                self.next_close = mode.next_close(self.t);
            }
            // Handled by the runtime loop.
            Command::SetPaused(_)
            | Command::SetSpeed(_)
            | Command::SetBudgetMs(_)
            | Command::SetLogLevel(_)
            | Command::Reset { .. }
            | Command::Shutdown => {}
        }
    }

    // ------------------------------------------------------------------
    // Frames
    // ------------------------------------------------------------------

    /// Rebuild the full table (O(n) read-only pass). The runtime calls this
    /// a few times a second, not every tick.
    pub fn rebuild_table(&mut self) {
        let mut rows = Vec::with_capacity(self.live);
        let (mut adv, mut dec) = (0, 0);
        let mut movers: Vec<Vec<(f32, CrawlCell)>> = vec![Vec::new(); EXCHANGES];
        for s in self.slots.iter().flatten() {
            let ch = s.change_pct() as f32;
            if ch > 0.0 {
                adv += 1;
            } else if ch < 0.0 {
                dec += 1;
            }
            rows.push(Row {
                id: s.id,
                ticker: s.ticker,
                exchange: s.exchange,
                sector: s.sector as u8,
                perks: s.perks,
                price: s.price as f32,
                change_pct: ch,
                market_cap: s.market_cap() as f32,
                value_gap_pct: ((s.price / s.fair - 1.0) * 100.0) as f32,
                vol: s.equity_vol(self.globals.vol_mult) as f32,
            });
            // Crawl: biggest movers weighted by size, per board.
            let score = ch.abs() * (s.market_cap() as f32).max(1.0).ln();
            movers[s.exchange as usize].push((
                score,
                CrawlCell { id: s.id, ticker: s.ticker, price: s.price as f32, change_pct: ch },
            ));
        }
        for (ex, list) in movers.iter_mut().enumerate() {
            let k = 40.min(list.len());
            if k > 0 {
                list.select_nth_unstable_by(k - 1, |a, b| b.0.total_cmp(&a.0));
                list.truncate(k);
                list.sort_by(|a, b| b.0.total_cmp(&a.0));
            }
            self.crawl[ex] = list.iter().map(|x| x.1).collect();
        }
        self.breadth = (adv, dec);
        self.table = Some(Arc::new(rows));
        self.table_version += 1;
    }

    /// Reset per-window statistics; returns (updates, mean age s, max age s).
    pub fn take_window(&mut self) -> (u64, f64, f64) {
        let c = &mut self.counters;
        let n = c.window_updates;
        let mean = if n > 0 { c.window_age_sum / n as f64 } else { 0.0 };
        let out = (n, mean, c.window_age_max);
        c.last_mean_age = mean;
        c.last_max_age = c.window_age_max;
        c.window_updates = 0;
        c.window_age_sum = 0.0;
        c.window_age_max = 0.0;
        out
    }

    pub fn frame(&mut self, include_table: bool) -> Frame {
        let m = &self.econ;
        let parts = ClockParts::from_t(self.t);
        let drivers = Drivers {
            policy_rate: m.policy_rate,
            taylor_target: m.taylor_target(),
            inflation: m.inflation,
            output_gap: m.output_gap,
            y3m: m.y3m(),
            y2: m.curve.yield_at(2.0),
            y10: m.y10(),
            term_spread: m.term_spread(),
            credit_spread: m.credit_spread,
            erp: m.equity_risk_premium(),
            fear: 16.0 * m.vol_multiplier(),
            risk_appetite: m.risk_appetite,
            recession_prob: m.recession_probability(),
            curve: m.curve.points(),
            forwards: crate::macro_econ::TENORS.map(|t| m.curve.forward_at(t)),
            curve_month_ago: self.curve_history.iter().rev().nth(20).copied(),
            cpi_index: 100.0 * m.log_cpi.exp(),
            gdp_index: 100.0 * m.log_real_gdp().exp(),
            advancers: self.breadth.0,
            decliners: self.breadth.1,
            market_sentiment: self.factors.market,
            sector_sentiment: self.factors.sector,
        };
        // Live composite line for the dock chart.
        let composite = self.indices[EXCHANGES].level();
        if self.t - self.composite_live_t >= 300.0 {
            self.composite_live_t = self.t;
            push_capped(&mut self.composite_live, composite as f32, 400);
        }
        let names = std::iter::once("Composite").chain(EXCHANGE_NAMES.iter().copied());
        let order = [EXCHANGES, 0, 1, 2, 3];
        let indices = names
            .zip(order)
            .map(|(name, i)| {
                let b = &self.indices[i];
                IndexView {
                    name: name.to_string(),
                    level: b.level(),
                    change_pct: (b.level() / self.index_open[i] - 1.0) * 100.0,
                    members: b.members,
                    history: self.index_history[i].iter().copied().collect(),
                }
            })
            .collect();

        let selected = self.selected.and_then(|id| self.detail(id));
        let player = self.player_view();
        let brokers = self.broker_rows();
        let money = MoneyFlows {
            interest_paid: self.money.interest_paid + self.player.interest,
            dividends_paid: self.money.dividends_paid + self.player.dividends,
            fees_collected: self.money.fees_collected,
            broker_capital_in: self.money.broker_capital_in,
            broker_capital_out: self.money.broker_capital_out,
            takeover_cash: self.money.takeover_cash,
            ipo_capital: self.money.ipo_capital,
            cash_total: self.cash_total(),
            market_cap_total: self.indices[EXCHANGES].cap_sum,
        };
        let c = &self.counters;
        Frame {
            seed: self.cfg.seed,
            clock: ClockView {
                t: self.t,
                parts,
                open: self.is_open(),
                session: Some(self.cfg.session),
                speed: 1.0,
                paused: false,
            },
            stats: EngineStats {
                live: self.live,
                capacity: CAPACITY,
                band: self.cfg.band,
                target: self.cfg.target_population,
                updates_per_sec: 0.0,
                samples_per_tick: c.last_samples,
                sweep_per_tick: c.last_sweep,
                mean_age_min: c.last_mean_age / 60.0,
                max_age_min: c.last_max_age / 60.0,
                tick_ms: 0.0,
                non_finite_events: c.non_finite,
                ipos: c.ipos,
                defaults: c.defaults,
                acquisitions: c.acquisitions,
                delistings: c.delistings,
                births_per_day: self.births_per_day(),
                deaths_per_day: c.death_ema,
                brokers_alive: self.brokers.iter().filter(|b| b.alive).count(),
                brokers_broke: c.brokers_broke,
            },
            drivers,
            indices,
            macro_history: MacroHistory {
                y10: self.macro_history[0].iter().copied().collect(),
                y3m: self.macro_history[1].iter().copied().collect(),
                policy: self.macro_history[2].iter().copied().collect(),
                inflation: self.macro_history[3].iter().copied().collect(),
                live: self.composite_live.iter().copied().collect(),
            },
            crawl: self.crawl.clone(),
            selected,
            player,
            brokers,
            money,
            table: if include_table { self.table.clone() } else { None },
            table_version: self.table_version,
            notices: std::mem::take(&mut self.notices),
        }
    }

    fn detail(&self, id: SecurityId) -> Option<Detail> {
        let s = self.security(id)?;
        let g = &self.globals;
        let eps = s.eps(g);
        let vol = s.equity_vol(g.vol_mult);
        let assets = s.price + s.debt_per_share;
        let dd = if s.debt_per_share > 0.0 {
            (assets / s.debt_per_share).ln() / (vol * s.price / assets).max(0.02)
        } else {
            f64::INFINITY
        };
        let issuer = match s.issuer {
            Issuer::Company => "Operating company".to_string(),
            Issuer::PlayerFounded => "Founded by you".to_string(),
            Issuer::BrokerFirm(b) => {
                format!("Listed broker: {}", self.brokers.get(b as usize).map_or("?", |b| b.name.as_str()))
            }
        };
        let is_firm = matches!(s.issuer, Issuer::BrokerFirm(_));
        Some(Detail {
            id,
            ticker: s.ticker,
            sector: s.sector,
            exchange: s.exchange,
            perks: s.perks,
            issuer,
            price: s.price,
            prev_close: s.prev_close,
            day_high: s.day_high,
            day_low: s.day_low,
            fair: s.fair,
            mispricing: s.price / s.fair - 1.0,
            pe: if is_firm { f64::NAN } else { s.price / eps },
            div_yield: if is_firm { 0.0 } else { s.payout * eps / s.price },
            beta: s.beta,
            vol,
            market_cap: s.market_cap(),
            shares: s.shares,
            leverage: s.leverage,
            distance_to_default: dd,
            cost_of_equity: s.cost_of_equity(g),
            growth: s.g,
            forecast: s.forecast_1y(g),
            listed_days: (self.t - s.listed_t) / SECONDS_PER_DAY,
            age_min: (self.t - s.t_last) / 60.0,
            updates: s.updates,
            split_factor: s.split_factor,
            history: s.closes.iter().copied().collect(),
            intraday: self.intraday.iter().copied().collect(),
            held_shares: self.player.positions.get(&id).map_or(0.0, |p| p.base_shares * s.split_factor),
        })
    }

    fn player_view(&self) -> PlayerView {
        let p = &self.player;
        let mut positions = Vec::new();
        let mut sector_value = [0.0; 11];
        let (mut invested, mut unrealized) = (0.0, 0.0);
        for (id, pos) in &p.positions {
            let Some(s) = self.security(*id) else { continue };
            let shares = pos.base_shares * s.split_factor;
            let value = shares * s.price;
            invested += value;
            unrealized += value - pos.cost;
            sector_value[s.sector as usize] += value;
            positions.push(PositionView {
                id: *id,
                ticker: s.ticker,
                sector: s.sector,
                shares,
                price: s.price,
                avg_cost: pos.cost / shares.max(1e-12),
                value,
                unrealized: value - pos.cost,
                change_pct: s.change_pct(),
            });
        }
        positions.sort_by(|a, b| b.value.total_cmp(&a.value));
        let orders = self
            .orders
            .iter()
            .map(|o| {
                let s = self.security(o.id);
                OrderView {
                    order_id: o.order_id,
                    id: o.id,
                    ticker: s.map(|s| s.ticker).unwrap_or_default(),
                    side: o.side,
                    kind: o.kind,
                    trigger: o.trigger,
                    amount: o.amount,
                    last_price: s.map_or(0.0, |s| s.price),
                }
            })
            .collect();
        PlayerView {
            cash: p.cash,
            equity: p.cash + invested,
            invested,
            unrealized,
            realized: p.realized_pnl,
            dividends: p.dividends,
            interest: p.interest,
            fees: p.fees,
            start_cash: self.cfg.player_cash,
            positions,
            orders,
            fills: self.fills.iter().cloned().collect(),
            sector_value,
        }
    }

    fn broker_rows(&self) -> Vec<BrokerRow> {
        self.brokers
            .iter()
            .map(|b| {
                let mut top: Vec<(Ticker, f64)> = b
                    .port
                    .positions
                    .iter()
                    .filter_map(|(id, p)| self.security(*id).map(|s| (s.ticker, p.base_shares * s.price * s.split_factor)))
                    .collect();
                top.sort_by(|a, b| b.1.total_cmp(&a.1));
                top.truncate(8);
                BrokerRow {
                    index: b.index,
                    name: b.name.clone(),
                    strategy: b.strategy,
                    alive: b.alive,
                    listed: b.listed.and_then(|id| self.security(id)).map(|s| s.ticker),
                    cash: b.port.cash,
                    equity: b.equity,
                    capital_in: b.capital_in,
                    roe: b.roe,
                    positions: b.port.positions.len(),
                    trades: b.trades,
                    age_days: (self.t - b.born_t) / SECONDS_PER_DAY,
                    top_holdings: top,
                }
            })
            .collect()
    }

    /// One-line status for the console.
    pub fn status_line(&self) -> String {
        let c = ClockParts::from_t(self.t);
        format!(
            "day {} {} {:02}:{:02} {} | live {} (band {}-{}, cap {}) | ipo {} def {} acq {} dl {} | brokers {}/{} broke {} | comp {:.1} | 10y {:.2}% pol {:.2}% cpi {:.2}% | NaN {}",
            c.day,
            c.weekday_name(),
            c.hour,
            c.minute,
            if self.is_open() { "OPEN" } else { "closed" },
            self.live,
            self.cfg.band.0,
            self.cfg.band.1,
            CAPACITY,
            self.counters.ipos,
            self.counters.defaults,
            self.counters.acquisitions,
            self.counters.delistings,
            self.brokers.iter().filter(|b| b.alive).count(),
            self.brokers.len(),
            self.counters.brokers_broke,
            self.indices[EXCHANGES].level(),
            self.econ.y10() * 100.0,
            self.econ.policy_rate * 100.0,
            self.econ.inflation * 100.0,
            self.counters.non_finite,
        )
    }

    /// Check every number the UI could show. Used by tests and the soak run.
    pub fn audit(&self) -> Result<(), String> {
        if !self.econ.is_finite() {
            return Err("macro state not finite".into());
        }
        for s in self.securities() {
            if !s.is_finite() || !(s.price > 0.0) {
                return Err(format!("security {} not finite: price {}", s.ticker.as_str(), s.price));
            }
        }
        for b in &self.brokers {
            if !b.port.cash.is_finite() || b.port.cash < -1e-6 || !b.equity.is_finite() {
                return Err(format!("broker {} bad cash/equity {} {}", b.name, b.port.cash, b.equity));
            }
        }
        for (i, book) in self.indices.iter().enumerate() {
            if !book.level().is_finite() {
                return Err(format!("index {i} not finite"));
            }
        }
        if !self.player.cash.is_finite() {
            return Err("player cash not finite".into());
        }
        Ok(())
    }
}

fn slot_is_selected(sel: Option<SecurityId>, s: &Security) -> bool {
    sel == Some(s.id)
}

fn push_capped(q: &mut VecDeque<f32>, v: f32, cap: usize) {
    if q.len() == cap {
        q.pop_front();
    }
    q.push_back(v);
}

/// Compact money formatting shared with logs and notices.
pub fn money_str(v: f64) -> String {
    let a = v.abs();
    let (n, suffix) = if a >= 1e12 {
        (v / 1e12, "T")
    } else if a >= 1e9 {
        (v / 1e9, "B")
    } else if a >= 1e6 {
        (v / 1e6, "M")
    } else if a >= 1e3 {
        (v / 1e3, "K")
    } else {
        (v, "")
    };
    format!("§{n:.2}{suffix}")
}

/// All sectors, for UI pickers.
pub fn sectors() -> &'static [Sector] {
    &SECTORS
}
