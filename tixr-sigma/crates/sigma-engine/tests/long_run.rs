//! Long-horizon stability tests: the alpha drifted into inf/NaN after roughly
//! 300 sim days. These run the market headless.
//!
//! The price model is exact for any update gap, so the print density is
//! turned down to keep the tests fast without changing market statistics.
//! Broker decisions cannot be thinned the same way (they change the market),
//! so the long runs use a smaller broker population and a separate, shorter
//! test covers the full default population.
//!
//! * `cargo test -p sigma-engine --release` runs the 400-day tests.
//! * `cargo test -p sigma-engine --release -- --ignored` adds a 10-year soak.

use sigma_engine::broker::{MAX_FIRMS, MAX_PLAIN_BROKERS};
use sigma_engine::console::{self, Level};
use sigma_engine::time::SECS_PER_DAY;
use sigma_engine::{Engine, EngineConfig, MAX_ENTITIES, MAX_SECURITIES, TARGET_MIN};

fn run(config: EngineConfig, days: u64) -> Engine {
    console::set_level(Level::Off);
    let mut e = Engine::new(config);
    e.tick_density = 0.001;
    let start = e.now;
    for d in 1..=days {
        e.advance_to(start + d as f64 * SECS_PER_DAY, None);
        if d % 10 == 0 {
            if let Err(err) = e.health_check() {
                panic!("day {d}: {err}");
            }
            let n = e.live_count();
            // "Around 8,000 without ever really going lower", hard cap 2^14.
            assert!(n as f64 >= TARGET_MIN * 0.985, "day {d}: population {n} fell under the floor");
            assert!(n <= MAX_SECURITIES, "day {d}: population {n} above the cap");
            assert!(e.firms_alive() <= MAX_FIRMS && e.brokers_alive() - e.firms_alive() <= MAX_PLAIN_BROKERS);
            assert!(n + e.brokers_alive() <= MAX_ENTITIES);
            for ex in &e.exchanges {
                let idx = ex.index_level();
                assert!(idx > 50.0 && idx < 60_000.0, "day {d}: {} index {idx}", ex.spec.name);
            }
        }
    }
    e
}

fn light(seed: u64) -> EngineConfig {
    EngineConfig { seed, brokers: 600, firms: 100, ..Default::default() }
}

#[test]
fn four_hundred_days_stay_finite_and_in_band() {
    let e = run(light(11), 400);
    assert_eq!(e.stats.repairs, 0, "non-finite values had to be repaired");
    // The life cycle is actually turning over.
    assert!(e.stats.ipos > 50, "ipos {}", e.stats.ipos);
    assert!(e.stats.bankruptcies + e.stats.acquisitions > 50);
}

#[test]
fn full_population_runs_a_quarter() {
    let e = run(EngineConfig { seed: 13, ..Default::default() }, 90);
    assert_eq!(e.stats.repairs, 0);
    assert!(e.brokers_alive() > 4_000, "participants {}", e.brokers_alive());
    assert!(e.stats.trades > 10_000, "trades {}", e.stats.trades);
}

#[test]
#[ignore = "10-year soak; run with --ignored"]
fn ten_year_soak() {
    let e = run(light(12), 3_650);
    assert_eq!(e.stats.repairs, 0);
}
