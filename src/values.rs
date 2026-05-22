use eframe::egui::Color32;


pub const INDEX_MATRIX: usize = (1 << 13) - 1;
//pub const INDEX_MATRIX: usize = 24;
//const INDEX_MATRIX: usize = 8192;
//const INDEX_MATRIX: usize = (1 << 13) - 1;

pub const HISTOGRAPHY: usize = (1 << 13); // 2**13 = 8192, cannot use 2^13 it is bitwise op
//const HISTOGRAPHY: usize = (1 << 14);

pub const USER_INTERFACE_SAMPLING: usize = 24; // ?
pub const TICK_RATES_PER_SNAPSHOT: u64 = 60; // example
pub const PRESET_MAX_QUEUE: usize = 3;
pub const INDEX_TO_PRINTER: usize = 64;

pub const PLOT_ASPECT_RATIO: f32 = 2.92; // 2.4, higher is wider!

// EconomyEngine
pub const FORCE_TICK_RATE: u64 = 59;
//const FORCE_TICK_RATE: u64 = 333;
//const FORCE_TICK_RATE: u64 = 666;
//const FORCE_TICK_RATE: u64 = 999;
pub const YEAR_OF_TICK_RATE: usize = HISTOGRAPHY / 4;

//
pub const MEAN_REVERT_APPLY: f32 = 0.0022223; //0.005

//
pub const CRASH_PROB_FLOOR: f32 = 0.00012223; // 0.0003
pub const CRASHING_SCORE:   f32 = 0.00016667; // 0.00010
pub const CRASHING_BOUND:   f32 = 0.00066667; // 0.0012

// .. SetTickerStatus
pub const DRIFT_RANGE: (f32, f32) = (-0.00011112, 0.00022223); // 0.00015 .. 0.00025
pub const VOLUME_RANGE: (f32, f32) = (0.002768, 0.022223); // 0.003 .. 0.018
pub const BETA_RANGE: (f32, f32) = (0.33334, 1.98765); // 0.4 .. 1.8
pub const LIQUID_RANGE: (f32, f32) = (0.0999998, 1.0); // 0.25 .. 1.0
pub const SENTIMENT_RANGE: (f32, f32) = (-0.19876, 0.19876); // -0.2 .. 0.2

pub const ECONOMY_BASE: f32 = 24.898; // 10.0
pub const ECONOMY_PERCENTAGE: f32 = 234.0; // 250.0
pub const ECONOMY_DRIVER: f32 = 0.12345; // 0.10


// .. FlowStates
pub const STEP_DELAY: u64 = 33;
pub const STEP_DELAY_F32: f32 = 11.0;
pub const STEP_LOCK_MOTION: f32 = 222.0; // 0.0
pub const TENSE_1: f32 = 10001.0; // 0.01 .. at 200.0 SLM set to 20000.0 to grant equal effect
pub const TENSE_2: f32 = 30003.0; // 0.002
pub const TENSE_3: f32 = 50005.0; // 0.005
pub const RELAX_1: f32 = 6006.0; // 0.08
pub const RELAX_2: f32 = 3003.0; // 0.04
pub const RELAX_3: f32 = 2002.0; // 0.01
pub const FLOW_ZEN: f32 = 0.50001; // 0.60
pub const FLOW_BLISS: f32 = 0.66667; // 1.0
pub const FLOW_TUMULT: f32 = 1.3334; // 1.8
pub const FLOW_CHAOS: f32 = 2.9998; // 3.8



pub const NAMEPLATE_MIN: usize = 3;
pub const NAMEPLATE_MAX: usize = 6;

pub const STARTING_BALANCE: f64 = 10000.0;
pub const ACCOUNT_BALANCE: f64 = 0.0;
pub const PAYOUT_DECAY_RATE: f32 = 1.0 / HISTOGRAPHY as f32;

pub const QUOTIANT_HI: u64 = 0xFFFC_FFFB_FFFA_FFF0; // 0xFFFC_FFFB_FFFA_FFF0
pub const QUOTIANT_LO: u64 = 0x987_0987_0987_0987; // 0x0987_0987_0987_0987

pub const SAVING_KEY_USED_0: u8 = 0x5A;
pub const SAVING_KEY_USED_1: u8 = 0x77;
pub const SAVING_KEY_USED_2: u8 = 0x9B;





pub const COLOR_LINE_ACCENT: Color32 = Color32::from_rgb(11, 234, 127);
