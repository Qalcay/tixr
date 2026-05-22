use crate::values::*;

use std::collections::VecDeque;
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

use rand::prelude::*;
use rand_chacha::ChaCha8Rng;
use rkyv::{Archive, Deserialize, Serialize};


// setup market
#[derive(Clone, Copy, Debug, Archive, Serialize, Deserialize)]
pub struct SeedMarketTheory {
    pub qhi: u64,
    pub qlo: u64,
}

impl SeedMarketTheory {
    pub fn set_u128(self) -> u128 {
        ((self.qhi as u128) << 64) | self.qlo as u128
    }
}

pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self { Self(seed) }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    pub fn next_f32_range(&mut self, min: f32, max: f32) -> f32 {
        let x = (self.next_u64() >> 40) as f32 / ((1u64 << 24) as f32);
        min + (max - min) * x
    }
}

#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
pub struct SetTickerStatus {
    pub price: f32,
    pub log_price: f32,
    pub target_price_log: f32,
    pub drift: f32,
    pub vol: f32,
    pub beta: f32,
    pub liquidity: f32,
    pub sentiment: f32,
    pub last_value: f32,
}

impl SetTickerStatus {
    pub fn new(seed_mix: u64, base_price: f32) -> Self {
        let mut sm = SplitMix64::new(seed_mix);
        let drift = sm.next_f32_range(      DRIFT_RANGE.0,      DRIFT_RANGE.1);
        let vol = sm.next_f32_range(        VOLUME_RANGE.0,     VOLUME_RANGE.1);
        let beta = sm.next_f32_range(       BETA_RANGE.0,       BETA_RANGE.1);
        let liquidity = sm.next_f32_range(  LIQUID_RANGE.0,     LIQUID_RANGE.1);
        let sentiment = sm.next_f32_range(  SENTIMENT_RANGE.0,  SENTIMENT_RANGE.1);
        Self {
            price: base_price.max(0.01),
            log_price: base_price.max(0.01).ln(),
            target_price_log: base_price.max(0.01).ln(),
            drift,
            vol,
            beta,
            liquidity,
            sentiment,
            last_value: 0.0,
        }
    }
}

#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
pub struct UserPosition {
    pub index_ticker: usize,
    pub entry_price: f32,
    pub has_quantity: f32,
    pub entry_tick: u64,
}

struct UserTradingAccount {
    pub total_balance: f64,
    pub current_position: Option<UserPosition>,
}

#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
struct DeadStock {
    stock_name: String,
    final_price: f32,
    death_price: u64,
}

#[derive(Clone, Copy, Debug, Archive, Serialize, Deserialize)]
pub enum FlowStates {
    TradingZen,
    TradingBliss,
    TradingTumult,
    TradingChaos,
}

#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
pub struct IndexGrouper {
    pub with_name: String,
    pub of_members: Vec<u32>,
    pub last_value: f32,
    pub with_history: VecDeque<f32>,
}

impl IndexGrouper {
    pub fn new(with_name: String, of_members: Vec<u32>, initialize: f32) -> Self {
        let mut with_history = VecDeque::with_capacity(HISTOGRAPHY);
        with_history.push_back(initialize);
        Self { with_name, of_members, last_value: initialize, with_history }
    }
}

#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
pub struct SnapshotEconomy {
    pub tick: u64,
    pub flow: FlowStates,
    pub driver: f32, // market stress, dynamics, etc...
    pub stocks: Vec<SetTickerStatus>,
    pub index: Vec<(String, f32)>,
    pub samples: Vec<(String, Vec<f32>)>,
    pub seed: SeedMarketTheory,
}

#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
pub struct SaveGameState {
    pub version:        u32,
    pub seed:           SeedMarketTheory,
    pub tick:           u64,
    pub flow:           FlowStates,
    pub driver:         f32,
    pub stocks:         Vec<SetTickerStatus>,
    pub index:          Vec<IndexGrouper>,
    pub ring_cursor:    usize,
    pub ring_prices_history: Vec<[f32; HISTOGRAPHY]>,
}


#[derive(Debug)]
pub struct EconomyEngine {
    pub seed:           SeedMarketTheory,
    pub tick:           u64,
    pub rng:            ChaCha8Rng,
    pub stocks:         Vec<SetTickerStatus>,
    pub index:          Vec<IndexGrouper>,
    pub flow:           FlowStates,
    pub driver:         f32,
    pub ring_cursor:    usize,
    pub ring_price_history: Vec<[f32; HISTOGRAPHY]>,
}

