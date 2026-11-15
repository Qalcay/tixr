# How the Tixr-sigma stock engine differs from the original Tixr "alpha"

Tixr-sigma is a ground-up rebuild of [Tixr](../../README.md) (called **alpha** below). This document explains what changed in the engine and why. It covers path A, the event-driven design.

---

## 1. The one-paragraph summary

Alpha ran a fixed loop. Every tick it advanced **all 255 stocks** with a small Euler step, every broker rolled dice, and the display read the result. That design does not scale: cost grows with `stocks x ticks`, and errors compound with every step. Sigma is a **discrete-event simulation**. Each security has its own random clock (a Poisson process), and a security is touched only when that clock rings, when someone trades it, or when its exchange opens or closes. Between touches nothing happens to it, and nothing needs to. Its price model has an exact closed-form transition, so catching up over 3 seconds or over a long weekend is the same O(1) computation and is statistically exact. Tens of thousands of listings can sit in the background at almost no cost.

---

## 2. Side-by-side

| Topic | Alpha | Sigma (path A) |
|---|---|---|
| Update scheme | Every stock, every tick (`for s in stocks`) | Only when picked by its own exponential clock, a trade, or a session boundary |
| Scale | 255 fixed slots | Up to 16,384 listings at once (2^14), held around 8,600 with a floor near 8,000; 32,768 entities in total |
| Scheduler | none (loop) | Calendar queue (bucketed timing wheel), O(1) push/pop |
| Time | tick counter; ~2,600x too fast; not tied to wall clock | `f64` sim seconds; **90 real min = 24 sim h** (16x), wall-clock paced and **time-locked at 1x** in release builds (speed controls exist only in dev builds, section 9) |
| Numeric type | `f32` prices and log prices | `f64` everywhere; prices clamped to e^±30 |
| Price model | Euler-stepped GBM + GARCH + cycle + reversion + jumps | Exact multi-factor model: market, rates, inflation, credit, world, business-cycle and recovery factors, fundamental random walk with jumps, a deterministic life-cycle term, OU mispricing |
| Market co-movement | per-tick shock per exchange | shared factor *level* per exchange; securities store the factor value they last saw and catch up exactly |
| Restoring forces | weak reversion to a drifting `fair_log` | 6 explicit forces (section 4) |
| Price impact | linear: `0.6 x notional / market cap`, unbounded | square-root law, capped at 10% per order, lands in the mispricing term and decays |
| Markets | 4 exchanges, same hours | 4 exchanges with their own sessions (follow-the-sun), volatility, growth tilt and valuation |
| Macro | Taylor rule + linear yield-curve interpolation | New-Keynesian loop + Nelson-Siegel curve + forward curve + regime chain driven by stress |
| Life cycle | dead slot respawns at 1.1% chance per tick | IPOs, bankruptcies (structural default barrier), penny delisting, acquisitions, fund closures, all steered by a carrying capacity K |
| Ownership | holdings keyed by **slot index**; respawned ticker inherits old positions | generational `SecurityId` (slot + generation); positions are settled in cash when a listing exits |
| Brokers | 255 bots, never fail | Up to 13,927 brokers and 2,457 firms (85/15) with books, stop-loss/take-profit, operating costs; they go broke and are replaced |
| Save / load | `rkyv` snapshot | sealed state file + separate chart history file; a loaded game continues exactly (section 9) |
| Charts | one rolling buffer | eight ranges from 1 hour to 10 years per stock, pre-aggregated, drawn in microseconds (section 9) |
| Determinism | seed mixed with system time | same seed => same market, independent of frame timing (tested) |
| Personas | none | 16 stacking perks A-P per security, one signature mechanism each, audited over all 65,536 combinations |
| Forecasts | none | analytic forecast cone (mean, 68%, 95%) per security; forward curve for rates |
| Tests | none | about 70 unit tests + 400-day and full-population integration tests + optional 10-year soak |
| Debug output | embedded egui log | engine thread writes to the terminal (stderr) at a runtime-selectable level |
| Dependencies | eframe (wgpu), egui_plot, egui_extras **0.35** (a second egui!), rand, rand_chacha, rkyv, crossbeam | eframe (glow, minimal features). The engine has **zero** dependencies. |

---

## 3. Why the alpha could not scale, and why "update only when picked" works

### The fundamental problem
If every one of 10,000 stocks must be stepped every tick, the frame budget is spent on stocks no one is looking at. Patching that by updating subsets in rotation makes things worse in a subtle way. A stock updated less often gets fewer small Euler steps, so its volatility, drift and mean reversion all change with the update rate. The market's statistics come to depend on the scheduler.

