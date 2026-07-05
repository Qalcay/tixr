use crate::values::*;
use crate::knobs::Knobs;

use std::collections::{HashMap, HashSet, VecDeque};

use rand::prelude::*;
use rand_chacha::ChaCha8Rng;
use rkyv::{Archive, Deserialize, Serialize};



/*
 * Seed as 128-bit
 */

#[derive(Clone, Copy, Debug, Archive, Serialize, Deserialize)]
pub struct SeedMarketTheory { pub q_high: u64, pub q_low: u64 }
impl SeedMarketTheory {
    pub fn as_u128(self) -> u128 { ((self.q_high as u128) << 64) | self.q_low as u128 }

    pub fn expand(self) -> [u8; 32] {
        let u = self.as_u128();
        let mut out = [0u8; 32];
        out[..16].copy_from_slice(&u.to_le_bytes());
        let mut sm = SplitMix64::new(self.q_high ^ SPLIT_MIX_HIGH);
        let a = sm.next_u64() ^ self.q_low;
        let b = sm.next_u64().wrapping_add(self.q_high);
        out[16..24].copy_from_slice(&a.to_le_bytes());
        out[24..32].copy_from_slice(&b.to_le_bytes());
        out
    }
}

pub struct SplitMix64(u64);
impl SplitMix64 {
    pub fn new(seed: u64) -> Self { Self(seed) }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(NEXT_U64);
        let mut z = self.0;
        z = (z ^ (z >> BIT_SHIFT_1)).wrapping_mul(Z_STEP_1);
        z = (z ^ (z >> BIT_SHIFT_2)).wrapping_mul(Z_STEP_2);
        z ^ (z >> BIT_SHIFT_3)
    }
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        let x = (self.next_u64() >> 40) as f32 / ((1u64 << 24) as f32);
        lo + (hi - lo) * x
    }
    pub fn range64(&mut self, lo: f64, hi: f64) -> f64 {
        let x = (self.next_u64() >> 11) as f64 / ((1u64 << 53) as f64);
        lo + (hi - lo) * x
    }
}




/*
 * Regime
 *
 */

#[derive(Clone, Copy, Debug, PartialEq, Archive, Serialize, Deserialize)]
pub enum FlowStates { Zen, Bliss, Tumult, Chaos }

impl FlowStates {
    pub fn params(self) -> (f32, f32, f32, f32) {
        match self {
            FlowStates::Zen     => RIDE_ZEN,
            FlowStates::Bliss   => RIDE_BLISS,
            FlowStates::Tumult  => RIDE_TUMULT,
            FlowStates::Chaos   => RIDE_CHAOS,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            FlowStates::Zen     => "calm",
            FlowStates::Bliss   => "bullish",
            FlowStates::Tumult  => "bearish",
            FlowStates::Chaos   => "crash"
        }
    }
}



/*
 * Per-Stock State
 */

#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
pub struct SetTickerStatus {
    pub nameplate:      String,
    pub market_id:      u16,
    pub set_alive:      bool,
    pub price:          f32,
    pub log_price:      f32,
    pub fair_log:       f32,
    pub mu:             f32,
    pub beta:           f32,
    pub var:            f32,
    pub sig_long:       f32,
    pub cycle_amp:      f32,
    pub cycle_period:   f32,
    pub cycle_phase:    f32,
    pub solvency:       f32,
    pub shares:         f64,
    pub last_return:    f32,
    pub last_volume:    f64,
    pub ending_tick:    Option<u64>,
    pub close_daily:    VecDeque<f32>,
    #[rkyv(with = rkyv::with::Skip)]
    pub live_tail:      VecDeque<f32>,
}

