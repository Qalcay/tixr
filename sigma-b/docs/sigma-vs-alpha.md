# How the Tixr-sigma Stock Engine Differs from the Original Tixr "alpha"

This document compares the original tixr ("alpha", the `src/` folder at the
repository root) with Tixr-sigma **path B**, the sampled engine in `sigma-b/`.
Path A, built separately, takes an event-driven approach to the same problem.

## At a glance

| | tixr alpha | Tixr-sigma path B |
|---|---|---|
| Securities | 255 fixed slots, every one stepped every tick | Up to 65,536 slots, live count held between 8,192 and 32,768 |
| Update model | Loop over all stocks each tick | Budgeted weighted random sample each tick, plus exact lazy catch-up |
| Cost per tick | O(number of stocks) | O(budget x log n), independent of the number of stocks |
| Pricing | Sum of drift, cycle, reversion, GARCH noise, beta shock, jumps on log price | Fundamental value (Gordon growth on earnings, rates-driven cost of equity) times sentiment factors times an OU mispricing |
| Stabilising forces | Clamps after the fact | Mean reversion by construction, bounded multiples, soft-capped personas, square-root impact that fades, splits, listing rules |
| Markets | 4 exchanges, each a source of shared shocks | 11 sectors drive co-movement; 4 exchanges are listing tiers by size with monthly promotion and demotion |
| Macro | Taylor rule, inflation, gap, ad-hoc 6-point curve, money supply | OU macro with exact steps, Taylor rule with a zero floor, Nelson-Siegel curve, forward curve, credit spread, ERP, fear gauge, recession probability |
| Long runs | Reported to reach inf/NaN after about 300 sim days | 1,000 sim days at full scale: 0 non-finite values |
| Time | Fixed steps per loop (speed depended on CPU) | Wall-clock paced: 90 real minutes = 1 sim day at 1x, 0.25x to 1024x |
| Threads | Engine and UI threads over crossbeam channels | Engine and UI threads over `std::sync::mpsc`, nothing shared |
| Debugging | Needed an embedded log | Console status line every 5 s on stderr, `SIGMA_LOG` levels, live audit |
| Holdings | Keyed by slot, so a respawned stock inherited old positions | Keyed by a generation-tagged id; a reused slot gets a new id |
| Reproducibility | Seed mixed with the system clock | Same seed, same market (tested) |
| Dependencies | 271 crates, two copies of egui | 125 crates for the app, 0 for the engine |

---

## 1. The core problem and the path-B answer

Alpha advanced every stock on every tick. With 255 stocks that was fine; at
10,000 it is not, because the work per tick grows with the number of stocks
and the UI has to wait for it. Patches that skip stocks break the maths:
a stock that is skipped for a while and then stepped once with the per-tick
formula moves too little.

Path B solves this with two ideas that only work together.

### Idea 1: exact catch-up over any gap

Each stock's private state is made of processes whose transition over **any**
time gap has a closed form:

* **Mispricing `m`** and **volatility state `h`** are Ornstein-Uhlenbeck (OU)
  processes. For an OU process,
  `x(t+dt) = mu + (x - mu) e^(-k dt) + sigma sqrt((1 - e^(-2k dt)) / 2k) Z`
  is exact for any `dt`.
* **Earnings `e`** follow Brownian motion with drift, also exact for any `dt`.
* **News jumps** arrive as a Poisson process: the number of jumps in a gap is
  drawn once, and the sum of normal jump sizes is itself normal.
* **Excess growth `g`** is OU; its integral over the gap uses the trapezoid
  rule (the one approximation, and a small one).

So a stock that has not been touched for an hour can be brought up to date in
one step, and the result has **the same distribution** as if it had been
stepped every second. The test `update_cadence_does_not_change_distribution`
checks this: 20 simulated days in one step versus forty steps give the same
spread of returns.

### Idea 2: shared factors are read, not stored

Co-movement comes from global factors (market sentiment, 11 sector
sentiments, rates, GDP, CPI). A stock's price is

```
ln P = ln V(earnings, rates, growth) + beta * S_market + gamma * S_sector + m
```

The global terms are evaluated at the moment the stock is updated. They are
not accumulated per stock. So when a stale stock is sampled it instantly
reflects everything the market did while it slept, and co-movement is exact
however rarely each stock is visited. There are only a few dozen global
numbers, and they are stepped every tick.

### Putting it together: the tick

Each engine tick (30 per real second):

1. Step the clock, the macro economy and the sentiment factors, in sub-steps
   that never cross a market open or close.
2. Update the **forced** set: the selected stock, the player's holdings and
   stocks with resting orders.
3. **Sweep**: a round-robin cursor updates just enough slots that no stock is
   older than 30 simulated minutes. This bounds quote age and guarantees every
   daily close is recorded.
4. **Sample**: draw a budget of slots from a Fenwick-tree sampler weighted by
   volatility and attention (stocks on screen get 8x weight, held ones 20x).
5. Let a Poisson number of brokers act.
6. List new companies at the rate the population controller sets.

