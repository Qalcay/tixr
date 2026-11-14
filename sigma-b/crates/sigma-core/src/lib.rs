//! # sigma-core
//!
//! The Tixr-sigma market engine, path B: **budgeted random sampling with
//! exact lazy catch-up**. This crate has zero dependencies and knows
//! nothing about the UI; the app crate talks to it only through the
//! message types in [`protocol`], via the thread in [`runtime`].
//!
//! Module map, roughly bottom-up:
//! * [`rng`]: deterministic random numbers.
//! * [`clock`]: calendar time, trading sessions, variance time.
//! * [`sampler`]: Fenwick-tree weighted sampler over 2^16 slots.
//! * [`persona`]: perks A to P.
//! * [`macro_econ`]: economy, Taylor rule, Nelson-Siegel yield curve.
//! * [`market`]: sectors, exchanges, sentiment factors.
//! * [`security`]: one stock's state, valuation and exact catch-up.
//! * [`portfolio`], [`broker`]: market participants.
//! * [`engine`]: ties it together into ticks.
//! * [`protocol`], [`runtime`]: the engine thread and its messages.
//! * [`log`]: console output.

pub mod broker;
pub mod clock;
pub mod engine;
pub mod log;
pub mod macro_econ;
pub mod market;
pub mod names;
pub mod persona;
pub mod portfolio;
pub mod protocol;
pub mod rng;
pub mod runtime;
pub mod sampler;
pub mod security;

pub use engine::{Config, Engine};