impl SetTickerStatus {
    fn spawn(pp: &mut SplitMix64, market_id: u16, nameplate: String) -> Self {
        let price = pp.range(LAUNCH_IPO_PRICE.0, LAUNCH_IPO_PRICE.1);
        let sig_long = pp.range(SIGMA_LONG.0, SIGMA_LONG.1);
        let mut close_daily = VecDeque::with_capacity(DAYS_HISTOGRAPH);
        close_daily.push_back(price);
        Self {
            nameplate, market_id, set_alive: true,
            price, log_price: price.ln(), fair_log: price.ln(),
            mu:             pp.range(MU_ANN.0, MU_ANN.1),
            beta:           pp.range(BETA_RANGE.0, BETA_RANGE.1),
            var:            sig_long * sig_long,
            sig_long,
            cycle_amp:      pp.range(CYCLE_AMPLITUDE.0, CYCLE_AMPLITUDE.1),
            cycle_period:   pp.range(CYCLE_PERIODICITY.0, CYCLE_PERIODICITY.1),
            cycle_phase:    pp.range(0.0, std::f32::consts::TAU),
            solvency:       pp.range(INITIAL_SOLVENCY.0, INITIAL_SOLVENCY.1),
            shares:         pp.range64(SHARES_IN_RANGE.0, SHARES_IN_RANGE.1),
            last_return: 0.0, last_volume: 0.0, ending_tick: None,
            close_daily, live_tail: VecDeque::with_capacity(LIVE_RING),
        }
    }
    pub fn market_cap(&self) -> f64 { self.price as f64 * self.shares }
}



/*
 * Macro-Economics
 */

#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
pub struct MacroState {
     pub short_rate:    f32,
     pub inflation:     f32,
     pub output_gap:    f32,
     pub money_supply:  f64,
     pub long_rate:     f32,
     pub yield_curve:   [f32; 6],
}

impl MacroState {
    fn new() -> Self {
        Self {
            short_rate: 0.03,
            inflation: 0.022,
            output_gap: 0.0,
            money_supply: INITIAL_MONEY_SUPPLY,
            long_rate: 0.038,
            yield_curve: [0.03, 0.032, 0.034, 0.036, 0.038, 0.04]
        }
    }

    fn step(&mut self, rng: &mut ChaCha8Rng) {
        let g = || -> f32 { 0.0 };
        let _ = g; // placeholder keeping diffs small

        let money_growth = 0.05 + 0.03 * self.output_gap;
        self.money_supply *= (1.0 + money_growth as f64 * DT as f64).max(0.0);

        let infl_pull = money_growth - MARKET_PRODUCTIVITY;
        self.inflation += 1.5
            * (infl_pull - self.inflation)
            * DT
            + 0.015
            * DT.sqrt()
            * standard_normal(rng);
        self.inflation = self.inflation.clamp(CLAMP_RATE_INFLATION.0, CLAMP_RATE_INFLATION.1);

        // taylor rule target, ease policy rate to it... (banks move slowly)
        let taylor = RATE_NEUTRALITY + self.inflation
            + 0.5
            * (self.inflation - INFLATION_TARGET)
            + 0.5
            * self.output_gap;
        self.short_rate += 2.0 * (taylor - self.short_rate) * DT;
        self.short_rate = self.short_rate.clamp(CLAMP_RATE_SPENDING.0, CLAMP_RATE_SPENDING.1);

        let real_rate = self.short_rate - self.inflation;
        self.output_gap
            += (-0.8 * real_rate - 0.5 * self.output_gap)
            * DT
            + 0.02
            * DT.sqrt()
            * standard_normal(rng);
        self.output_gap = self.output_gap.clamp(CLAMP_RATE_GAP.0, CLAMP_RATE_GAP.1);

        let expected_short = self.short_rate
            + 0.5
            * (RATE_NEUTRALITY - self.short_rate);
        self.long_rate = expected_short + TERM_PREMIUM;
        let tenors = [0.25f32, 1.0, 2.0, 5.0, 10.0, 30.0];
        for (k, &t) in tenors.iter().enumerate() {
            let w = (t / 10.0).min(1.0);
            self.yield_curve[k] =
                self.short_rate
                    * (1.0 - w)
                    + self.long_rate
                    * w
                    + TERM_PREMIUM
                    * (t / 30.0)
        }
    }
}