impl EconomyEngine {
    pub fn new(seed: SeedMarketTheory) -> Self {
        let mut rng = ChaCha8Rng::seed_from_u64(seed.set_u128() as u64 ^ ((seed.set_u128() >> 64) as u64));
        let mut stocks = Vec::with_capacity(INDEX_MATRIX);
        let mut ring_price_history = Vec::with_capacity(INDEX_MATRIX);
        for i in 0..INDEX_MATRIX {
            let base = ECONOMY_BASE + (i as f32 % ECONOMY_PERCENTAGE);
            let s = SetTickerStatus::new(rng.next_u64() ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15), base);
            ring_price_history.push([s.price; HISTOGRAPHY]);
            stocks.push(s);
        }

        let index = Self::build_index(INDEX_MATRIX);
        Self {
            seed,
            tick: 0,
            rng,
            stocks,
            index,
            flow:           FlowStates::TradingBliss,
            driver:         ECONOMY_DRIVER,
            ring_cursor:    0,
            ring_price_history,
        }//;

        //for _ in 0..YEAR_OF_TICK_RATE { engine.step(); }
        //engine
    }

    //pub fn initialize_stock(rng: &mut ChaCha8Rng, );

    pub fn build_index(n: usize) -> Vec<IndexGrouper> {
        let mut groups = Vec::new();
        let all: Vec<u32> = (0..n as u32).collect();
        groups.push(IndexGrouper::new("indexed".into(), all, 1000.0));

        for g in 0..1 {
            let members = (0..n as u32).filter(|i| (i % 1) == g).collect::<Vec<_>>();
            groups.push(IndexGrouper::new(format!("index_{}", g), members, 1000.0));
        }
        groups
    }


    pub fn step(&mut self) {
        thread::sleep(Duration::from_millis(STEP_DELAY));
        self.tick += 1;
        let flowing: f32 = self.rng.random();
        self.flow = match (self.flow, flowing) {
            (FlowStates::TradingZen, r)     if r < STEP_LOCK_MOTION / TENSE_1 => FlowStates::TradingBliss,
            (FlowStates::TradingBliss, r)   if r < STEP_LOCK_MOTION / TENSE_2 => FlowStates::TradingTumult,
            (FlowStates::TradingTumult, r)  if r < STEP_LOCK_MOTION / TENSE_3 => FlowStates::TradingChaos,
            (FlowStates::TradingChaos, r)   if r < STEP_LOCK_MOTION / RELAX_1 => FlowStates::TradingTumult,
            (FlowStates::TradingTumult, r)  if r < STEP_LOCK_MOTION / RELAX_2 => FlowStates::TradingBliss,
            (FlowStates::TradingBliss, r)   if r < STEP_LOCK_MOTION / RELAX_3 => FlowStates::TradingZen,
            _ => self.flow,
        };

        let flow_volume = match self.flow {
            FlowStates::TradingZen      =>       (STEP_DELAY_F32 / STEP_LOCK_MOTION) + FLOW_ZEN, // 0.60,
            FlowStates::TradingBliss    =>       (STEP_DELAY_F32 / STEP_LOCK_MOTION) + FLOW_BLISS, // 1.0,
            FlowStates::TradingTumult   =>       (STEP_DELAY_F32 / STEP_LOCK_MOTION) + FLOW_TUMULT, // 1.8,
            FlowStates::TradingChaos    =>      (STEP_DELAY_F32 / STEP_LOCK_MOTION) + FLOW_CHAOS, // 3.8,
        };

        let driver_innovating = self.fat_tail_noise() * 0.06;
        self.driver = (0.988 * self.driver + driver_innovating.abs()).clamp(0.0, 4.0);

        let crash_risk = (
            CRASH_PROB_FLOOR
            + self.driver
            * CRASHING_SCORE
            + if matches!(self.flow, FlowStates::TradingChaos) { CRASHING_BOUND }
            else { 0.0 }).clamp(0.0, 0.02);
        let market_craters = self.rng.random::<f32>() < crash_risk;
        let crash_alert = if market_craters {
            -self.rng.random_range(0.09..0.33) // 0.07 .. 0.22
        } else { 0.0 };

        let local_noise = self.fat_tail_noise();

        for (i, stock) in self.stocks.iter_mut().enumerate() {
            let id = i as f32;
            let cyc = ((self.tick as f32 *0.0015) + id * 0.013).sin();
            //let x = self.stocks.iter_mut();
            let cross = 0.33 * self.driver * cyc + 0.10 * stock.sentiment;

            let mean_reversion = -MEAN_REVERT_APPLY * (stock.log_price - (stock.price.max(1.0).ln()));
            //let mean_reversion = -0.002 * (stock.log_price - (stock.price.max(1.0).ln()));

            let volume_scaling = flow_volume * (stock.vol * (1.0 + 0.4 * self.driver));
            let jump_prob = (0.00005 + self.driver * 0.00008 + stock.liquidity * 0.00002).clamp(0.0, 0.01);
            let jumps = if self.rng.random::<f32>() < jump_prob {
                let dir = if self.rng.random::<f32>() < 0.55 { -1.0 } else { 1.0 };
                dir * self.rng.random_range(0.01..0.06) * (1.0 + stock.beta * 0.35)
            } else { 0.0 };

            let ret = stock.drift
                + mean_reversion
                + cross * 0.0012
                + local_noise * volume_scaling
                + jumps
                + crash_alert * stock.beta * 0.018;

            stock.last_value = ret;

            stock.sentiment = (0.995 * stock.sentiment + 0.02 * ret -0.01 * self.driver).clamp(-1.0, 1.0);
            stock.vol = (0.997 * stock.vol + 0.003 * (0.004 + ret.abs() * 0.75)).clamp(0.001, 0.12);
            stock.price = (stock.price * (ret).exp()).clamp(0.05, 1000000.0);
            stock.log_price = stock.price.ln();

            self.ring_price_history[i][self.ring_cursor] = stock.price;
        }

        self.ring_cursor = (self.ring_cursor + 1) % HISTOGRAPHY;

        for work_index in &mut self.index {
            let sum = work_index.of_members.iter().map(|&m| self.stocks[m as usize].price as f64).sum::<f64>();
            let v = (sum / work_index.of_members.len() as f64) as f32;
            work_index.last_value = v;
            work_index.with_history.push_back(v);

            while work_index.with_history.len() > HISTOGRAPHY {
                work_index.with_history.pop_front();
            }
        }
    }

    //
    //
    //
    //

    pub fn fat_tail_noise(&mut self) -> f32 {
        let u1: f32 = self.rng.random::<f32>().clamp(1e-7, 1.0 - 1e-7);
        let u2: f32 = self.rng.random::<f32>().clamp(1e-7, 1.0 - 1e-7);
        let u3: f32 = self.rng.random::<f32>().clamp(1e-7, 1.0 - 1e-7);

        let normal = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
        let cauchy = (std::f32::consts::PI * (u3 - 0.5)).tan().clamp(-20.0, 20.0);

        (0.88 * normal + 0.12 * cauchy).clamp(-8.0, 8.0)
    }

    pub fn snapshot(&self) -> SnapshotEconomy {
        let index = self.index.iter().map(|g| (g.with_name.clone(), g.last_value)).collect();
        let samples = self.samples();
        SnapshotEconomy {
            tick: self.tick,
            flow: self.flow,
            driver: self.driver,
            stocks: self.stocks.clone(),
            index,
            samples,
            seed: self.seed,
        }
    }

    pub fn samples(&self) -> Vec<(String, Vec<f32>)> {
        let mut out = Vec::new();
        let stride = (INDEX_MATRIX / USER_INTERFACE_SAMPLING).max(1);
        for i in 0..USER_INTERFACE_SAMPLING {
            let work_index = i * stride;
            let mut series = Vec::with_capacity(HISTOGRAPHY);
            for j in 0..HISTOGRAPHY {
                let p = self.ring_price_history[work_index][(self.ring_cursor + j) % HISTOGRAPHY];
                series.push(p);
            }
            out.push((format!("ticker{}", work_index + 1), series));
        }
        out
    }

    pub fn saver(&self) -> SaveGameState {
        SaveGameState {
            version: 15,
            seed: self.seed,
            tick: self.tick,
            flow: self.flow,
            driver: self.driver,
            stocks: self.stocks.clone(),
            index: self.index.clone(),
            ring_cursor: self.ring_cursor,
            ring_prices_history: self.ring_price_history.clone(),
        }
    }
}
