//! Long-horizon stability tests: the alpha drifted into inf/NaN after roughly
//! 300 sim days. These run the full default market headless.
//!
//! The price model is exact for any update gap, so the print density is
//! turned down to keep the tests fast without changing market statistics.
//!
//! * `cargo test -p sigma-engine --release` runs the 400-day test.
//! * `cargo test -p sigma-engine --release -- --ignored` adds a 10-year soak.

use sigma_engine::console::{self, Level};
use sigma_engine::time::SECS_PER_DAY;
use sigma_engine::{Engine, EngineConfig, MAX_SECURITIES, TARGET_MAX, TARGET_MIN};

fn run(days: u64, seed: u64) -> Engine {
    console::set_level(Level::Off);
    let mut e = Engine::new(EngineConfig { seed, ..Default::default() });
    e.tick_density = 0.001;
    let start = e.now;
    for d in 1..=days {
        e.advance_to(start + d as f64 * SECS_PER_DAY, None);
        if d % 10 == 0 {
            if let Err(err) = e.health_check() {
                panic!("day {d}: {err}");
            }
            let n = e.live_count() as f64;
            assert!((TARGET_MIN..=TARGET_MAX).contains(&n), "day {d}: population {n} left the band");
            assert!(e.live_count() <= MAX_SECURITIES);
            for ex in &e.exchanges {
                let idx = ex.index_level();
                assert!(idx > 100.0 && idx < 20_000.0, "day {d}: {} index {idx}", ex.spec.name);
            }
        }
    }
    e
}

#[test]
fn four_hundred_days_stay_finite_and_in_band() {
    let e = run(400, 11);
    assert_eq!(e.stats.repairs, 0, "non-finite values had to be repaired");
    // The life cycle is actually turning over.
    assert!(e.stats.ipos > 100, "ipos {}", e.stats.ipos);
    assert!(e.stats.bankruptcies + e.stats.acquisitions > 100);
}

#[test]
#[ignore = "10-year soak; run with --ignored"]
fn ten_year_soak() {
    let e = run(3_650, 12);
    assert_eq!(e.stats.repairs, 0);
}
