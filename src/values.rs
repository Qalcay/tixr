use eframe::egui::Color32;

//pub const EVALUATED:                usize = (1 << 13) - 1;
pub const EVALUATED:                usize = (1 << 10) - 1;
//pub const EVALUATED:                usize = (1 << 13) - 1;
pub const EXCHANGES:                usize = 4;


pub const DAYS_YEAR:                u64 = 384; // 256
pub const TICKS_DAY:                u64 = 256; // 64
//pub const TICKS_DAY:                u64 = 16;  // 64
pub const DT:                       f32 = 1.0 / 98304.0; // = 1 / (384 * 64) ??
//pub const DT:                       f32 = 1.0 / 16384.0; // = 1 / (252 * 64) ??
//pub const DT:                       f32 = 1.0 / 2016.0; // = 1 / (252 * 8) ??


pub const DAYS_TRADING:             u64 = 5; // 3
pub const DAYS_REST:                u64 = 2; // 1
pub const LENGTH_WEEK:              u64 = DAYS_TRADING + DAYS_REST;


pub const PREHEAT_YEARS:            u64 = 0;
pub const PREHEAT_TICKS:            u64 = PREHEAT_YEARS * DAYS_YEAR * TICKS_DAY;
pub const DAYS_HISTOGRAPH:          usize = 1280;
pub const LIVE_RING:                usize = 1024;


pub const MU_ANN:                   (f32, f32) = (-0.033, 0.1667);
pub const BETA_RANGE:               (f32, f32) = (0.256, 1.667);
pub const SIGMA_LONG:               (f32, f32) = (0.15, 0.54);
pub const CYCLE_AMPLITUDE:          (f32, f32) = (0.0, 0.111);
pub const CYCLE_PERIODICITY:        (f32, f32) = (1.5, 6.0);
pub const INITIAL_SOLVENCY:         (f32, f32) = (0.667, 1.667);
pub const SHARES_IN_RANGE:          (f64, f64) = (1.987e8, 7.2e9);
pub const LAUNCH_IPO_PRICE:         (f32, f32) = (27.0, 198.0);


pub const KAPPA: f32 = 0.66667;


pub const MARKET_VOLUME:            f32 = 0.16667;
pub const EQUITY_PREMIUM:           f32 = 0.03334;
pub const JUMP_LAMBA_ANN:           f32 = 0.19876;
pub const JUMP_MEAN:                f32 = -0.0667;
pub const JUMP_STD:                 f32 = 0.09876;


pub const OMEGA_GARCH:              f32 = 0.0987;
pub const ALPHA_GARCH:              f32 = 0.0667;
pub const BETA_GARCH:               f32 = 0.8888;
pub const CLAMP_VARIANCE:           (f32, f32) = (9.87e-4, 8.8);


pub const FLOOR_PRICE:              f32 = 0.19876;


pub const RIDE_ZEN:                 (f32, f32, f32, f32) = (0.888, 0.00, 1.0, 0.00);
pub const RIDE_BLISS:               (f32, f32, f32, f32) = (0.777, 0.50, 0.667, 0.05);
pub const RIDE_TUMULT:              (f32, f32, f32, f32) = (1.334, -0.66, 1.98, -0.198);
pub const RIDE_CHAOS:               (f32, f32, f32, f32) = (1.987, -2.67, 8.88, -0.667);


pub const RATE_NEUTRALITY:          f32 = 0.01667; // r* -- ''resting'' policy rate
pub const INFLATION_TARGET:         f32 = 0.01667;
pub const MARKET_PRODUCTIVITY:      f32 = 0.02223; // real growth eats into money printing
pub const INITIAL_MONEY_SUPPLY:     f64 = 0.987e15; // 987_000_000_000_000.0 == 987 Trillion!
pub const CLAMP_RATE_SPENDING:      (f32, f32) = (0.0, 0.198);
pub const CLAMP_RATE_INFLATION:     (f32, f32) = (-0.01, 0.198);
pub const CLAMP_RATE_GAP:           (f32, f32) = (-0.07, 0.07);
pub const TERM_PREMIUM:             f32 = 0.0111; // extra yield for lends 'long' against 'short'


pub const BROKERS:                  usize = 127;
pub const BROKER_ACTION_CHANCE:     f32 = 0.07654321;
pub const BROKER_WALLET_LAUNCH:     (f64, f64) = (2.7e6, 19.2e9);
pub const PLAYER_WALLET_BEGINS:     f64 = 0.777e6;


