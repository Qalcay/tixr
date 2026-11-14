# Code walkthrough: Tixr-sigma path A

A guided tour of the code: what each part does, why it was built this way instead of the alternatives, and how a professional team would treat it. Read it alongside the source. Every file also has a module doc comment (`//!`) at the top.

---

## 1. Project structure

```text
tixr-sigma/
├── Cargo.toml                 workspace: members + shared profiles
├── crates/
│   ├── sigma-engine/          library, ZERO dependencies
│   │   ├── src/               rng, time, queue, persona, macroecon, exchange,
│   │   │                      security, broker, engine, protocol, runner, console
│   │   ├── tests/long_run.rs  integration tests (public API only)
│   │   └── examples/probe.rs  headless calibration tool
│   └── sigma-app/             binary `tixr-sigma`, depends on engine + eframe
│       └── src/               main, app, panels, chart, ticker, format, theme,
│                              windows/{brokers, personal, settings}
└── docs/                      the two design docs + this walkthrough
```

**Why a workspace with two crates?** This is the standard Rust split between *core logic* and *front-end*.

* The engine compiles in seconds and its tests never build a GUI stack. CI on a headless server can test it.
* The crate boundary *enforces* the thread split: the engine cannot reach into egui, and the UI can only use what the engine marks `pub`.
* Someone could later write a terminal UI, a web front-end or a batch backtester against the same engine.

The alternative, one crate with modules (what alpha did), is fine for small apps, but nothing then stops UI code from leaking into simulation code.

**Why `tixr-sigma/` beside the alpha instead of replacing it?** Alpha stays buildable for comparison, and the PR review is a pure addition. Moving sigma to the repo root later is a one-commit change if you choose this path.

---

## 2. Engine, bottom-up

### `rng.rs`: random numbers without `rand`
* **xoshiro256++** is the same algorithm `rand::SmallRng` uses on 64-bit targets. It is about 30 lines, fast, and statistically strong. **SplitMix64** expands one `u64` seed into its 256-bit state, as the authors recommend.
* Distributions: `uniform`, `normal` (Box-Muller, caching the spare value), `exponential` (inverse CDF: `-ln U / rate`), `poisson` (Knuth for small means, normal approximation above 30), and `weighted_index`.
* *Why hand-roll?* `rand` + `rand_chacha` + `rand_core` and their features add compile time and version churn (alpha pinned `rand 0.10`, whose API changed again). The cost is that we own the tests. See the bottom of the file: each distribution's mean and variance is checked over 100k-200k samples.
* *Professional note:* this is fine for simulation. **Never** use a hand-rolled RNG for cryptography or anything security-related.

### `time.rs`: the clock
* Sim time is an `f64` of sim seconds. `BASE_TIME_SCALE = 86_400 / 5_400 = 16` encodes "90 real minutes = 24 sim hours" as arithmetic, not a magic number. A unit test asserts it equals 16.
* `Session` answers "is it open?", "when does it next open?" and "when does it next close?", with weekends included. Those three functions are all the engine needs to schedule open/close events.

### `queue.rs`: the calendar queue
This is the heart of "update only when picked".

* A ring of buckets, each covering 0.25 sim seconds. Each bucket is a `Vec` sorted descending, so the next event to pop is at the *end* and `Vec::pop` is O(1).
* Events beyond the ring's horizon wait in a `BinaryHeap` and migrate into the ring as the cursor approaches.
* When the ring is empty (nights, weekends) the cursor jumps straight to the next event instead of stepping through empty buckets.
* *Alternatives considered.* A plain `BinaryHeap` is O(log n) and simpler, and honestly fine up to ~100k events. A hierarchical timing wheel (as in the Linux kernel or Tokio) is better for timers that are usually cancelled, which ours are not. The calendar queue wins for "many events, mostly near future, rarely cancelled".
* *Testing habit:* `matches_binary_heap_under_random_load` is a **differential test**. It runs the clever structure and a trivially-correct reference side by side on random input and asserts identical output. This is the standard way to test an optimised data structure.

