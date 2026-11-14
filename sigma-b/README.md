# Tixr-sigma, path B: the sampled engine

A rebuild of [tixr](../) ("alpha") as a market simulator that scales to the
full 2^16 = 65,536 securities, keeps 8,192 to 32,768 of them alive through
market forces, and runs in real time at **90 real minutes per simulated day**.

Path B's core idea: **each engine tick updates a fixed budget of randomly
chosen securities, and any security can catch up over any gap in one exact
step.** The cost of a tick does not depend on how many securities exist.
Update frequency only decides how finely a price path is drawn, never where it
goes. See [docs/sigma-vs-alpha.md](docs/sigma-vs-alpha.md) for the full story.

## Run it

```sh
cd sigma-b
cargo run --release                 # the app (seed optional: -- 12345)
SIGMA_LOG=debug cargo run --release # more console output
```

Debugging output goes to the **terminal you launched from**, one status line
every five seconds (population, update rate, quote age, tick time, NaN count,
rates), plus events at `debug` and `trace`. There is no log panel inside the
window. The level can also be changed live under Windows > Settings.

On Linux you need the usual X11 or Wayland development libraries that any
`winit`/OpenGL app needs.

## Test it

```sh
cargo test -p sigma-core --release      # engine unit + integration tests (seconds)
cargo test --workspace --release        # everything, including the app's formatters
cargo run -p sigma-core --release --example soak -- 1000   # 1,000 sim days, full scale
cargo run -p sigma-core --release --example soak -- 400 7 small   # quick, small market
```

The soak run audits every price, broker, index and macro variable for
non-finite values every 30 simulated days and fails loudly if it finds one.

## Layout of the workspace

```
sigma-b/
  Cargo.toml                 workspace: two crates, one shared release profile
  crates/
    sigma-core/              the engine. Zero dependencies. No UI code.
      src/rng.rs             deterministic RNG (xoshiro256++, seeded by SplitMix64)
      src/clock.rs           calendar time, sessions, variance time
      src/sampler.rs         Fenwick-tree weighted sampler over 2^16 slots
      src/persona.rs         perks A-P, length distribution, soft-capped stacking
      src/macro_econ.rs      economy, Taylor rule, Nelson-Siegel yield curve
      src/market.rs          sectors, exchanges, sentiment factors
      src/security.rs        one stock: valuation and exact catch-up
      src/portfolio.rs       cash, positions, dividends, interest
      src/broker.rs          strategies and the broker life cycle
      src/engine.rs          the tick: global step, sweep, sample, brokers, IPOs
      src/protocol.rs        Command / Frame: the whole engine <-> UI contract
      src/runtime.rs         the engine thread: wall-clock pacing, channels
      src/log.rs             console logging (SIGMA_LOG)
      tests/engine.rs        integration tests on a small market
      examples/soak.rs       long headless run
    sigma-app/               the egui window. Depends on sigma-core + eframe.
      src/app.rs             app state, menus, the two layouts
      src/ui/*.rs            list, detail, drivers, crawl, chart, windows
  docs/
    sigma-vs-alpha.md        how the sigma engine differs from tixr alpha
    personas-brokers-money.md  personas, brokers & firms, and money
    walkthrough.md           a guided tour of the code, with team practice notes
```

## Controls

* **View** menu: Desk or Terminal layout, panel width (Narrow, Normal, Wide),
  dark or light theme. Panels never resize themselves from their content.
* **Windows** menu: Brokers & firms, Personal finances, Settings.
* **Sim** menu: pause and speed presets (1x is 90 min per sim day).
* Click a stock in the list or type a ticker prefix in Find. Type letters in
  Perks to keep only stocks with all of them.