/*
 * Exhange Markets
 *     group of stocks will share a 'systemic shock'
 *      + an actual chiron field
 */

#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
pub struct Markets {
    pub name:           String,
    pub members:        Vec<u32>,
    pub index_value:    f32,
    pub last_shocks:    f32,
}



/*
 * Portfolio
 *      both Player and NPCs (Brokers)
 *       share a single shape system
 */

#[derive(Clone, Debug, Default, Archive, Serialize, Deserialize)]
pub struct Portfolio {
    pub cash:           f64,
    pub holdings:       HashMap<usize, f64>,
    pub realized_pnl:   f64,
}

impl Portfolio {
    fn new(cash: f64) -> Self { Self { cash, holdings: HashMap::new(), realized_pnl: 0.0 } }
    pub fn equity(&self, stocks: &[SetTickerStatus]) -> f64 {
        self.cash + self.holdings.iter()
            .filter(|&(&i, _)| stocks[i].set_alive)
            .map(|(&i, &sh)| stocks[i].price as f64 * sh)
            .sum::<f64>()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Archive, Serialize, Deserialize)]
pub enum Strategy {
    Momentum,
    Value,
    Index,
    Panic,
    Whale,
    Random
}

#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
pub struct Brokers {
    pub name:           String,
    pub strategy:       Strategy,
    pub port:           Portfolio,
}



/*
 * SnapShot
 *
 *
 */

#[derive(Clone, Debug)]
pub struct StockRow {
    pub ix:             usize,
    pub nameplate:      String,
    pub market_id:      u16,
    pub price:          f32,
    pub perc:           f32,
    pub alive:      bool,
}

#[derive(Clone, Debug)]
pub struct NpcRow {
    pub name:           String,
    pub strategy:       Strategy,
    pub equity:         f64,
}

#[derive(Clone, Debug)]
pub struct SnapshotEconomy {
    pub tick:           u64,
    pub day:            u64,
    pub market_open:    bool,
    pub flow:           FlowStates,
    pub seed:           SeedMarketTheory,
    pub macro_state:    MacroState,
    pub total_market_cap: f64,
    pub total_volume:   f64,
    pub markets:        Vec<(String, f32)>,
    pub rows:           Vec<StockRow>,
    pub selected:       Option<(String, Vec<f32>)>,
    pub player_cash:    f64,
    pub player_equity:  f64,
    pub npcs:           Vec<NpcRow>,
}



/*
 * Save
 */

#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
pub struct SaveGameState {
    pub version:        u32,
    pub seed:           SeedMarketTheory,
    pub tick:           u64,
    pub flow:           FlowStates,
    pub macro_states:   MacroState,
    pub stocks:         Vec<SetTickerStatus>,
    pub markets:        Vec<Markets>,
    pub player:         Portfolio,
    pub npcs:           Vec<Brokers>,
    pub used_names:     Vec<String>,
 }



/*
 * the Engine
 */

pub struct EconomyEngine {
    pub seed:           SeedMarketTheory,
    pub tick:           u64,
    pub rng:            ChaCha8Rng,
    pub param_rng:      SplitMix64,
    pub stocks:         Vec<SetTickerStatus>,
    pub markets:        Vec<Markets>,
    pub macro_state:    MacroState,
    pub flow:           FlowStates,
    pub player:         Portfolio,
    pub npcs:           Vec<Brokers>,
    pub used_names:     HashSet<String>,
    pub knobs:          Knobs,
    pub selected:       usize,
}