### `persona.rs`: perks A-P
* `Persona(u16)` is a **newtype** around a bitmask: 2 bytes, `Copy`, and cheap to put in every snapshot row. Methods (`has`, `with`, `without`, `letters`, `parse`) hide the bit twiddling.
* `PerkEffect` uses struct-update syntax (`PerkEffect { kappa_mul: 1.8, ..e }`) so each perk lists only what it changes.
* *Naming:* Rust convention is `CamelCase` for types and enum variants, `snake_case` for functions and fields, `SCREAMING_SNAKE_CASE` for constants. The alpha mixed styles (`SetTickerStatus`, `RIDE_ZEN`, `DT`); sigma follows the convention throughout, and `cargo clippy` will flag deviations.

### `macroecon.rs`: drivers and the yield curve
* `ou_step` is the **exact** Ornstein-Uhlenbeck transition. Every mean-reverting variable uses it, so the hourly step size cannot cause overshoot. This is the general fix for "after 300 days it blows up".
* Nelson-Siegel is the curve model many central banks publish. Three numbers (level, slope, curvature) give a smooth curve, and `forward_at` gives the market-implied policy path, which is the "forecast" on the yield-curve chart.
* The regime chain moves between Calm, Bull, Correction and Crisis. Its odds rise with `stress()`: slack, curve inversion, inflation above 4%, and wide credit spreads.

### `exchange.rs`: the market factor
`X = ln(earnings) + ln(P/E)`. The P/E is pulled toward the Gordon fair multiple, so the index cannot drift away from fundamentals forever, and rate moves reach stocks through valuation. Global and local shocks are mixed with correlation 0.65.

### `security.rs`: the price model
* Read the module docs first. They explain why exact transitions make update frequency irrelevant.
* `SecurityId` and `BrokerId` are **generational indices**: slot plus generation packed into a `u32`. This is a common game-engine pattern (the `slotmap` and `generational-arena` crates use it). A recycled slot gets a new generation, so stale references simply fail to resolve. That fixes the alpha bug where a respawned ticker inherited old holdings.
* `Ticker` stores up to five ASCII bytes inline: no heap allocation per ticker, `Copy`, and hashable.
* `reprice()` is the safety net: if anything ever becomes non-finite, it repairs, counts and reports instead of letting `NaN` spread.

### `broker.rs`: participants
* `Portfolio` is shared by the player and brokers ("one shape for everyone", as in alpha). Positions are a **`BTreeMap`**, not a `HashMap`. Std's `HashMap` randomises iteration order *per instance*, which made two runs with the same seed diverge. The determinism test caught it, which is exactly why that test exists.
* `score()` is a **pure function** (inputs in, number out), so strategy logic is trivially unit-testable without spinning up an engine.

### `engine.rs`: the event loop
* `advance_to(target, deadline)` pops due events and dispatches with one `match`. An optional deadline lets the runner bound each slice to 10 ms.
* The pattern `let Some(x) = ... else { return };` (let-else) keeps the "stale event, ignore it" path short and flat.
* Each security has exactly one pending `SecTick`; `tick_epoch` invalidates a whole schedule at once (used when warm-up ends).
* `delist` swap-removes from the dense `live` list in O(1), using `live_pos` as a reverse index. That is the classic sparse-set trick for "remove from the middle of a list without shifting".
* `health_check()` lists every invariant in one place. Tests call it, and the runner calls it every 5 s and prints failures to the console.

