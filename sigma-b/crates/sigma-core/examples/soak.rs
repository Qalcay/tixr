//! Headless soak run: simulate many days as fast as possible and print a
//! report every 30 simulated days. This is how we prove the engine no
//! longer explodes after ~300 days.
//!
//!     cargo run -p sigma-core --release --example soak -- 1000 [seed] [small]

use sigma_core::clock::SECONDS_PER_DAY;
use sigma_core::{Config, Engine};
use std::time::Instant;

fn main() {
    sigma_core::log::init_from_env();
    let args: Vec<String> = std::env::args().collect();
    let days: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1000);
    let seed: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(7);
    let small = args.iter().any(|a| a == "small");
    let cfg = if small { Config::small(seed) } else { Config { seed, ..Config::default() } };

    let t0 = Instant::now();
    let mut e = Engine::new(cfg);
    println!("built in {:.2}s, {} live", t0.elapsed().as_secs_f64(), e.live());
    let step = 300.0; // 5 sim minutes
    let steps_per_day = (SECONDS_PER_DAY / step) as u32;
    let start = Instant::now();
    let (mut lo, mut hi) = (usize::MAX, 0);
    for day in 1..=days {
        for _ in 0..steps_per_day {
            let samples = e.live() / 12;
            e.advance(step, samples);
        }
        lo = lo.min(e.live());
        hi = hi.max(e.live());
        if day % 30 == 0 || day == days {
            if let Err(err) = e.audit() {
                panic!("audit failed on day {day}: {err}");
            }
            println!("[{:>6.1}s] {}", start.elapsed().as_secs_f64(), e.status_line());
        }
    }
    println!(
        "done: {days} days in {:.1}s, population range {lo}..{hi}, non-finite events {}",
        start.elapsed().as_secs_f64(),
        e.non_finite_events()
    );
}
