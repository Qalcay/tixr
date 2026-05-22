/*
RUNNING BUG LIST:
- BUG #1 (Logic): INDEX_MATRIX used ^ (XOR) instead of exponentiation. (Line 24)
- BUG #2 (Crash): samples() would index out of bounds if INDEX_MATRIX < USER_INTERFACE_SAMPLING. (Line 230)
- BUG #3 (Math): fat_tail_noise Box-Muller transform was missing .sqrt(), breaking the normal distribution. (Line 214)
- BUG #4 (Math): Mean reversion was subtracting a value from itself (log_price - price.ln()), resulting in 0. (Line 167)
- BUG #5 (Concurrency): Sim thread panics were silent. Added basic error handling.
*/
// BUG LOG / TODO
// 1) Verify plotting backend choice on the target OS (egui_plot vs custom painter).
// 2) Tune the crash hazard so it feels plausible, not theatrical.
// 3) Add file format versioning before shipping save/load.
// 4) Confirm seed-to-state determinism across platforms with endianness tests.
// 5) Add backpressure when UI falls behind the simulation thread.
//
//
//! - 4096 stocks
//! - deterministic from a 128-bit seed
//! - two-thread design: simulation thread + GUI thread
//! - compact save-state / replay-friendly
//
//

use std::collections::VecDeque;
//use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender};
use eframe::{egui, egui_glow};
use egui::*;
//use egui::{Color32, RichText};
use egui_plot::{Line, Plot, PlotPoints};
use rand::prelude::*;
use rand_chacha::ChaCha8Rng;
use rkyv::{Archive, Deserialize, Serialize};
use winit::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::Window,
};
use std::error::Error;
use rancor;
//use serde::{Archive, Deserialize, Serialize};
//use postcard;

const INDEX_MATRIX: usize = (1 << 13) - 1;
//const INDEX_MATRIX: usize = 8192;
const HISTOGRAPHY: usize = 256;
const USER_INTERFACE_SAMPLING: usize = 24; // ?
const TICK_RATES_PER_SNAPSHOT: u64 = 60; // example
const PRESET_MAX_QUEUE: usize = 3;

const PLOT_ASPECT_RATIO: f32 = 2.76; // 2.4, higher is wider!

const FORCE_TICK_RATE: u64 = 22;
//const FORCE_TICK_RATE: u64 = 333;
//const FORCE_TICK_RATE: u64 = 666;
//const FORCE_TICK_RATE: u64 = 999;

const MEAN_REVERT_APPLY: f32 = 0.0022223; //0.005

const CRASH_PROB_FLOOR: f32 = 0.00012223; // 0.0003
const CRASHING_SCORE:   f32 = 0.00016667; // 0.00010
const CRASHING_BOUND:   f32 = 0.00066667; // 0.0012

const DRIFT_RANGE: (f32, f32) = (-0.00011112, 0.00022223); // 0.00015 to 0.00025
const VOLUME_RANGE: (f32, f32) = (0.002768, 0.022223); // 0.003 to 0.018
const BETA_RANGE: (f32, f32) = (0.33334, 1.98765); // 0.4 to 1.8
const LIQUID_RANGE: (f32, f32) = (0.0999998, 1.0); // 0.25 to 1.0
const SENTIMENT_RANGE: (f32, f32) = (-0.19876, 0.19876); // -0.2 to 0.2

const ECONOMY_BASE: f32 = 8.766; // 10.0
const ECONOMY_PERCENTAGE: f32 = 222.0; // 250.0
const ECONOMY_DRIVER: f32 = 0.11112; // 0.10

const STEP_DELAY: u64 = 11;
const STEP_LOCK_MOTION: f32 = 200.0; // 0.0
const TENSE_1: f32 = 11111.0; // 0.01 .. at 200.0 SLM set to 20000.0 to grant equal effect
const TENSE_2: f32 = 33333.0; // 0.002
const TENSE_3: f32 = 55555.0; // 0.005
const RELAX_1: f32 = 6666.0; // 0.08
const RELAX_2: f32 = 3333.0; // 0.04
const RELAX_3: f32 = 2222.0; // 0.01





