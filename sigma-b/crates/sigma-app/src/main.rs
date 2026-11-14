//! Tixr-sigma desktop app (path B).
//!
//! Two threads:
//! * the **engine** thread (`sigma_core::runtime`) owns all market state,
//! * this **display** thread owns only what it draws.
//! They share nothing and talk through channels (see `runtime.rs`).
//!
//! Debug output goes to the console that launched the app, not to the
//! window: run from a terminal and set `SIGMA_LOG=debug` for more.

mod app;
mod fmt;
mod theme;
mod ui;

use eframe::egui;
use sigma_core::Config;

fn main() -> eframe::Result<()> {
    sigma_core::log::init_from_env();
    let seed = std::env::args()
        .nth(1)
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(Config::default().seed);
    sigma_core::slog!(sigma_core::log::Level::Info, "app", "Tixr-sigma (path B) starting, seed {seed}");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Tixr-sigma")
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([980.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Tixr-sigma",
        options,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            let cfg = Config { seed, ..Config::default() };
            let engine = sigma_core::runtime::spawn(cfg, move || ctx.request_repaint());
            Ok(Box::new(app::SigmaApp::new(engine, seed)))
        }),
    )
}
