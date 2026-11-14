//! Tixr-sigma desktop app.
//!
//! Two threads, message passing only:
//! * **Engine thread** (`sigma_engine::runner`): owns the market, advances it
//!   against the wall clock, sends a `Snapshot` about every 100 ms.
//! * **Display thread** (this one; GUI toolkits want the main thread): draws
//!   the latest snapshot and sends `Command`s back.
//!
//! Debug output is written by the engine thread to this process's terminal
//! (stderr), so run it from a console to watch it live:
//!
//! ```text
//! cargo run --release -p sigma-app -- --seed 42 --log debug
//! ```

mod app;
mod chart;
mod format;
mod panels;
mod theme;
mod ticker;
mod windows;

use eframe::egui;
use sigma_engine::console::{self, ALL_LEVELS, Level};
use sigma_engine::{EngineConfig, protocol::Command};

struct Args {
    config: EngineConfig,
    log: Level,
}

fn parse_args() -> Result<Args, String> {
    let mut config = EngineConfig::default();
    let mut log = Level::Info;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--seed" => config.seed = value("--seed")?.parse().map_err(|_| "seed must be an integer")?,
            "--securities" => {
                config.initial_securities =
                    value("--securities")?.parse().map_err(|_| "securities must be an integer")?
            }
            "--log" => {
                let v = value("--log")?;
                log = ALL_LEVELS.into_iter().find(|l| l.label() == v).ok_or(format!("unknown log level {v}"))?;
            }
            "-h" | "--help" => {
                println!("tixr-sigma [--seed N] [--securities N] [--log off|warn|info|debug|trace]");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other} (try --help)")),
        }
    }
    Ok(Args { config, log })
}

fn main() -> eframe::Result<()> {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };
    console::set_level(args.log);
    eprintln!("Tixr-sigma: debug console (level {}). Change it in Settings.", args.log.label());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Tixr-sigma")
            .with_inner_size([1480.0, 900.0])
            .with_min_inner_size([1100.0, 680.0]),
        ..Default::default()
    };
    let config = args.config;
    let log = args.log;
    eframe::run_native(
        "Tixr-sigma",
        options,
        Box::new(move |cc| {
            // Hand the engine a way to wake the UI when a snapshot lands.
            let ctx = cc.egui_ctx.clone();
            let handle = sigma_engine::spawn(config, move || ctx.request_repaint());
            handle.send(Command::SetLogLevel(log));
            Ok(Box::new(app::App::new(handle)))
        }),
    )
}
