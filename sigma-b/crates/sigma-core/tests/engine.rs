//! Integration tests: drive the whole engine through its public API on a
//! small market (about 1,000 securities) so the suite runs in seconds.

use sigma_core::clock::SECONDS_PER_DAY;
use sigma_core::protocol::Command;
use sigma_core::{Config, Engine};

/// Run `days` simulated days in 5-minute steps.
fn run(e: &mut Engine, days: u32) {
    let steps = (days as f64 * SECONDS_PER_DAY / 300.0) as u32;
    for _ in 0..steps {
        let samples = e.live() / 12;
        e.advance(300.0, samples);
    }
}

/// Advance until the market is open.
fn until_open(e: &mut Engine) {
    for _ in 0..2_000 {
        if e.is_open() {
            return;
        }
        e.advance(300.0, 0);
    }
    panic!("market never opened");
}

#[test]
fn a_year_stays_finite_and_in_band() {
    let mut e = Engine::new(Config::small(11));
    for _ in 0..12 {
        run(&mut e, 30);
        e.audit().expect("audit");
    }
    let (lo, hi) = e.cfg.band;
    assert!(e.live() >= lo && e.live() <= hi, "live {} outside {lo}..{hi}", e.live());
    assert_eq!(e.non_finite_events(), 0);
}

#[test]
fn same_seed_same_market() {
    let mut a = Engine::new(Config::small(5));
    let mut b = Engine::new(Config::small(5));
    run(&mut a, 20);
    run(&mut b, 20);
    assert_eq!(a.status_line(), b.status_line());
    let pa: Vec<f64> = a.securities().map(|s| s.price).collect();
    let pb: Vec<f64> = b.securities().map(|s| s.price).collect();
    assert_eq!(pa, pb);
}

#[test]
fn different_seeds_differ() {
    let mut a = Engine::new(Config::small(1));
    let mut b = Engine::new(Config::small(2));
    run(&mut a, 5);
    run(&mut b, 5);
    assert_ne!(a.status_line(), b.status_line());
}

#[test]
fn population_recovers_from_below_the_band() {
    let cfg = Config { initial_population: 300, ..Config::small(3) };
    let mut e = Engine::new(cfg);
    assert!(e.live() < 512);
    run(&mut e, 200);
    assert!(e.live() >= 512, "population only reached {}", e.live());
}

#[test]
fn player_round_trip() {
    let mut e = Engine::new(Config::small(9));
    until_open(&mut e);
    let id = e.securities().next().unwrap().id;
    let start = e.player().cash;
    e.apply(Command::Buy { id, notional: 50_000.0 });
    assert!(e.player().positions.contains_key(&id), "buy did not create a position");
    assert!(e.player().cash < start - 49_000.0);
    e.apply(Command::Sell { id, fraction: 1.0 });
    assert!(e.player().positions.is_empty());
    // Round trip costs fees and some price impact, never more than ~2%.
    let lost = start - e.player().cash;
    assert!(lost > 0.0 && lost < 1_000.0, "round trip cost {lost}");
}

#[test]
fn ids_never_alias_after_slot_reuse() {
    let mut e = Engine::new(Config::small(21));
    run(&mut e, 120);
    // Every live security's id must resolve to itself, and a stale id from a
    // reused slot must not resolve at all.
    for s in e.securities() {
        assert_eq!(e.security(s.id).map(|x| x.id), Some(s.id));
        if s.id.generation() > 0 {
            let stale = sigma_core::security::SecurityId::new(s.id.slot() as u16, s.id.generation() - 1);
            assert!(e.security(stale).is_none());
        }
    }
}

#[test]
fn frames_are_well_formed() {
    let mut e = Engine::new(Config::small(4));
    run(&mut e, 2);
    e.rebuild_table();
    let f = e.frame(true);
    let table = f.table.expect("table requested");
    assert_eq!(table.len(), e.live());
    assert_eq!(f.indices.len(), 5);
    assert!(f.drivers.curve.iter().all(|y| y.is_finite()));
    assert!(f.crawl.iter().any(|row| !row.is_empty()));
}
