//! Headless calibration run: `cargo run -p sigma-engine --release --example probe -- [days] [seed]`
//!
//! Prints one line per 50 sim days so long-run behaviour (population,
//! defaults, index levels, broker turnover) can be eyeballed without the UI.
use sigma_engine::console::{self, Level};
use sigma_engine::time::SECS_PER_DAY;
use sigma_engine::{Engine, EngineConfig};

fn main() {
    let mut args = std::env::args().skip(1);
    let days: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(1_000);
    let seed: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(1);
    console::set_level(Level::Warn);
    let started = std::time::Instant::now();
    let mut config = EngineConfig { seed, warmup: std::env::var("NOWARM").is_err(), ..Default::default() };
    if let Some(n) = std::env::var("BROKERS").ok().and_then(|v| v.parse::<usize>().ok()) {
        config.brokers = n * 85 / 100;
        config.firms = n - config.brokers;
    }
    let mut e = Engine::new(config);
    eprintln!(
        "init {:.2}s events {} ticks {} trades {}",
        started.elapsed().as_secs_f64(),
        e.stats.events,
        e.stats.sec_ticks,
        e.stats.trades
    );
    e.tick_density = 0.001;
    let t0 = e.now;
    for d in 1..=days {
        e.advance_to(t0 + d as f64 * SECS_PER_DAY, None);
        if std::env::var("EVERY").is_ok() {
            eprintln!(
                "d{d} t={:.2}s events {} ticks {} closed {} decisions {} trades {} positions {}",
                started.elapsed().as_secs_f64(),
                e.stats.events,
                e.stats.sec_ticks,
                e.stats.closed_ticks,
                e.stats.broker_decisions,
                e.stats.trades,
                e.brokers().map(|b| b.port.positions.len()).sum::<usize>()
            );
        }
        if d % 50 == 0 || d == days {
            let s = &e.stats;
            let idx: Vec<String> = e.exchanges.iter().map(|x| format!("{:.0}", x.index_level())).collect();
            let max_price = e.live_ids().map(|id| e.security(id).unwrap().price).fold(0.0, f64::max);
            println!(
                "day {d:5} live {:6} K {:6.0} ipo {:5} bk {:4} penny {:4} acq {:4} splits {:3} | brokers {:5} firms {:4} broke {:4} rescues {:3} | {:<10} 3M {:5.2}% 10Y {:5.2}% pi {:5.2}% | idx {} | max px {:.0} | {}",
                e.live_count(),
                e.carrying_capacity,
                s.ipos,
                s.bankruptcies,
                s.penny_delistings,
                s.acquisitions,
                s.splits,
                e.brokers_alive() - e.firms_alive(),
                e.firms_alive(),
                s.brokers_broke,
                s.rescues,
                e.regime().label(),
                e.macro_state.curve.yield_at(0.25) * 100.0,
                e.macro_state.y10() * 100.0,
                e.macro_state.inflation * 100.0,
                idx.join("/"),
                max_price,
                match e.health_check() {
                    Ok(()) => "ok".to_string(),
                    Err(x) => x,
                },
            );
        }
    }
    eprintln!("{} days in {:.1}s ({} events)", days, started.elapsed().as_secs_f64(), e.stats.events);
}