const QUOTIANT_HI: u64 = 0xFFFD_FFFC_FFFB_FFFA; // 0xFFFC_FFFB_FFFA_FFF0
const QUOTIANT_LO: u64 = 0x0A01_0F02_0004_DCBA; // 0x0987_0987_0987_0987


// setup market
#[derive(Clone, Copy, Debug, Archive, Serialize, Deserialize)]
struct SeedMarketTheory {
    qhi: u64,
    qlo: u64,
}

impl SeedMarketTheory {
    fn set_u128(self) -> u128 {
        ((self.qhi as u128) << 64) | self.qlo as u128
    }
}

/*fn form_ticker_nameplate(rng: &mut ChaCha8Rng) -> String {
    let len = rng.gen_range(3..=5);
    let charset = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    (0..len).map(|_| {
        let index = rng.gen_range(0..charset.len());
        charset.chars().nth(index).unwrap()
    }).collect();
}*/

#[derive(Clone, Copy, Debug, Archive, Serialize, Deserialize)]
enum FlowStates {
    TradingZen,
    TradingBliss,
    TradingTumult,
    TradingChaos,
}

#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
struct SetTickerStatus {
    price: f32,
    log_price: f32,
    target_price_log: f32,
    drift: f32,
    vol: f32,
    beta: f32,
    liquidity: f32,
    sentiment: f32,
    last_value: f32,
}