impl EconomyEngine {
    pub fn new(seed: SeedMarketTheory) -> Self {
        let mut rng = ChaCha8Rng::from_seed(seed.expand());
        let mut param_rng = SplitMix64::new(seed.as_u128() as u64 ^ SCRAMBLING_SALT);
        let mut used_names: HashSet<String> = HashSet::with_capacity(EVALUATED);

        let names = [" #1EX ", " #C2X ", " #4MX ", " #ST8 "];
        //let names = [" #OnEx ", " #JadEx ", " #QuMax ", " #W1N "];
        //let names = ["PANAXO", "FASQNET", "SORTVEL", "TOPMARK"];
        let mut stocks = Vec::with_capacity(EVALUATED);
        for i in 0..EVALUATED {
            let m = (i % EXCHANGES) as u16;
            let np = draw_nameplate(&mut param_rng, m, &mut used_names);
            stocks.push(SetTickerStatus::spawn(&mut param_rng, m, np));
        }

        let mut markets = Vec::with_capacity(EXCHANGES);
        for m in 0..EXCHANGES {
            let members = (0..EVALUATED as u32).filter(|&i| (i as usize % EXCHANGES) == m).collect();
            markets.push(Markets { name: names[m].into(), members, index_value: 1000.0, last_shocks: 0.0 });
        }

        let strategies = [
            Strategy::Momentum,
            Strategy::Value,
            Strategy::Index,
            Strategy::Panic,
            Strategy::Whale,
            Strategy::Random
        ];
        let mut npcs = Vec::with_capacity(BROKERS);
        for i in 0..BROKERS {
            let strat = strategies[i % strategies.len()];
            let cash = param_rng.range64(BROKER_WALLET_LAUNCH.0, BROKER_WALLET_LAUNCH.1)
                * if strat == Strategy::Whale { 20.0 } else { 1.0 };
            npcs.push(Brokers { name: format!("bot_{i:03}"), strategy: strat, port: Portfolio::new(cash) });
        }

        let mut eng = Self {
            seed, tick: 0, rng, param_rng, stocks, markets,
            macro_state: MacroState::new(), flow: FlowStates::Bliss,
            player: Portfolio::new(PLAYER_WALLET_BEGINS), npcs, used_names,
            knobs: Knobs::default() ,selected: 0,
        };

        for _ in 0..PREHEAT_TICKS { eng.step(); }
        eng.tick = 0;   // start at 0 init, but run prices to age
        eng
    }

    pub fn day(&self) -> u64 { self.tick / TICKS_DAY }
    pub fn market_open(&self) -> bool { (self.day() % LENGTH_WEEK) < DAYS_TRADING }

