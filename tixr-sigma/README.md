# Tixr-sigma (path A: event-driven engine)

A rebuild of Tixr: a market simulator with tens of thousands of listings that launch, trade, and die, driven by a macro economy and a yield curve. Brokers and firms compete, and you trade alongside them.

**Path A** is the event-driven design. Every listing has its own random (Poisson) clock and is updated only when that clock fires, when someone trades it, or when its exchange opens or closes. Its price model has exact closed-form transitions, so rarely-updated listings are never "behind". See [docs/sigma-vs-alpha.md](docs/sigma-vs-alpha.md).

## Run

```bash
cd tixr-sigma
cargo run --release -p sigma-app -- --seed 42 --log info
```

* Run it from a terminal: the engine's live debug output goes there, not into the window. Change the level under **Windows → Settings** or with `--log off|warn|info|debug|trace`.
* Pace: **90 real minutes = 1 sim day** at 1x. Speed buttons sit in the top bar (1x / 16x / 256x); Settings goes from 0.25x to 1024x. Nights and weekends, when every exchange is shut, run 8x faster by default.
* Linux needs the usual X11/Wayland libraries (`libxkbcommon-x11`, OpenGL). Nothing extra is needed on Windows or macOS.

## What is on screen

* **Ticker crawl**: one row per exchange, with index level and daily change.
* **Security list**: search by ticker or perk letters, filter by exchange, sort by column. It is virtualised, so 30k rows scroll smoothly.
* **Detail**: price, model fair value, mispricing, beta, rate duration, volatility, mispricing half-life, distance to default, persona (hover for perks), chart with a forecast cone (mean, 68% and 95% bands), and an order ticket.
* **Dock** (Classic layout) or **left column** (Terminal layout): yield curve with forward curve, market drivers, exchanges, population, market feed, personal summary.
* **Windows**: Brokers & Firms, Personal finance (positions, history, launch your own listing), Settings.

## Layout of the code

| Path | What |
|---|---|
| `crates/sigma-engine` | Simulation library. **No dependencies.** |
| `crates/sigma-app` | egui desktop app (`tixr-sigma` binary) |
| `docs/sigma-vs-alpha.md` | How the sigma engine differs from the original |
| `docs/personas-brokers-money.md` | Personas (perks A-P), brokers & firms, and what "money" means here |
| `docs/walkthrough.md` | Guided code tour with design rationale |

## Tests

```bash
cargo test --release                                            # unit + 400-day integration (~50 s)
cargo test --release -p sigma-engine -- --ignored               # 10-year soak
cargo run --release -p sigma-engine --example probe -- 1500 7   # long-run calibration printout
```