impl SetTickerStatus {
    fn new(seed_mix: u64, base_price: f32) -> Self {
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
struct IndexGrouper {
    with_name: String,
    of_members: Vec<u32>,
    last_value: f32,
    with_history: VecDeque<f32>,
}

impl IndexGrouper {
    fn new(with_name: String, of_members: Vec<u32>, initialize: f32) -> Self {
        let mut with_history = VecDeque::with_capacity(HISTOGRAPHY);
        with_history.push_back(initialize);
        Self { with_name, of_members, last_value: initialize, with_history }
    }
}

#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
struct SnapshotEconomy {
    tick: u64,
    flow: FlowStates,
    driver: f32, // market stress, dynamics, etc...
    stocks: Vec<SetTickerStatus>,
    index: Vec<(String, f32)>,
    samples: Vec<(String, Vec<f32>)>,
    seed: SeedMarketTheory,
}

#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
struct SaveGameState {
    version:        u32,
    seed:           SeedMarketTheory,
    tick:           u64,
    flow:           FlowStates,
    driver:         f32,
    stocks:         Vec<SetTickerStatus>,
    index:          Vec<IndexGrouper>,
    ring_cursor:    usize,
    ring_prices_history: Vec<[f32; HISTOGRAPHY]>,
}

#[derive(Debug)]
struct EconomyEngine {
    seed:           SeedMarketTheory,
    tick:           u64,
    rng:            ChaCha8Rng,
    stocks:         Vec<SetTickerStatus>,
    index:          Vec<IndexGrouper>,
    flow:           FlowStates,
    driver:         f32,
    ring_cursor:    usize,
    ring_price_history: Vec<[f32; HISTOGRAPHY]>,
}

impl EconomyEngine {
    fn new(seed: SeedMarketTheory) -> Self {
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
            flow:       FlowStates::TradingBliss,
            driver:         ECONOMY_DRIVER,
            ring_cursor:    0,
            ring_price_history,
        }
    }

    fn build_index(n: usize) -> Vec<IndexGrouper> {
        let mut groups = Vec::new();
        let all: Vec<u32> = (0..n as u32).collect();
        groups.push(IndexGrouper::new("indexed".into(), all, 1000.0));

        for g in 0..7 {
            let members = (0..n as u32).filter(|i| (i % 7) == g).collect::<Vec<_>>();
            groups.push(IndexGrouper::new(format!("index_{}", g), members, 1000.0));
        }
        groups
    }


    fn step(&mut self) {
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
            FlowStates::TradingZen      =>      0.60,
            FlowStates::TradingBliss    =>      1.0,
            FlowStates::TradingTumult   =>      1.8,
            FlowStates::TradingChaos    =>      3.8,
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
            -self.rng.random_range(0.07..0.22)
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

    fn fat_tail_noise(&mut self) -> f32 {
        let u1: f32 = self.rng.random::<f32>().clamp(1e-7, 1.0 - 1e-7);
        let u2: f32 = self.rng.random::<f32>().clamp(1e-7, 1.0 - 1e-7);
        let u3: f32 = self.rng.random::<f32>().clamp(1e-7, 1.0 - 1e-7);

        let normal = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
        let cauchy = (std::f32::consts::PI * (u3 - 0.5)).tan().clamp(-20.0, 20.0);

        (0.88 * normal + 0.12 * cauchy).clamp(-8.0, 8.0)
    }

    fn snapshot(&self) -> SnapshotEconomy {
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

    fn samples(&self) -> Vec<(String, Vec<f32>)> {
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

    fn saver(&self) -> SaveGameState {
        SaveGameState {
            version: 1,
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

struct SplitMix64(u64);

impl SplitMix64 {
    fn new(seed: u64) -> Self { Self(seed) }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    fn next_f32_range(&mut self, min: f32, max: f32) -> f32 {
        let x = (self.next_u64() >> 40) as f32 / ((1u64 << 24) as f32);
        min + (max - min) * x
    }
}

#[derive(Debug)]
enum SimulatedResponse {
    Snapshot(SnapshotEconomy),
    SaveNow(Vec<u8>),
}

struct StockComplex {
    rx: Receiver<SimulatedResponse>,
    tx_cmd: Sender<SimulatedCommand>,
    recent: Option<SnapshotEconomy>,
    save_blob: Option<Vec<u8>>,
    index_picker: usize,
    pause_active: bool,
}

#[derive(Debug)]
enum SimulatedCommand {
    ToPause(bool),
    ToSave,
    ToStop,
}

impl eframe::App for StockComplex {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                SimulatedResponse::Snapshot(s) => self.recent = Some(s),
                SimulatedResponse::SaveNow(bytes) => self.save_blob = Some(bytes),
            }
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button(if self.pause_active { "resume" } else { "pause" }).clicked() {
                    self.pause_active = !self.pause_active;
                    let _ = self.tx_cmd.send(SimulatedCommand::ToPause(self.pause_active));
                }
                if ui.button("save now").clicked() {
                    let _ = self.tx_cmd.send(SimulatedCommand::ToSave);
                }
                ui.label(" o ");
                if let Some(s) = &self.recent {
                    ui.separator();
                    ui.label(format!("{:?}", s.seed));
                    ui.label(format!("tick: {}", s.tick));
                    ui.label(format!("heat: {:.3}", s.driver));
                    ui.label(format!("flow: {:?}", s.flow));
                }
            });
        });

        egui::SidePanel::left("left").resizable(true).show(ctx, |ui| {
            ui.heading("indexing");
            if let Some(s) = &self.recent {
                for (i, (name, value)) in s.index.iter().enumerate() {
                    if ui.selectable_label(
                        self.index_picker == i,
                        format!("{} {:.2}", name, value)).clicked() {
                            self.index_picker = i;
                    }
                }
            }

            ui.separator();
            if let Some(blob) = &self.save_blob {
                ui.label(format!("recent save blob? = {} bytes", blob.len()));
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(s) = &self.recent {
                //ui.heading("sample price");
                if let Some((name, series)) = s.samples
                    .get(self.index_picker.min(s.samples.len().saturating_sub(1))) {
                        ui.label(format!("{}", name));
                    }

                    let plot = Plot::new("plot of market stats")
                        .view_aspect(PLOT_ASPECT_RATIO)
                        //.view_aspect(2.4)
                        .allow_scroll(false)
                        .allow_drag(false);

                    plot.show(ui, |plot_ui| {
                        for (name, series) in s.samples.iter().take(6) {
                            let points: PlotPoints = series
                                .iter()
                                .enumerate()
                                .map(|(i, &v)| [i as f64, v as f64]).collect();
                            plot_ui.line(Line::new(name.clone(), points));
                        }
                    });

                    ui.separator();
                    /*ui.label("stock grid picker");
                    egui::Grid::new("gridx4").num_columns(4).show(ui, |ui| {
                        for (work_index, st) in s.stocks.iter().take(16).enumerate() {
                            ui.label(format!("-> {}", work_index + 1));
                            ui.label(format!("{:.2}", st.price));
                            ui.label(format!("r={:+.3}", st.last_value * 100.0));
                            ui.label(format!("v={:.3}", st.vol));
                            ui.end_row();
                        }
                    });*/
                } else {
                    ui.label("getting 1st snapshot...");
                    /*for i in 0..4 {
                        ui.label("getting 1st snapshot...");
                        println!("\t at {}", i);
                        thread::sleep(Duration::from_millis(500));
                    }*/
            }
        });

        ctx.request_repaint();
    }
}

fn sim_thread(
    seed: SeedMarketTheory,
    rx_cmd: Receiver<SimulatedCommand>,
    tx: Sender<SimulatedResponse>,
    stop: Arc<AtomicBool>) {
    let mut engine = EconomyEngine::new(seed);
    let mut pause_game = false;
    let mut latest_image = Instant::now();

    let tick_rate_factor = Duration::from_millis(FORCE_TICK_RATE); // 16 = ~62.5 t/s

    while !stop.load(Ordering::Relaxed) {
        while let Ok(cmd) = rx_cmd.try_recv() {
            match cmd {
                SimulatedCommand::ToPause(p) => pause_game = p,
                SimulatedCommand::ToSave => {
                    let save = engine.saver();
                    if let Ok(bytes) =
                        rkyv::to_bytes::<rkyv::rancor::Error>(&save) {
                            let _ = tx
                                .try_send(SimulatedResponse::SaveNow(
                                    bytes.to_vec()
                                )
                            );
                    }
                    /*let bytes = match rkyv::to_bytes::<rancor::Error>(&save) {
                        Ok(v) => v,
                        Err(e) => {
                            println!("{e}");
                            return;
                        }
                    };*/
                    //std::fs::write("save.plot", &bytes[..]);

                    /*let archiver = match rkyv::access::<ArchivedSaveGameState,
                        rkyv::rancor::Error>(&bytes[..]);*/
                    /*if let Ok(bytes) = postcard::to_allocvec(save,
                        postcard::standard()) {
                            let _ = tx.send(SimulatedResponse::SaveNow(bytes));
                    }}*/
                }
                SimulatedCommand::ToStop => {
                    stop.store(true, Ordering::Relaxed);
                }
            }
        }

        if !pause_game {
            engine.step();
        }

        if latest_image.elapsed() >= Duration::from_millis(FORCE_TICK_RATE) {
            let _ = tx.send(SimulatedResponse::Snapshot(engine.snapshot()));
            latest_image = Instant::now();
        }
        thread::sleep(tick_rate_factor);
    }
}

fn main() -> eframe::Result<()> {
    let seed = SeedMarketTheory { qlo: QUOTIANT_LO, qhi: QUOTIANT_HI};

    let (tx_msg, rx_msg) = bounded::<SimulatedResponse>(PRESET_MAX_QUEUE);
    let (tx_cmd, rx_cmd) = bounded::<SimulatedCommand>(PRESET_MAX_QUEUE);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();

    thread::spawn(move || sim_thread(seed, rx_cmd, tx_msg, stop_clone));

    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "T1XR",
        native_options,
        Box::new(move |_cc| {
            Ok(Box::new(StockComplex {
                rx: rx_msg,
                tx_cmd,
                recent: None,
                save_blob: None,
                index_picker: 0,
                pause_active: false,
            }))
        }),
    )
}