    pub fn step(&mut self) {
        self.tick += 1;
        let new_day = self.tick % TICKS_DAY == 0;

        // (1) macro pushes tick slowly
        self.macro_state.step(&mut self.rng);

        // (2) transitory regime, hidden markov chain -- bad macro raises crash odds
        self.flow = next_regime(self.flow, &self.macro_state, &mut self.rng);
        let k = self.knobs;
        let (mut volm, reg_drift, lam_mult, sys_bias) = self.flow.params();
        if self.flow == FlowStates::Chaos { volm *= k.crash_mult; } // godly crash severity ??

        // (3) systemic shock per market this tick (co-movement source)
        let mut market_shock = [0.0f32; EXCHANGES];
        for m in 0..EXCHANGES {
            market_shock[m] = standard_normal(&mut self.rng) + sys_bias;
            self.markets[m].last_shocks = market_shock[m];
        }

        let short_rate = self.macro_state.short_rate;
        let t_years = self.tick as f32 * DT;
        let mut total_volume = 0.0f64;

        // (4) price loop -- competing equations, summed per stock
        for s in self.stocks.iter_mut() {
            if !s.set_alive { continue; }
            let eps = standard_normal(&mut self.rng);

            let drift = (
                s.mu
                + (EQUITY_PREMIUM
                    - short_rate
                    + reg_drift
                    * 0.01)
                * s.beta)
                * DT
                * k.drift_mult;

            let cycle =
                s.cycle_amp
                * (std::f32::consts::TAU
                    * t_years
                    / s.cycle_period
                    + s.cycle_phase).sin()
                * DT;

            let reversion =
                -KAPPA
                * (s.log_price
                    - s.fair_log)
                * DT;

            let idio =
                (s.var
                    * DT).sqrt()
                * volm
                * eps
                * k.vol_mult;

            let systemic =
                s.beta
                * MARKET_VOLUME
                * DT.sqrt()
                * volm
                * market_shock[s.market_id as usize]
                * k.vol_mult;

            let lam =
                JUMP_LAMBA_ANN
                * lam_mult
                * k.jump_mult;

            let jump =
                if self.rng.random::<f32>()
                    < lam
                    * DT {
                        JUMP_MEAN
                        + JUMP_STD
                        * standard_normal(&mut self.rng)
                    } else { 0.0 };

            let ret =
                drift
                + cycle
                + reversion
                + idio
                + systemic
                + jump;

            s.last_return = ret;
            s.log_price += ret;
            s.fair_log += s.mu * DT;

            let realized = (ret / DT.sqrt()).powi(2);
            s.var = (
                OMEGA_GARCH
                * s.sig_long
                * s.sig_long
                + ALPHA_GARCH
                * realized
                + BETA_GARCH
                * s.var
                ).clamp(CLAMP_VARIANCE.0, CLAMP_VARIANCE.1);
            s.price = s.log_price.exp();
            s.last_volume = s.price as f64
                * s.shares
                * 0.001
                * (1.0 + 40.0 * ret.abs() as f64);
            total_volume += s.last_volume;

            // solvency: chronic losers will erode, winners become replinshed
            s.solvency += s.mu * 0.4 * DT + ret * 3.0 - 0.0008 * DT * k.ending_mult;

            s.live_tail.push_back(s.price);
            if s.live_tail.len() > LIVE_RING { s.live_tail.pop_front(); }
            if new_day {
                s.close_daily.push_back(s.price);
                if s.close_daily.len() > DAYS_HISTOGRAPH { s.close_daily.pop_front(); }
            }
        }

        // full market volume scales up to the entire sim-planet, spiked in chaos
        let vol_scale = if self.flow == FlowStates::Chaos { 5.0 } else { 1.0 };
        let _full_volume = total_volume * vol_scale; // is exposed via snapshot()


        // (5) 'ending' & 'spawning' -- bankrupt or floored stocks delist;
        //          their slot empties...
        let tick = self.tick;
        for i in 0..self.stocks.len() {
            let s = &self.stocks[i];
            if s.set_alive && (s.solvency <= 0.0 || s.price < FLOOR_PRICE) {
                let old = self.stocks[i].nameplate.clone();
                self.used_names.remove(&old);
                self.stocks[i].set_alive = false;
                self.stocks[i].ending_tick = Some(tick);
            } else if !s.set_alive {
                if self.rng.random::<f32>() < 0.0006 {
                    let m = self.stocks[i].market_id;
                    let np = draw_nameplate(&mut self.param_rng, m, &mut self.used_names);
                    self.stocks[i] = SetTickerStatus::spawn(&mut self.param_rng, m, np);
                }
            }
        }

        // (6) indexes -- aberage member price per market
        for mk in self.markets.iter_mut() {
            let (mut sum, mut n) = (0.0f64, 0.0f64);
            for &mem in &mk.members {
                let st = &self.stocks[mem as usize];
                if st.set_alive { sum += st.price as f64; n += 1.0; }
            }
            if n > 0.0 { mk.index_value = (sum / n) as f32; }
        }

        // (7) allow npcs to trade
        //      (cheap: each acts rarely, on a few names)...
        //          Works when open!
        if self.market_open() { self.brokers_work(); }
    }