### The sigma answer: exact transitions
Every random component of a sigma price has a closed-form distribution for *any* elapsed time `dt`:

```text
ln P = base + Σ_k e_k * F_k(t)   (systematic factors)
     + y(t)                       (firm fundamental)
     + u(t)                       (mispricing)

y(t+dt) = y + (alpha - sigma_f²/2) dt + sigma_f sqrt(dt) Z + Σ_{N~Poisson(lambda dt)} jumps
u(t+dt) = u e^{-kappa dt} + sigma_u sqrt((1 - e^{-2 kappa dt}) / 2kappa) Z'
F_k     : read from global state; a security stores the F_k it last saw
```

So the update frequency is a *presentation* choice (how often a ticker prints), not a modelling choice. The test `security::tests::variance_is_independent_of_update_frequency` checks this. One update per day and 200 updates per day give the same one-day return variance, within sampling error.

That freedom is what the engine exploits.

* **Per-security Poisson clocks.** Large caps and Key or Crypto names print often, small caps and Novel names rarely. Inter-arrival times are exponential, which is memoryless. A pending event never has to be re-drawn when conditions change.
* **Adaptive print density.** The engine thread measures its own CPU use every 0.5 s. Above 60% of a core it lowers a global `tick_density` multiplier; below 25% it raises it back toward 1. At 1x speed the default market (8,600 listings, about 9,600 brokers and firms) uses about 12k events per real second, a few percent of one core, so density stays at 1. At very high dev-build speeds density drops and prints get sparser, but the market's statistics are unchanged.
* **Session boundaries are exact.** At every open and close the exchange's members are caught up to that instant. Daily closes, penny-stock checks and splits always use exact values, however sparse the prints were.

### The scheduler: a calendar queue
All activity is an event in one queue: stock prints, broker decisions, per-minute market factor steps, hourly macro steps, opens and closes, and the midnight roll (regime, IPOs, acquisitions). The queue is a calendar queue (Brown, 1988). The near future is split into 4,096 buckets of 0.25 sim seconds, and anything beyond that horizon waits in a small heap. Push and pop are amortised O(1). Ties are broken by insertion order, so the pop order depends only on the push history. Whole runs therefore reproduce exactly from a seed, however the wall-clock frames chop up the work (`engine::tests::same_seed_same_market_regardless_of_chunking`).

---

## 4. The forces that stop wild moves

The alpha's main restoring force was reversion toward `fair_log`, and `fair_log` drifted by itself. Sigma layers six forces, each with a real-market counterpart.

1. **Mispricing mean reversion (security level).** `u` is an OU process with half-life `ln 2 / kappa`, typically 2-7 weeks (Meager shortens it, Hype lengthens it). Prices wander around fair value but are always pulled back.
2. **Valuation anchor (market level).** Each exchange's factor is `ln(earnings) + ln(P/E)`. P/E is pulled toward the Gordon-model fair multiple `payout / (10Y + equity premium - growth)`. If an index runs too far, its P/E drifts back. Higher rates or risk premia lower the fair P/E, which is how the yield curve moves stocks.
3. **Macro mean reversion.** Output gap, inflation, the policy rate (Taylor rule with smoothing), term premium, equity risk premium and credit spread are all mean-reverting, with exact OU steps and hard clamps. None of them can compound.
4. **Transient, square-root price impact.** Impact is `coef x 2% x sqrt(order / daily volume)`, the empirical square-root law, capped at 10% per order. It is added to `u`, so it decays at the security's own reversion speed. A whale can move a stock, but it cannot send it to infinity, and the move fades.
5. **Value traders.** Value and Whale brokers see a noisy estimate of `u` and buy below fair value and sell above it. That is the arbitrage that, in real markets, *is* the restoring force.
6. **Exit and renewal.** Fundamentals that fall through a structural default barrier go bankrupt. Sub-$1 stocks delist after 20 sessions. Key stocks are the exception to both: they are recapitalised at the barrier (diluting holders) and reverse-split below $1. Prices above $2,500 split 10-for-1. Zombies and runaways are removed rather than left to distort indices.

---

## 5. Why alpha hit inf/NaN after ~300 days (and why sigma does not)

These are findings from reading the alpha source, not from a reproduced crash, so treat them as the most likely causes.

