//! # sigma-engine
//!
//! Event-driven market simulation core for **Tixr-sigma** (path A).
//!
//! Module map, roughly bottom-up:
//!
//! | module        | responsibility                                            |
//! |---------------|-----------------------------------------------------------|
//! | [`rng`]       | xoshiro256++ RNG and distributions (no `rand` crate)      |
//! | [`time`]      | sim clock, calendar, exchange sessions                    |
//! | [`queue`]     | calendar queue: O(1) discrete-event scheduler             |
//! | [`persona`]   | perks A-P and their stacking effects                      |
//! | [`macroecon`] | output gap, inflation, Taylor rule, Nelson-Siegel curve   |
//! | [`exchange`]  | per-exchange market factor (earnings x valuation)         |
//! | [`security`]  | closed-form price model, listings, splits, exits          |
//! | [`history`]   | tiered chart history (Hour ... 10-Year, instant to draw)  |
//! | [`save`]      | save container, tamper seal, zero-dependency serialiser   |
//! | [`broker`]    | portfolios, brokers/firms, strategies                     |
//! | [`engine`]    | the event loop that ties it all together                  |
//! | [`protocol`]  | Command / Snapshot messages for the UI thread             |
//! | [`runner`]    | the engine thread and wall-clock pacing                   |
//! | [`console`]   | real-time debug output to the terminal                    |

pub mod broker;
pub mod console;
pub mod engine;
pub mod exchange;
pub mod history;
pub mod macroecon;
pub mod persona;
pub mod protocol;
pub mod queue;
pub mod rng;
pub mod runner;
pub mod save;
pub mod security;
pub mod time;

pub use engine::{Engine, EngineConfig, MAX_ENTITIES, MAX_SECURITIES, TARGET_MAX, TARGET_MIN};
pub use protocol::{Command, Snapshot};
pub use runner::{EngineHandle, RunnerOptions, spawn};