The **Fenwick tree** (binary indexed tree) keeps prefix sums of 65,536
weights, so changing a weight and drawing a weighted sample both cost
O(log n), exactly 16 steps because the cap is a power of two.

The **budget** adapts: the engine measures how long an update takes and fits
as many as possible into a configurable slice of each tick (1.5 ms by
default, adjustable in Settings). On the development machine that was about
1,600 updates per tick, roughly 45,000 per second, at around 0.9 microseconds
each, for 15,000 live stocks: each stock is refreshed about three times a
real second, while the engine uses about 1.7 ms of each 33 ms tick. Doubling
the number of stocks would halve how often each is refreshed and change
nothing else.

### Variance time

Real markets move more in a session than overnight. Sigma runs a second clock,
**variance time**, that advances at full speed while the market is open and at
12.4% speed overnight (4% on weekends), normalised so annual volatilities are
calendar-year volatilities. Prices diffuse on variance time, fundamentals on
calendar time. Each stock remembers the variance time of its last update, so
the catch-up step uses the right amount of randomness whether the gap spanned
a lunch hour or a weekend.

---

## 2. A different stock valuation

Alpha moved log price directly with a sum of competing terms. Several of them
drifted with no restoring force, so over hundreds of days nothing anchored the
price.

Sigma separates **what a company is worth** from **what the market pays**:

```
earnings E  = exp(e + pass_through * ln CPI + ln real GDP + (cyclicality - 1) * output gap)
cost of eq. = 10y yield + beta * equity risk premium + persona premium + leverage * credit spread / 2
growth g_lt = potential growth + pass_through * expected inflation + 0.3 * current excess growth
multiple    = (0.4 + 0.6 * payout) * (1 + g_lt) / (cost of equity - g_lt)     clamped to 3..45
fair value  = E * multiple
price       = fair value * exp(beta * S_market + gamma * S_sector + m)
```

That is the Gordon growth model with a CAPM cost of equity, which is how a
textbook would value a company. Consequences you can watch happen:

* When the 10-year yield rises, every multiple compresses at once.
* When fear rises, the equity risk premium rises and prices fall even if
  earnings did not change.
* Cyclical stocks swing with the output gap, Pricing-power stocks track
  inflation, Kontrarian stocks shrug off the cycle.
* The detail panel shows model value, the value gap, P/E, dividend yield,
  cost of equity and a one-year model forecast with a 90% range.

### The forces that stop wild moves

1. **Mean reversion of mispricing.** `m` is OU with a half-life of a few weeks
   (personas change it: Anchor faster, Momentum slower). Prices can drift away
   from fair value but are always pulled back. Value brokers trade the gap,
   which is the economic story behind the maths.
2. **Volatility mean reversion.** Volatility clusters (`h` is OU) but cannot
   explode; `h` is bounded at plus or minus 1.5.
3. **Bounded valuation.** The multiple is clamped to 3 to 45 times earnings
   (0.3 to 4 times book for broker firms), and the cost of equity is always at
   least 2.5 points above long-run growth.
4. **Soft-capped personas.** Stacked perks saturate (see the personas doc).
5. **Square-root market impact that fades.** Order impact scales with the
   square root of order size over daily volume, is capped at 5% per order, and
   goes into `m`, so it decays like real price pressure.
6. **Stationary sentiment.** Market and sector sentiment are OU factors; they
   produce booms and busts but have a long-run average.
7. **Corporate actions.** Stocks split 10-for-1 above 1,000. Stocks under 1.00
   for 30 closes are delisted (like the NYSE $1 rule).
8. **A last guard.** Non-finite values are refused at repricing, counted, and
   the security is delisted with a console warning.

### Do we still use different markets?

