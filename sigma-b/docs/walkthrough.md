# Code Walkthrough (path B)

A guided tour of `sigma-b`, in the order that makes the most sense to read
it, with notes on why each choice was made and how a professional team would
handle the same code. Read alongside the source; every file also starts with
a doc comment explaining its role.

---

## 0. Project structure: a two-crate workspace

```
sigma-b/Cargo.toml          [workspace] with members sigma-core and sigma-app
crates/sigma-core           the engine, a library with zero dependencies
crates/sigma-app            the egui binary, depends on sigma-core + eframe
```

**Why split?** Three reasons a team would care about:

1. **The boundary is enforced by the compiler.** `sigma-core` cannot import
   egui, so engine code physically cannot reach into UI state. In a single
   crate that rule lives only in people's heads.
2. **Fast tests.** `cargo test -p sigma-core` compiles no GUI code at all.
3. **Reuse.** The headless soak run (`examples/soak.rs`) and any future
   server, CLI or benchmark use the same engine.

The alternative, one crate with `src/engine/` and `src/ui/` folders, is fine
for small projects (tixr alpha was one). Workspaces start paying off once
the engine is worth testing on its own, which it is here.

`[workspace.package]` shares the version and edition so the two crates never
drift. `[profile.dev.package."*"] opt-level = 2` optimises *dependencies* in
debug builds: egui in debug mode is slow, but our own code stays quick to
compile and easy to step through.

---

## 1. `rng.rs`: owning our randomness

`Rng` is xoshiro256++, seeded through SplitMix64. About 60 lines replace two
crates (`rand`, `rand_chacha`).

* `normal()` uses the Marsaglia polar method and caches the spare value.
* `below(n)` uses Lemire's multiply-shift instead of `x % n`, which is biased.
* `poisson(mean)` switches from Knuth's method to a normal approximation above
  a mean of 30.
* `fork(salt)` derives independent streams. The engine uses separate streams
  for prices, life cycle and brokers, so changing how many price updates run
  per tick does not change which companies IPO.

**Team practice.** Hand-rolling an RNG is usually a red flag in review. It is
defensible here because the algorithm is a published, well-known one, the
code is short, and the tests check reproducibility and the first two moments
of the normal. If statistical quality ever mattered more (cryptography,
scientific publication), you would go back to a vetted crate.

---

## 2. `clock.rs`: two clocks

Calendar time `t` (seconds) and variance time `v` (variance years). Key
functions: `SessionMode::is_open`, `next_boundary`, `next_close`,
`variance_rate`, `variance_seconds_per_year`.

**Naming.** Units are spelled out where ambiguity would hurt:
`SECONDS_PER_DAY`, `REAL_SECONDS_PER_SIM_DAY`, `dt_sec`, `dt` in years. A
classic bug class is mixing seconds and years; a stronger fix is newtypes
(`struct SimSeconds(f64)`), which a team might adopt later.

---

## 3. `sampler.rs`: the Fenwick tree

`WeightedSampler::set(slot, w)`, `total()`, `sample(u)` and `rebuild()`.

* `set` walks up the tree adding the delta: `i += i & i.wrapping_neg()`
  (add the lowest set bit).
* `sample` walks down from the root in 16 halving steps, subtracting block
  sums. It then steps back over zero-weight slots in case floating-point
  error lands it on one.
* `rebuild` recomputes everything once per simulated day, because thousands
  of `+= delta` updates accumulate rounding error.

**Testing habit.** `samples_follow_weights` draws 100,000 samples and checks
frequencies within 1 percentage point. Statistical tests need a fixed seed
(so they are deterministic) and a tolerance wide enough to never flake but
tight enough to catch a real bug. Pick tolerances from the expected standard
error, not by trial and error.

---

## 4. `persona.rs`: perks A to P

`Perk` (a `#[repr(u8)]` enum, so the discriminant is the bit index),
`PerkSet(u16)`, `LENGTH_WEIGHTS`, `PerkSet::draw`, `Modifiers`.