* **Unbounded linear impact.** `impact = 0.6 x impact_mult x notional / (price x shares)`. Whale wallets start at up to `19.2e9 x 80 ≈ $1.5T`, larger than most simulated companies. A single whale order can add tens of units to a log price. Repeated, that passes `ln(f32::MAX) ≈ 88.7`, and `exp` returns `inf`.
* **`f32` state.** Prices, log prices, variances and returns were `f32`, which overflows early and loses precision over long runs.
* **Contagion.** Once one price is `inf`, `inf - inf` or `0 x inf` in market caps, index averages and broker equity produces `NaN`, and `NaN` spreads to every aggregate it touches.
* **Slot-keyed holdings.** When a dead slot respawned, positions silently transferred to the new ticker. Equity calculations could then mix stale and new prices.

Sigma addresses each one: square-root impact capped at 10%, `f64` state with clamped log prices, exact transitions with no step-size blow-up, generational ids with cash settlement on exit, and a `reprice()` guard. The guard repairs any non-finite value, counts it in `stats.repairs`, and reports it on the console. The 400-day test and the 10-year soak assert that `repairs == 0` and that `health_check()` passes every 10 days.

---

## 6. Do we still use different markets?

Yes, and now they matter. The four exchanges (EX01, CHA2, 4MAQ, AST8) keep the alpha's names and ticker lengths, and gain:

* **Sessions.** CHA2 01:30-08:00, 4MAQ 08:00-16:30, EX01 13:30-20:00, AST8 16:00-23:30, Monday to Friday. Something is usually open, and overnight gaps appear naturally at each open.
* **Their own factor.** Each exchange has its own earnings trend, growth tilt, valuation and volatility. The factors are correlated through a shared global shock (rho = 0.65), so crises are global but not identical.
* **Closed-hours fast-forward (dev builds only).** When every exchange is shut, a dev build can run the clock 8x faster. It is off by default and unavailable in release builds.

---

## 7. Display side

* **Thread split with message passing.** This is the idiomatic Rust answer for two threads. The engine thread owns all market state. The UI sends `Command`s over an `mpsc::channel`. The engine sends owned `Snapshot`s over a bounded `sync_channel(2)`, and skips a snapshot rather than queueing stale ones if the UI is busy. No locks, no shared mutation, and the compiler proves there are no data races.
* **Console debugging.** The engine writes to the terminal with sim timestamps. That includes a daily summary line, open and close, IPOs, bankruptcies, broker failures, a perf line every 10 s at `debug`, and health-check failures. The level can be changed live in Settings or with `--log`.
* **Jitter-free ticker crawl.** The crawl moves a whole number of pixels per frame. Each cell's text is laid out once when it enters and stays fixed until it leaves, and widths are measured from the laid-out text. See `crates/sigma-app/src/ticker.rs`.
* **Layout never moves with content.** Side and dock panels have exact sizes, every number goes through a bounded-width formatter, and the security list is virtualised. Only resizing the window changes the layout. The View menu (and Settings) offers two layouts: **Classic** and **Terminal**.
* **New windows.** Settings, Brokers & Firms (every participant, sortable and virtualised, with a drill-down into one book and its equity chart), Financials (sector and market-wide charts), and Personal finance (balance sheet, positions, order history, and launching your own listing).

---

## 8. Dependencies

The 1-2 GB `target/` directory came mostly from eframe's default **wgpu** backend (wgpu + naga + Vulkan/Metal/DX12 layers), plus a second copy of egui pulled in by the mismatched `egui_extras = 0.35`, plus `rkyv` and `rand`. Sigma uses:

* `sigma-engine`: **no dependencies**. RNG (xoshiro256++), distributions, scheduler and maths are in-tree and unit-tested.
* `sigma-app`: `eframe` with `default-features = false` and only `glow` (OpenGL), `default_fonts`, `x11` and `wayland`. Charts are drawn with egui's `Painter` instead of `egui_plot`.
* Workspace profiles drop debug info for third-party crates in dev builds.

Measured on Linux x86-64 with clean target directories (Rust 1.97):

| | Alpha | Sigma (default) | Sigma (`--no-default-features --features x11`) |
|---|---|---|---|
| Crates in the dependency graph | 345 | 151 | 124 |
| Crates when targeting Windows | 207 | 107 | 107 |
| `target/` after `cargo build` | **2.5 GB** (230 s) | **560 MB** (90 s) | 485 MB (76 s) |
| `target/` after dev + release builds | 3.3 GB | 1.1 GB | n/a |
| Release binary | 13.0 MB | 10.4 MB | n/a |
| Engine crate alone | n/a | 23 MB (2 s) | n/a |