    fn brokers_work(&mut self) {
        for k in 0..self.npcs.len() {
            if self.rng.random::<f32>()
                > BROKER_ACTION_CHANCE
                * self.knobs.broker_aggro
                { continue; }
            let strat = self.npcs[k].strategy;
            let pick = self.rng.random_range(0..self.stocks.len());
            if !self.stocks[pick].set_alive { continue; }
            let st = &self.stocks[pick];
            let want_buy = match strat {
                Strategy::Momentum  => st.last_return > 0.0,            // chase winners
                Strategy::Value     => st.log_price < st.fair_log,      // buy cheap
                Strategy::Index     => self.rng.random::<bool>(),       // mechanical DCA??
                Strategy::Panic     => st.last_return > 0.0,            // buy calm, dump at red
                Strategy::Whale     => st.last_return.abs() < 0.005,    // accumulate quietly
                Strategy::Random    => self.rng.random::<bool>(),
            };

            let cash = self.npcs[k].port.cash;
            let notional = cash * self.rng.random_range(0.01..0.08);
            if want_buy && notional > 0.0 {
                self.fill(pick, notional, true, Some(k));
            } else if !want_buy {
                // sell a slice if held
                if let Some(&sh) = self.npcs[k].port.holdings.get(&pick) {
                    let sell = sh * self.rng.random_range(0.1..0.5);
                    self.fill(pick, sell * self.stocks[pick].price as f64, false, Some(k));
                }
            }
        }
    }

    pub fn fill(&mut self, ix: usize, notional: f64, buy: bool, who: Option<usize>) -> f64 {
        if !self.market_open() || !self.stocks[ix].set_alive { return 0.0; }
        let price = self.stocks[ix].price as f64;
        if price <= 0.0 { return 0.0; }
        let shares = (notional / price).max(0.0);
        if shares <= 0.0 { return 0.0; }
        {
            let port = match who { None => &mut self.player, Some(k) => &mut self.npcs[k].port };
            if buy {
                if notional > port.cash { return 0.0; }
                port.cash -= notional;
                *port.holdings.entry(ix).or_insert(0.0) += shares;
            } else {
                let held = *port.holdings.get(&ix).unwrap_or(&0.0);
                let sell = shares.min(held);
                if sell <= 0.0 { return 0.0; }
                port.cash += sell * price;
                *port.holdings.get_mut(&ix).unwrap() -= sell;
            }
        }
        let impact_mult = self.knobs.impact_mult;
        let st = &mut self.stocks[ix];
        let impact = PRICE_ORDER_IMPACT * impact_mult * (notional / (price * st.shares).max(1.0)) as f32;
        st.log_price += if buy { impact } else { -impact };
        st.price = st.log_price.exp();
        shares
    }

    pub fn player_buy(&mut self, ix: usize, notional: f64) -> f64 { self.fill(ix, notional, true, None) }
    pub fn player_sell(&mut self, ix: usize, notional: f64) -> f64 { self.fill(ix, notional, false, None) }
    pub fn select(&mut self, ix: usize) { if ix < self.stocks.len() { self.selected = ix; } }



    // create snapshot ui system

    pub fn snapshot(&self) -> SnapshotEconomy {
        let rows = self.stocks
            .iter()
            .enumerate()
            .filter(|(_, s)| s.set_alive)
            .map(|(i, s)| {
                let prev = s.close_daily.back().copied().unwrap_or(s.price);
                let perc = if prev != 0.0 { (s.price - prev) / prev * 100.0 } else { 0.0 };
                StockRow {
                    ix: i,
                    nameplate: s.nameplate.clone(),
                    market_id: s.market_id,
                    price: s.price,
                    perc,
                    alive: s.set_alive
                }
            }).collect();

        let sel = &self.stocks[self.selected];
        let mut series: Vec<f32> = sel.close_daily.iter().copied().collect();
        series.extend(sel.live_tail.iter().copied());
        let selected = Some((sel.nameplate.clone(), series));

        let total_market_cap = self.stocks.iter().filter(|s| s.set_alive).map(|s| s.market_cap()).sum();
        let total_volume = self.stocks.iter().filter(|s| s.set_alive).map(|s| s.last_volume).sum::<f64>()
            * if self.flow == FlowStates::Chaos { 5.0 } else { 1.0 };

        let markets = self.markets.iter().map(|m| (m.name.clone(), m.index_value)).collect();
        let npcs = self.npcs.iter().map(|n| NpcRow {
            name: n.name.clone(),
            strategy: n.strategy,
            equity: n.port.equity(&self.stocks),
        }).collect();

        SnapshotEconomy {
            tick: self.tick,
            day: self.day(),
            market_open: self.market_open(),
            flow: self.flow,
            seed: self.seed,
            macro_state: self.macro_state.clone(),
            total_market_cap,
            total_volume,
            markets,
            rows,
            selected,
            player_cash: self.player.cash,
            player_equity: self.player.equity(&self.stocks),
            npcs,
        }
    }

