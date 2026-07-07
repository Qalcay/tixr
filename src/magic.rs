use eframe::egui::Color32;


/*
 * Get rid of Magic Numbers!!
 *  Ahhhhhhhh...
 *
 */

 /*pub const : = ;
 pub const : = ;
 pub const : = ;
 pub const : = ;*/



pub const SHORT_RATE:                       f32 = 0.03;
pub const INFLATION_MODE:                   f32 = 0.022;
pub const OUTPUT_GAP:                       f32 = 0.0011112;
pub const LONG_RATE:                        f32 = 0.038;
pub const YC_0:                             f32 = 0.03;
pub const YC_1:                             f32 = 0.032;
pub const YC_2:                             f32 = 0.034;
pub const YC_3:                             f32 = 0.036;
pub const YC_4:                             f32 = 0.038;
pub const YC_5:                             f32 = 0.04;

pub const SET_TO_ZERO:                      f32 = 0.0;
pub const OFFSET_ZERO:                      f32 = 0.0011112;
pub const MONEY_GROWTH_YEAR_BASELINE:       f32 = 0.05;
pub const MONEY_GROWTH_YEAR_EXTRA:          f32 = 0.03;


// INFLATION -> Macro step
pub const INFLATION_ADJUSTMENT:             f32 = 1.5;
pub const INFLATION_WIGGLE:                 f32 = 0.015;



pub const TAYLOR_RULE_1:                    f32 = 0.5;
pub const TAYLOR_RULE_2:                    f32 = 0.5;
pub const EASE_TO_TAYLOR_TARGET:            f32 = 2.0;



pub const REAL_RATES_COOL_OFF:              f32 = -0.8;
pub const SELF_REVERT_GAP:                  f32 = -0.5;
pub const GAP_NOISE:                        f32 = 0.02;



pub const EXPECT_RATE_REVERT:               f32 = 0.5;



pub const MATURITY_3MO:                     f32 = 0.25;
pub const MATURITY_1YR:                     f32 = 1.0;
pub const MATURITY_2YR:                     f32 = 2.0;
pub const MATURITY_5YR:                     f32 = 5.0;
pub const MATURITY_10Y:                     f32 = 10.0;
pub const MATURITY_30Y:                     f32 = 30.0;




pub const INIT_BASE_VALUE:                  f32 = 1000.0;



pub const WHALE_MULT:                       f64 = 80.0; // 20.0
pub const REG_BROKER_MULT:                  f64 = 0.6667; // 1.0


pub const DRIFT_TO_PERCENT:                 f32 = 0.01;


pub const VOLUME_FRACTION_SCALAR:           f64 = 0.001;
pub const VOLUME_ADD_ONE:                   f64 = 1.0;
pub const VOLUME_TICK_AGGRO:                f64 = 22.0; // 40.0


pub const SOLVENCY_RECOVERY_RATES:          f32 = 0.22;      // (40% def.) percentage rate at which the winners regain their value
pub const SOLVENCY_LOSSES_BUILDUP:          f32 = 2.76;      // (3x def.) each movement swings amount 3x so losses add up faster
pub const SOLVENCY_HEDGING_BLEEDS:          f32 = 0.000125;   // (0.0008) constant base bleed rate so flat stocks eventually fail


pub const VOLUME_MARKET_CRASH:              f64 = 3.1415926535897932; // default 5x, but I want a modest (PI)x to the market
pub const VOLUME_MARKET_NORMAL:             f64 = 1.0;  // normal is just 100% the volume ?


pub const SPAWN_CHANCE_PER_TICK:            f32 = 0.011; // very small number, def. '0.0006'

pub const WHALE_EATS:                       f32 = 0.0041239; // def. 0.005, whale silently earn capitol on calm markets, (|return|<0.5%)
pub const NPC_SPEND_HIGH:                   f64 = 0.1987667; // buys up to 8% (~19.9%)
pub const NPC_SPEND_LOW:                    f64 = 0.001; // buys floor 1%   (0.1%)
pub const NPC_SELLS_HIGH:                   f64 = 0.601;  // sells 50% shares (60.1%)
pub const NPC_SELLS_LOW:                    f64 = 0.011;  // sells 10% shares (1.1%)