Yes, but each now means one thing. **Sectors** (11, modelled on the GICS
sectors) are where shared movement comes from, with different growth, payout,
leverage and cyclicality profiles. **Exchanges** keep alpha's four names
(#EX01, #CHA2, #4MAQ, #AST8) and become listing tiers by market cap: venture
board below 300M, growth board to 2B, mid board to 10B, main board above. A
monthly review promotes and demotes companies with 20% hysteresis so they do
not flip back and forth. Each board and the whole market has a cap-weighted
index whose divisor is adjusted on listings and exits, as real index providers
do.

---

## 3. Yield curve and market drivers, rebuilt

The original curve blended a short and a long rate with a weight per tenor;
nothing kept its inputs in range, and money supply compounded without limit.

Sigma's macro layer (`macro_econ.rs`):

* **Output gap, inflation, term premium, curvature, credit spread and risk
  appetite** are OU processes stepped exactly. Each has a long-run mean, so
  none can wander off.
* **Policy rate** follows the Taylor (1993) rule with inertia and a zero floor.
* **Inflation** follows a Phillips curve around a 2% target; expectations are
  anchored (75% target, 25% current).
* **The curve** is Nelson-Siegel: level = neutral real rate + expected
  inflation + term premium; slope = policy rate minus level; curvature is its
  own OU factor. The curve inverts naturally when policy is tight.
* **Forward rates** from the same curve are drawn dashed: they are the
  market's forecast of where rates are going.
* **Recession probability** uses the Estrella-Mishkin model the New York Fed
  publishes, `Phi(-0.5333 - 0.6330 * (10y - 3m spread in percent))`.
* **Equity risk premium, fear gauge and credit spread** respond to risk
  appetite and the output gap, and feed straight into valuations.

The test `macro_stays_finite_for_a_century` steps the economy daily for 100
years. The soak run takes the whole market through 1,000 days.

### What probably caused the NaN after ~300 days in alpha

From reading alpha's `engine.rs` (not reproduced by running it):

* `fair_log += mu * DT` every tick with no limit, so the anchor itself drifted.
* Regime drift (`reg_drift * DRIFT_TO_PERCENT * beta`, up to -2.67 per year in
  Chaos) went straight into log price.
* Variance was clamped to 8.8, so `sqrt(var)` could reach ~3 per unit time,
  enough to throw log prices far out within a few bad ticks.
* Order impact divided by `price * shares`; as a price approached zero the
  impact of any order grew without bound, and `exp` of a large log price is
  `inf`, after which arithmetic yields `NaN`.
* Everything was `f32`, which runs out of precision far sooner than `f64`.

Sigma uses `f64` throughout the engine, only processes that have stationary
distributions, bounded multiples and capped impact.

---

## 4. Time, threads and debugging

* **Clock.** 90 real minutes = 24 simulated hours at 1x (16 sim seconds per
  real second). The engine measures real elapsed time each loop and advances
  by that, so the pace does not depend on the machine. A real step is capped at
  0.25 s so waking a laptop does not fast-forward the market. Speed runs from
  0.25x to 1024x; sessions can be Regular (09:30-16:00), Extended
  (04:00-20:00) or Always open.
* **Threads.** Engine and display each own their data and share nothing. The
  UI sends `Command`s; the engine sends complete `Frame`s through a 2-slot
  `sync_channel`. If the UI is busy the engine drops that frame instead of
  queuing it, so memory cannot pile up and the UI always draws the newest
  state. Message passing like this is the idiomatic Rust design for two
  threads with different deadlines.
* **Console debugging.** The engine thread prints a status line every 5
  seconds to the terminal: sim time, session, population and band, IPOs and
  exits by kind, brokers, composite index, rates, non-finite count, updates
  per second, cost per update, samples per tick, quote age at update and tick
  time. It also runs an audit of every number and warns if one is bad. Set
  `SIGMA_LOG=debug` (or change it in Settings) for daily closes and broker
  events, `trace` for every IPO, exit and tick.

---

## 5. Display fixes

* **The side panel can no longer be pushed out.** All panels have exact sizes
  (selectable in View > Panel width) and are not drag-resizable; only the
  centre grows with the window. Every number goes through a bounded-width
  formatter that switches to K/M/B/T before it can get long, rows are painted
  at fixed column positions inside a clip rectangle, and splits keep prices
  under 1,000.
* **Two layouts** under View: Desk (list left, drivers dock at the bottom) and
  Terminal (drivers column left, list right, one combined crawl).
* **Smooth crawl.** Alpha's crawl offset was `time * speed mod total_width`,
  and `total_width` changed whenever a stock listed or failed, so the crawl
  jumped. It also gave every cell the same width regardless of its text. Sigma
  keeps the offset as state advanced by the frame time, recycles cells only
  once they are off-screen, uses fixed-width monospace cells, and shifts the
  row by a whole number of physical pixels.
* **Virtualised list.** Only visible rows are drawn, so 32,768 rows cost the
  same as 30.
* **New windows.** Settings, Brokers & firms (every broker, its strategy,
  listed ticker, equity, ROE, holdings, and the economy's money flows) and
  Personal finances (net worth breakdown, sector allocation, positions with
  average cost, resting limit and stop orders, recent fills, founding your
  own company).

## 6. Dependencies

Measured on this branch with `cargo tree` (unique crates in the normal
dependency graph):

| | Crates | Notes |
|---|---|---|
| tixr alpha | 271 | eframe 0.30 with defaults, egui_plot 0.30, egui_extras 0.35 (a second copy of egui), rand, rand_chacha, rkyv, crossbeam-channel |
| sigma-app | 125 | eframe 0.37 with `default-features = false` and only `glow`, `default_fonts`, `x11`, `wayland` |
| sigma-core | 0 | standard library only |

What went: the wgpu renderer (eframe's new default, very large) in favour of
OpenGL via glow; accesskit; system font discovery; `egui_plot` and
`egui_extras` (replaced by ~200 lines of painter code); `rand`/`rand_chacha`
(replaced by xoshiro256++ in ~60 lines); `crossbeam-channel` (std's mpsc);
`rkyv` (save and load is not ported yet, see below). The build-size numbers
for a clean build are in the pull request description.

## 7. Not carried over (yet)

* **Save and load.** Alpha serialised with `rkyv`. Sigma's state is larger and
  still settling; a versioned save format is the natural next step.
* **Knob presets** (rush, quiet, casino). The speed, session and engine budget
  settings replace part of their role.
* **Custom fonts** from `fetch/font`. Sigma uses egui's bundled fonts, so the
  app has no runtime file dependencies.