// trade nudging by log-price Impact * notional / (price * shares * liquidity)
// keep small, allows whales/panics to move markets
pub const PRICE_ORDER_IMPACT:       f32 = 0.6;

pub const TYPE_CURRENCY:            &str = "$";
//pub const TYPE_CURRENCY:            &str = "Э";
//pub const TYPE_CURRENCY:            &str = "Ж";
//pub const TYPE_CURRENCY:            &str = "ɀ";
//pub const TYPE_CURRENCY:            &str = "ɣ";


pub const NAMEPLATE_ALPHABET:       &str = "ABCDEFGHKLMNOPQRSTUVWXYZ123456789"; // remove 0,
//pub const NAMEPLATE_ALPHABET:       &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"; // remove j, o, q, u, w, 0, 1
//pub const NAMEPLATE_ALPHABET:       &str = "ABCDEFGHIKLMNPRSTVXYZ23456789"; // remove j, o, q, u, w, 0, 1
pub const NAMEPLATE_LENGTHS:        [usize; EXCHANGES] = [3, 4, 3, 5]; // per market preference


pub const PLOT_ASPECT_RATIO:        f32 = 3.111;
pub const PRESET_MAX_QUEUE:         usize = 3;
pub const FORCE_SNAPSHOT_MS:        u64 = 59;
//pub const FORCE_SNAPSHOT_MS:        f64 = 59.998;
pub const STEP_BUDGET_MS:           u64 = 16;
//pub const STEP_BUDGET_MS:           f64 = 16.667;


pub const QUOTIENT_HIGH:            u64 = 0xF987_E789_D987_C789;
pub const QUOTIENT_LOW:             u64 = 0x1110_1010_0101_1001;


pub const COLOR_LINE_ASPECT: Color32 = Color32::from_rgb(11, 234, 128);
pub const COLOR_UPWARDS: Color32 = Color32::from_rgb(45, 202, 117);
pub const COLOR_DOWNWARDS: Color32 = Color32::from_rgb(234, 77, 66);



pub const SPLIT_MIX_HIGH:           u64 = 0xE198_C891_E198_C891;
//pub const SPLIT_MIX_HIGH:           u64 = 0xDEAD_BEEF_CAFE_F00D;

//pub const NEXT_U64:                 u64 = 0x517C_C1B7_2722_0A95; // high-entropy odd number, purely arbitrary
//pub const NEXT_U64:                 u64 = 0x98AB_87BC_76CD_65DE;
pub const NEXT_U64:                 u64 = 0x9E37_79B9_7F4A_7C15; // <- [sqrt(5) - 1 / 2] 'golden ratio mean'

//pub const Z_STEP_1:                 u64 = 0x9E37_79B1_85EB_CA87; // xxHash tweaked Golden Ratio as odd value
//pub const Z_STEP_1:                 u64 = 0xAC77_BE55_CA33_EC11; <- my dumb val
pub const Z_STEP_1:                 u64 = 0xBF58_476D_1CE4_E5B9;

//pub const Z_STEP_2:                 u64 = 0xC2B2_AE3D_27D4_EB4F; // xxHash second mult...
//pub const Z_STEP_2:                 u64 = 0x6667_981A_BCDE_F198; <- my dumb val
pub const Z_STEP_2:                 u64 = 0x94D0_49BB_1331_11EB;

//pub const BIT_SHIFT_1:              u64 = 31; // xxHash
pub const BIT_SHIFT_1:              u64 = 30; // used in SplitMix64 by Vigno for his consts
//pub const BIT_SHIFT_2:              u64 = 33; // xxHash
pub const BIT_SHIFT_2:              u64 = 27;
//pub const BIT_SHIFT_3:              u64 = 33; // xxHash
pub const BIT_SHIFT_3:              u64 = 31;


//pub const SCRAMBLING_SALT:            u64 = 0xB7E1_5162_8AED_2A6B;
pub const SCRAMBLING_SALT:          u64 = 0xA5A5_5A5A_F0F0_0F0F;


/*pub const : = ;
pub const : = ;*/



pub const WINDOW_SCALE_WIDTHS: f32 = 1800.0;
pub const WINDOW_SCALE_HEIGHT: f32 = 900.0;