    pub fn saver(&self) -> SaveGameState {
        SaveGameState {
            version:             32,
            seed:               self.seed,
            tick:               self.tick,
            flow:               self.flow,
            macro_states:       self.macro_state.clone(),
            stocks:             self.stocks.clone(),
            markets:            self.markets.clone(),
            player:             self.player.clone(),
            npcs:               self.npcs.clone(),
            used_names:         self.used_names.iter().cloned().collect(),
        }
    }

    pub fn loads(save: SaveGameState) -> Self {
        let mut rng = ChaCha8Rng::from_seed(save.seed.expand());
        for _ in 0..(save.tick % 4096) { let _ = rng.next_u32(); }
        Self {
            seed: save.seed,
            tick: save.tick,
            rng,
            param_rng: SplitMix64::new(save.seed.as_u128() as u64 ^ save.tick),
            stocks: save.stocks,
            markets: save.markets,
            macro_state: save.macro_states,
            flow: save.flow,
            player: save.player,
            npcs: save.npcs,
            used_names: save.used_names.into_iter().collect(),
            knobs: Knobs::default(), selected: 0,
        }
    }
}






















/*
 * Free Fucntions
 */

fn standard_normal(rng: &mut ChaCha8Rng) -> f32 {
    let u1 = rng.random::<f32>().clamp(1e-7, 1.0 - 1e-7);
    let u2 = rng.random::<f32>();
    (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
}

// hidden-Markov regime chain... return the following FlowState
fn next_regime(cur: FlowStates, m: &MacroState, rng: &mut ChaCha8Rng) -> FlowStates {
    let stress = (m.inflation -0.03).max(0.0) * 3.0 + (-m.output_gap).max(0.0) * 2.0;
    let r = rng.random::<f32>();
    match cur {
        FlowStates::Zen | FlowStates::Bliss => {
            if r < 0.0006 + 0.002 * stress { FlowStates::Chaos }
            else if r < 0.004 + 0.01 * stress { FlowStates::Tumult }
            else if rng.random::<bool>() { FlowStates::Bliss } else { FlowStates::Zen }
        }
        FlowStates::Tumult => {
            if r < 0.003 { FlowStates::Chaos }
            else if r < 0.02 { FlowStates::Zen }
            else { FlowStates::Tumult }
        }
        FlowStates::Chaos => if r < 0.06 { FlowStates::Tumult } else { FlowStates::Chaos },
    }
}

fn draw_nameplate(pp: &mut SplitMix64, market_id: u16, used: &mut HashSet<String>) -> String {
    let len = NAMEPLATE_LENGTHS[market_id as usize];
    let charset: Vec<char> = NAMEPLATE_ALPHABET.chars().collect();
    for _ in 0..64 {
        let name: String = (0..len)
            .map(|_| charset[(pp.next_u64() as usize) % charset.len()])
            .collect();
        if !used.contains(&name) {
            used.insert(name.clone());
            return name;
        }
    }
    let mut n = used.len();
    loop {
        let s = format!("Z{n}");
        if !used.contains(&s)
            {
                used.insert(s.clone()); return s;
            }
        n += 1;
    }
}