### `protocol.rs` + `runner.rs`: two threads, message passing
* **Answering the brief's question, "message passing, recommended for dual-thread Rust?"** Yes. The engine thread *owns* the state, and the UI holds only owned snapshots. No `Mutex`, no `Arc<RwLock<Engine>>`, no lock contention, and no chance of the UI freezing because the engine holds a lock. "Do not communicate by sharing memory; share memory by communicating."
* `mpsc::channel` (unbounded) carries commands, which are tiny and rare. `mpsc::sync_channel(2)` carries snapshots and gives **back-pressure**: if the UI falls behind, `try_send` fails and the engine skips that snapshot. Notices from a skipped snapshot are carried into the next one, so no trade confirmation is lost.
* *Why std channels and not `crossbeam`?* Since Rust 1.67, `std::sync::mpsc` *is* crossbeam's implementation, so the dependency buys nothing here.

### `console.rs`: real-time debug output
One `AtomicU8` holds the level, and the `console!` macro checks it before formatting anything, so disabled lines cost one relaxed load. Output goes to stderr with sim timestamps, so it can be piped or filtered (`cargo run ... 2>&1 | grep bankrupt`).

---

## 3. App

* **`main.rs`** parses arguments by hand (`--seed`, `--securities`, `--log`). That is enough for three flags; `clap` would be the move once there are a dozen. The engine is spawned inside eframe's creation closure, so it can capture the `egui::Context` and call `request_repaint()` when a snapshot lands.
* **`app.rs`** holds UI state. Layout is decided by `LayoutKind` (Classic or Terminal) with `exact_size`, non-resizable panels.
* **`panels.rs`**: `security_list` uses `ScrollArea::show_rows`, so only visible rows are laid out (**virtualisation**). Rows are painted at fixed x offsets with bounded-width text, so content can never widen the panel. The `app.snap.take()` / put-back pattern sidesteps a borrow-checker conflict (reading the snapshot while mutating `app`) without cloning. Taking a `Box` out of an `Option` is a pointer move.
* **`ticker.rs`**: the jitter fixes are listed in its header comment.
* **`chart.rs`**: two charts drawn with `Painter`. The forecast cone is two `Mesh` bands, because egui's polygon fill expects convex shapes and a cone around a curved mean is not convex.
* **`format.rs`**: bounded-width formatters, with a test that every output fits its column.

---

## 4. Testing habits used here

| Kind | Example | Why |
|---|---|---|
| Unit | `rng::tests::*`, `persona::tests::*` | small, fast, pin behaviour of one function |
| Statistical | `normal_has_unit_variance`, `variance_is_independent_of_update_frequency` | random code is tested by its *distribution*, with tolerances sized to the sample count |
| Differential | `queue::matches_binary_heap_under_random_load` | optimised structure versus a trivially-correct reference |
| Determinism | `same_seed_same_market_regardless_of_chunking` | guards reproducibility, which is what makes bugs reproducible |
| Integration | `tests/long_run.rs` | public API only; 400 sim days with invariant checks every 10 days |
| Soak (ignored by default) | `ten_year_soak` | too slow for every run, so opt in with `--ignored` |

Commands:

```bash
cargo test --release                                   # everything fast (~50 s)
cargo test --release -p sigma-engine -- --ignored      # 10-year soak
cargo run --release -p sigma-engine --example probe -- 1500 7   # calibration printout
```

Release mode matters for the simulation tests. The workspace also sets `opt-level = 2` for the engine in dev builds, so `cargo run` stays responsive.

---

## 5. How a team would review this

* **Split the PR.** In a real team this would land as several PRs: engine core with tests, then runner and protocol, then UI, then docs. Each would be reviewable in under an hour. It is one PR here because the brief asks for a complete alternative path to compare against path B.
* **Review order.** Read `lib.rs` (module map), then `security.rs` (the model), then `engine.rs` (the loop), then the tests. Reviewers check *invariants* first ("can a stale id ever resolve?", "can anything become non-finite?") and style last.
* **Tuning constants.** Numbers such as `0.05` births and `0.04` acquisitions are model calibration. In review they should come with evidence, which is why `examples/probe.rs` exists. Paste its output in the PR when changing them.
* **Clippy and fmt.** `cargo fmt` and `cargo clippy --all-targets` should be clean before review. CI usually enforces both.