What remains is mostly egui/winit and the platform windowing stack, which any native GUI needs. Going much lower would mean a different GUI toolkit, or a terminal UI over the same engine.

---

## 9. Saves, charts and the time lock

### What a save is
A save slot is a directory with two data files and a short-lived lock.

| File | Holds | Protected how |
|---|---|---|
| `state.tixr` (about 7 MB) | Current values only: every stock, broker, firm, exchange, the macro state, your ledger, the RNG, and the pending event queue | Masked and sealed (below) |
| `history.bin` (about 56 MB) | Chart history for every stock, broker and market series | Checksum only, tied to the state file by a save id |
| `.lock` | Present only while a save or load is in progress | Stops two running games reading and writing one slot at the same moment; a lock older than 30 s is treated as left over from a crash. Files are written to a temporary name and renamed, so a crash mid-save cannot leave a half-written state |

**The seal.** The state bytes are XOR-masked with a keystream and carry a 128-bit keyed tag (SipHash-2-4). The keys come from the **world seed** and a per-game **game seed** together, so a state file cannot be edited in a hex editor, and a block cannot be copied in from another game: on load the tag fails and the game refuses the file (`edited_state_is_refused`). The decoded state is also checked structurally (ids in range, counts within the limits, all values finite) before the game starts.

**What needs sealing.** Only what the engine *reads back and acts on*. Chart history is display data: nothing in the simulation reads it, so editing it changes a picture and nothing else. That is why it lives in its own file with only a corruption checksum, and why a save still loads if the history file is missing or from an older version (charts simply start empty). One detail: brokers' momentum signals use each stock's last 8 daily closes, so those 8 values are state and live in the sealed file, not in the history.

**Honest limit.** This is tamper *resistance*. The game is offline and open source, so the keys can be derived by anyone who reads the code. The seal stops casual editing and accidental corruption. Nothing short of a server can stop a determined cheat.

**Why binary and not JSONL.** JSON Lines was considered for the history file. The same data as text is roughly 5-8x larger and slower to parse, and at 16,000 stocks that is the difference between a 0.2 s load and several seconds. The binary file stores each tier as delta-encoded variable-length integers. An export to JSONL for analysis would be a small addition.

**Exact continuation.** Because the event queue and RNG are saved, a loaded game produces the same future the original would have (`loaded_game_continues_exactly_like_the_original`). Saving takes about 350 ms and loading about 200 ms. The game autosaves to the `autosave` slot on a clean exit.

### Charts: eight ranges, drawn instantly
Each stock keeps five small ring buffers of pre-aggregated bars, and each range reads the tier that fits it.

| Range | Reads tier | Points drawn |
|---|---|---|
| Hour | 1-minute bars (64 kept) | up to 60 |
| 1 day, 3 days | 5-minute bars (320 kept) | up to about 290 |
| Week, Month | 30-minute bars (384 kept; bars exist only while the exchange is open) | up to about 375 |
| Year | daily bars (272 kept) | up to about 260 |
| 3 years, 10 years | weekly bars (528 kept) | up to about 520 |

Drawing a range is a copy of a few hundred numbers from memory, which takes microseconds, so there is nothing to cache and no image to store. The cost is paid when a bar is *recorded* (one write per tier boundary), and the whole history of one stock is at most 12.5 KB. A new world is created with 10 years of back-history already filled in, generated directly from the price model at weekly then daily steps, so the 10-year chart is populated on day one. Brokers keep a 3-year equity history; market series keep all ranges.

### The time lock
`runner::TIME_CONTROLS` is true in debug builds and when the `dev-time` feature is on, false in a plain release build. When false, the engine ignores speed commands and the UI hides the controls, so a released game always runs at 90 real minutes per sim day. **Pause is still allowed** in the locked build: it stops the clock without changing the pace. If a dev build ever changes the speed, the save is flagged and the Settings window shows it.

```bash
cargo run --release -p sigma-app                       # time-locked, as shipped
cargo run --release -p sigma-app --features dev-time   # speed controls for testing
```

---

## 10. What sigma deliberately does not do (yet)

* **Sectors and volatility clustering.** Path B has both. See [why-path-a.md](why-path-a.md).
* **Short selling and leverage.** Portfolios are long-only and cash-funded.
* **Order books.** Trades execute against an implicit market (the public float) with square-root impact. An explicit limit order book could replace that later, per exchange.