* `draw` picks a length from the table, then letters by weighted sampling
  without replacement, zeroing a letter's conflict partner once chosen.
* `modifiers` folds every perk's nudges into log-space sums and applies the
  `tanh` soft cap in `finish()`.

**Why a newtype `PerkSet(u16)` instead of `Vec<Perk>`?** Two bytes instead of
24 plus a heap allocation, `Copy`, hashable, and set operations are bitwise.
The newtype also gives the raw `u16` a meaning, so it cannot be confused with
any other number.

**Testing habit.** The spec ("median 2-3, average 4-7, rare 11-12") became a
test, `length_distribution_matches_spec`. When a requirement is a number,
encode it as a test so it survives future tuning.

---

## 5. `macro_econ.rs`: the economy

`ou_step` (exact OU transition), `norm_cdf`, `YieldCurve` (Nelson-Siegel),
`Macro::step`.

Read `ou_step` closely: it is the building block of half the engine. Note the
`kappa * dt < 1e-8` branch, the Brownian-motion limit that avoids dividing
by almost zero.

**Defence in depth.** Each variable is mean-reverting (the real stability
mechanism) *and* clamped after the step (the safety net). In review, clamps
alone are a smell, because they hide bugs; a restoring force plus a clamp
is a good pattern because the clamp should almost never fire.

---

## 6. `market.rs`: sectors, exchanges, factors

Static tables via `match` (sector profiles, persona affinities), exchange
tiers, and `Factors` (the 12 sentiment OU processes).

Data tables in code are fine at this size and are checked by the compiler. A
team might move them to a config file once designers want to tune them
without recompiling.

---

## 7. `security.rs`: one stock

This is the most important file. In reading order:

1. `SecurityId`: slot in the low 16 bits, generation in the high 16. Fixes
   alpha's "respawned stock inherits holdings" bug by construction.
2. `Ticker`: five inline bytes, `Copy`, no heap allocation per row.
3. `Globals`: the market state a stock reads when it prices itself. `Copy`,
   so the engine can keep snapshots at each daily close.
4. `Security::list`: draws parameters from sector and persona, then solves
   for the earnings level that makes the model price equal the offer price.
5. `advance`: the exact catch-up step (growth, earnings, jumps, volatility,
   mispricing, dividends), then `reprice`, then `hazards`.
6. `record_close`: history, the $1 listing rule, 10-for-1 splits.
7. `forecast_1y`: model expected price and 90% range.

**Rust idiom: `Option::take` to dodge the borrow checker.** In
`engine.rs::update_slot` the engine needs `&mut Security` while also reading
`&self.brokers` and `&self.globals`. Borrowing `self.slots[slot]` mutably
would lock all of `self`. Instead the security is moved out with
`self.slots[slot].take()`, updated, and put back. It costs a memcpy and keeps
the code free of `RefCell` or index gymnastics.

---

## 8. `portfolio.rs` and `broker.rs`

`Portfolio` holds cash and a `BTreeMap<SecurityId, Position>`. Positions
count **base shares** (pre-split) and keep a dividend anchor.

**Determinism bug caught by a test.** The first version used `HashMap`. The
integration test `same_seed_same_market` failed: std's `HashMap` uses a
random hash seed per process, so iterating positions visited them in a
different order each run, which changed the order of trades and therefore
every price. Switching to `BTreeMap`/`BTreeSet` (ordered) fixed it. Rule of
thumb: if iteration order can affect results, do not iterate a `HashMap`.

`Broker::decide` is a **pure function** of its inputs plus the RNG: it
returns `Intent`s and never mutates the market. The engine executes them.
That separation is what makes `value_broker_buys_the_cheapest` a five-line
test.

---

## 9. `engine.rs`: the tick

Read `advance` first (it is numbered 1 to 6 in comments), then
`update_slot`, then `on_close`, `lifecycle`, `list`, `retire`, then the
broker and player sections, then `frame`.

Things worth noticing:

* **Index divisors.** `IndexBook::rebalance` adjusts the divisor when members
  join or leave so the index level does not jump; `mark` lets price moves
  change the level. Real index providers do exactly this.
* **Population controller.** `births_per_day` is
  `exits_per_day * exp(2 * (1 - N/target)) * ipo_window + rescue`. It is a
  feedback controller: the observed exit rate sets the baseline, the
  exponential pushes toward the target, sentiment opens and closes the IPO
  window, and a rescue term refills the band quickly if the population ever
  falls below 8,192. Crowding adds exit hazard above the target.
* **Settlement by anchors.** Dividends and exits are settled when a holder
  next looks, not by iterating holders on every event.

**Code review note.** `engine.rs` is the largest file (~1,600 lines). In a
team this would be split (for example `engine/tick.rs`, `engine/life.rs`,
`engine/brokers.rs`, `engine/player.rs`, `engine/frame.rs`) once the design
settles. Keeping it in one file during a prototype phase is a reasonable
trade-off, and a reviewer would likely ask for a follow-up issue to split it.

---

## 10. `protocol.rs` and `runtime.rs`: the two threads

`protocol.rs` is the entire contract between engine and UI: `Command` going
in, `Frame` coming out. Everything in a `Frame` is owned data. The big table
is an `Arc<Vec<Row>>` rebuilt a few times a second; `Row` is `Copy` and 40
bytes, so 32,768 rows are about 1.3 MB.

`runtime.rs` runs the loop: drain commands, measure real time, advance the
engine, publish a frame every 100 ms, print a status line every 5 s, sleep
until the next 33 ms tick. `EngineHandle::drop` sends `Shutdown` and joins
the thread, so closing the window cleanly stops the engine (RAII).

---

## 11. `sigma-app`: the display

* `app.rs`: `SigmaApp` state, menu bar, the two layouts. `ui()` is the per
  frame entry point in eframe 0.37 (older eframe used `update()`).
* `ui/list.rs`: filters and sorts only when the table or filters change
  (`view_dirty`), then draws visible rows with `ScrollArea::show_rows`.
* `ui/crawl.rs`: the smooth marquee (see the module docs for the why).
* `ui/chart.rs`: lines, grid, forecast cone, hover readout.
* `ui/detail.rs`, `ui/drivers.rs`, `ui/windows.rs`: panels and pop-ups.
* `fmt.rs`: bounded-width number formatting, with a test that checks widths.

The UI never mutates market data. Buttons send `Command`s; results come back
in the next `Frame` (within about 100 ms).

---

## 12. Testing strategy

| Layer | Where | Example |
|---|---|---|
| Unit | `#[cfg(test)] mod tests` at the bottom of each file | Fenwick sampling frequencies, OU variance, persona distribution |
| Property-style statistics | same | One big catch-up step vs many small steps give the same return spread |
| Integration | `crates/sigma-core/tests/engine.rs` | A year on a small market stays finite and in band; same seed gives identical prices; player buy and sell round trip |
| Soak | `examples/soak.rs` | 1,000 full-scale days, audited every 30 |
| Manual | run the app | Layout, crawl smoothness, windows |

Integration tests use `Config::small()` (about 1,000 securities) so the whole
suite runs in about 15 seconds in release mode. Run tests with `--release`:
statistical tests draw hundreds of thousands of samples.

---

## 13. If this were a team PR

This branch is one large PR because it is a rebuild. A team would usually
land it as a stack of reviewable pieces, each green on CI:

1. Workspace skeleton, `rng`, `clock`, `sampler` with tests.
2. `persona`, `market`, `macro_econ` with tests.
3. `security` with the cadence-invariance test.
4. `portfolio`, `broker`, `engine`, integration tests, soak example.
5. `protocol`, `runtime`, console logging.
6. The app, one panel per PR if the reviewers prefer.
7. Docs.

Review norms worth adopting: keep PRs under ~400 lines of logic where
possible, explain *why* in the description, add a test with every bug fix
(like the determinism fix above), and never merge with `NaN` warnings in the
soak output.
