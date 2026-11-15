# Stock Personas, Brokers & Firms, and how "Money" should be understood in Tixr-sigma

This document covers three things: what makes each security behave differently, who trades in the simulation, and what the dollar figures on screen actually mean.

---

## Part 1: Stock personas (perks A-P)

Every stock carries a **persona**: a set of 0 to 12 of the 16 perks lettered **A** to **P**, stored as a 16-bit mask. It appears as a short string such as `BGK` in the detail view (hover for the meaning of each letter), and you can search the list by perk letters.

### Where the list comes from

The list is a mix of the first sigma build's perks and the list in `A-P_Tixr_types_sigma.txt`. The rule for the mix was **one signature mechanism per perk**: each letter owns one behaviour that no other letter has, so two perks can never be "the same thing with different numbers". The test `every_perk_has_a_unique_signature` enforces it.

| Decision | Perks | Why |
|---|---|---|
| Taken from your list | Assets, Builder, Crypto, Data, Finance, Global, Hype, Innovate, Key, Legacy, Meager, Novel, Opportunity, Profit | Each has a distinct mechanism (table below) |
| Kept from the first build | **E** Expansion, **J** Junk | Nothing in your list covered long-duration growth (rate sensitivity) or leverage (credit sensitivity), and both give the yield curve and the credit spread something to bite on |
| Dropped: Equity | | "Slow, reliable appreciation, exceptional survivability" is Data (steady compounding) plus Meager (low volatility) plus Key (cannot disappear). It had no mechanism of its own |
| Dropped: Jumpy | | "Large swings regardless of the business" is what Crypto already does. Its fat symmetric jumps were folded into Crypto |

### How many perks a stock gets

The length is drawn from a fixed, right-skewed table (`persona::LENGTH_WEIGHTS`):

| Perks | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Weight % | 6 | 12 | 17 | 16 | 12 | 9 | 7 | 6 | 5 | 4 | 3 | 2 | 1 |

That gives a **median of 3**, a **mean of about 4.1**, and **11-12 perks in about 3%** of stocks. Letters are picked at random without repeats, skipping any letter that contradicts one already chosen (see "Exclusive pairs"). Index funds get a plain persona. When you launch your own company you can pick its perks by hand; contradictions are removed for you.

### What each perk does

| | Perk | Signature mechanism | Supporting changes | In plain words |
|---|---|---|---|---|
| **A** | Assets | Loads on the **inflation** factor (+2.5) | rate duration -1.5, fundamental vol x0.9 | Holds value when money is printed |
| **B** | Builder | **Contract wins**: rare, permanent upward steps in fundamentals, with the drift between wins lowered to pay for them | default barrier +0.1 | Staircase: flat stretches, then a step up |
| **C** | Crypto | **Fat, symmetric jumps**: 3x as often, 2.2x as large | mispricing vol x1.5, beta -0.2, barrier -0.35, prints x1.5 | Boom or bust |
| **D** | Data | **Steady drift**: +3%/yr growth with muted jumps (x0.6) | fundamental vol x0.8, barrier +0.5 | Compounds quietly |
| **E** | Expansion | **Long duration**: +4.5 years of rate sensitivity | growth +4.5%/yr, fundamental vol x1.2 | Growth story; falls when the 10Y rises |
| **F** | Finance | Loads on the **business-cycle** factor (running sum of the output gap) | beta +0.25, credit +1.5 | Outruns expansions, lags recessions |
| **G** | Global | **60% of its market exposure follows the world average** of all exchanges, and regimes hit it half as hard | inflation exposure x0.3, fundamental vol x0.9 | A local crisis is a smaller event |
| **H** | Hype | **Listing hump**: a mispricing bulge that peaks weeks after listing and unwinds over years | mispricing vol x1.5, reversion x0.6, growth -2%/yr | Mania, then a long hangover |
| **I** | Innovate | **Catalysts**: positive shocks to the *mispricing* term, so each one fades | growth +1%/yr, fundamental vol x1.1 | Pops on news, then drifts back |
| **J** | Junk | **Leverage**: credit exposure +3, default barrier -0.55 | beta +0.2, fundamental vol x1.15 | First to go in a credit crunch |
| **K** | Key | **Rescue**: at the default barrier it is recapitalised instead of delisted; below $1 it reverse-splits | barrier +0.6, impact x0.55, prints x1.5 | Too important to vanish |
| **L** | Legacy | **Decline**: after a grace period its fundamentals bleed, slowly at first | fundamental vol x0.8, beta -0.1, barrier +0.3 | Stable for years, then erodes |
| **M** | Meager | **Damping**: growth x0.4, fundamental vol x0.65, reversion x1.7 | beta -0.25, jumps x0.5 | Neither excels nor fails |
| **N** | Novel | **Verdict**: at a random date its fundamentals step up or down by 45-85%, 50/50, with higher volatility beforehand | barrier -0.15, impact x1.5, prints x0.6 | Unproven until the day it is proven |
| **O** | Opportunity | **Crashes that heal**: negative shocks to the *mispricing* term, so recovery is built in | growth +1%/yr | Sharp falls, real rebounds |
| **P** | Profit | Loads on the **recovery** factor, which climbs for about 300 days after a Crisis ends and 90 after a Correction | growth +1%/yr, barrier +0.2 | Earns most after it has been tested |

Builder, Innovate and Opportunity are the three "shock" perks, and they differ in where the shock lands. Builder's wins go into the **fundamental** and stay. Innovate's catalysts and Opportunity's crashes go into the **mispricing** and decay at the stock's own reversion speed.

### Exclusive pairs

Five pairs contradict each other in their own descriptions and are never combined (`persona::EXCLUSIVE_PAIRS`):

| Pair | Why |
|---|---|
| Crypto / Meager | "extreme boom or bust" against "consistent low volatility" |
| Crypto / Data | "catastrophic collapses equally likely" against "rarely experiences catastrophic decline" |
| Legacy / Novel | an established incumbent cannot be an unproven concept |
| Legacy / Expansion | slow loss of competitiveness against a growth story |
| Legacy / Hype | "stable early on" against a listing mania |

Everything else may stack. Opposites that only cancel in size (Meager with Junk, Key with Junk) are allowed: they produce a muted version of both, which is a legitimate character.

### Stacking audit: why no combination breaks the game

You asked for assurance that stacking cannot produce a flatline, a sine wave or a saw-tooth. Each one is ruled out by construction and then checked by a test.

| Failure | Why it cannot happen | Test |
|---|---|---|
| **Flatline** | After all multipliers, fundamental vol is clamped to at least 5%/yr, mispricing vol to at least 10%/yr and beta to at least 0.15. Even 12 damping perks leave a stock that moves every day | bounds test (all 65,536 masks); path test (no zero-return days) |
| **Sine wave** | The only restoring force on price is first-order mean reversion (an OU process). A first-order system decays toward its target; it has no second state to overshoot with, so it cannot oscillate. Reversion speed is clamped to 2-36 per year | path test: autocorrelation of daily returns below 0.25 at every lag from 1 to 60 days |
| **Saw-tooth** | A saw-tooth needs shocks on a regular beat. All shocks (wins, catalysts, crashes, jumps) arrive on Poisson clocks, which are memoryless, and their sizes are drawn relative to the stock's own noise. The transient ones are also capped in how much they can shift the average price (4% for catalysts, 10% for crashes) | path test: same autocorrelation check, plus longest run of same-sign days below 40 |
| **Runaway** | Additive effects are clamped per parameter; price above $2,500 splits; log price is clamped | bounds test |
| **Immortal or instant-death stocks** | The default barrier is clamped to a range, and Key rescues dilute the stock each time | bounds test; `rescue_restores_the_barrier_without_a_price_gap_beyond_dilution` |

The bounds test is `security::tests::all_65536_perk_sets_stay_in_safe_bounds` and the path test is `security::tests::stacked_personas_do_not_flatline_oscillate_or_sawtooth`. The path test simulates 500 random personas for 1,095 days each and checks every one: daily volatility between 0.25% and 20%, no zero-return day, no lag with meaningful autocorrelation, no long one-way run. Rare large events (a Novel verdict, a Crypto jump) are clipped before the autocorrelation is measured, so a single big day cannot mask or fake a pattern.

Two stacking effects are intended and worth knowing as a player:

* **Hype + Innovate + Crypto** is the wildest legal persona. It stays inside the 20% daily volatility ceiling the path test enforces, and it is rare and usually short-lived.
* **Key + Junk** goes to the barrier often and is rescued each time. Each rescue dilutes holders, so "cannot disappear" does not mean "cannot lose you money".

### Parameter glossary

* **beta**: loading on its exchange's market factor. A 1% index move gives about beta% in the stock.
* **duration**: loading on minus the 10Y yield. With duration 4, a +1 point rise in the 10Y costs about 4%.
* **inflation / credit exposure**: loadings on inflation and on minus the credit spread.
* **growth (alpha)** and **fundamental vol (sigma_f)**: drift and volatility of the firm's own earnings power `y`, plus compound-Poisson **jumps** (earnings surprises).
* **mispricing (u)**: how far the price is from model fair value, mean-reverting with half-life `ln 2 / kappa`.
* **default barrier**: the firm is bankrupt when its fair value falls this many log units below its listing level.
* **prints**: how often its price updates while its exchange is open. This affects only how often you see a new price, not how the price behaves.

---

## Part 2: Brokers & Firms

### How many of everything

| Limit | Value | Constant |
|---|---|---|
| Everything, all kinds together | 32,768 (2^15) | `engine::MAX_ENTITIES` |
| Stocks listed at once | 16,384 (2^14) | `engine::MAX_SECURITIES` |
| Stock floor | about 8,000 | `engine::TARGET_MIN` |
| Brokers and firms together | 16,384 | `broker::MAX_PARTICIPANTS` |
| Firms (15% of that) | 2,457 | `broker::MAX_FIRMS` |
| Plain brokers (85%) | 13,927 | `broker::MAX_PLAIN_BROKERS` |

A new game starts with 8,600 stocks and half the broker and firm caps. The stock count follows a carrying capacity `K` that the economy moves between 8,192 and 11,000. If the count ever nears 8,000 the engine pauses acquisitions and relists a quarter of the shortfall each day, so in a 2,000-day probe it stayed between 8,650 and 8,750. Brokers and firms drift toward a share of their caps that depends on the economy (35% in a slump, 95% in a boom).

### Who they are

A **firm** is a broker with a balance sheet: it starts with 5-25x more capital, has lower operating costs, and can also act as an **issuer**.

* **Underwriting.** About 40% of IPOs are underwritten by a firm. The firm earns a 2% fee on a 25% float and buys a 5% stake at the offer price.
* **Index funds.** Firms that have grown to 1.5x their starting capital occasionally launch an index fund (ticker ends in `X`). If the firm fails, its funds are wound up at their last price.

### Strategies

| Strategy | Buys when | Sells when |
|---|---|---|
| **Value** | estimated mispricing < -3% | mispricing > +1.5%, or stop/take-profit |
| **Whale** | mispricing < -6%, in large size | mispricing > +3% |
| **Momentum** | 5-session return > +3% (stronger on Hype stocks) | trend turns, or a tight 10% stop |
| **Contrarian** | sharp daily drop (> 4%) | sharp daily rally, or +12% profit |
| **Index** | funds and large caps | small caps; very loose stop |
| **Noise** | at random | at random |

With up to 16,384 participants instead of 256, each one decides about four times less often than in the first build (tens of minutes to hours of sim time), so total order flow per stock stays realistic. Every broker also has risk rules: a maximum number of names, a maximum weight per name, a cap of 5% of a stock's daily dollar volume per order, a stop-loss and a take-profit.

### Life cycle: active, broke, new

Brokers pay **operating costs** every sim hour. A broker **goes broke** when its equity falls below 50% of its starting capital, or below 30% of its peak. Its slot is freed, any funds it sponsored close, and the daily population step founds replacements with fresh names. Ids are generational, so nothing of the old broker carries over. In the probe, failures begin after about 450 days and reach roughly 400 by day 1,350.

### Charts for brokers and for the sector

Every broker and firm keeps an equity history (daily for a year, weekly for three years), shown in the Brokers & Firms window. The **Financials** window charts the sector as a whole (total broker equity and total firm equity) alongside the market-wide series: the four indices, policy rate, 10Y yield, inflation, credit spread and the number of listings.

---

## Part 3: How "money" should be understood in Tixr-sigma

Not every dollar figure on screen is the same kind of thing. Keeping them apart is the key to reading the simulation correctly.

### 1. Cash is the only money that changes hands

Each participant (you, every broker and firm) has a **cash ledger**. Cash moves only through explicit flows:

| Flow | From | To |
|---|---|---|
| Buy / sell | trader | **the public float** (see 2) |
| Trading fee (5 bps) | trader | leaves the system |
| Interest on cash (policy rate, hourly) | **outside the system** | holder |
| Operating costs | broker/firm | leaves the system |
| Underwriting fee | **outside the system** (the issuer) | firm |
| Exit payout (acquisition 130%, fund close 100%, penny 50%, bankruptcy 4% of last price) | **outside the system** (acquirer, liquidator) | holders |
| Listing your own company | you | becomes your founder stake (at offer price) plus a 1% fee that leaves the system |

The simulation is an **open system**. Cash is not conserved among the participants, just as a real brokerage's clients are not the whole economy.

### 2. Prices are quotes, not money

When you buy, the counterparty is the **public float**: the anonymous mass of investors outside the named brokers and firms. Their aggregate behaviour is what the model's noise and mispricing terms represent. A price is the rate at which that float will trade with you right now, and your own order moves it (square-root impact) before it fades.

### 3. Market cap is a mark, not a pile of cash

`market cap = price x shares outstanding`. A $3T company does not mean $3T exists anywhere. If every holder tried to sell, the price would fall long before the cap turned into cash. The same goes for **net worth** on the personal screen: cash plus holdings *marked at the last price*. **Unrealised P&L** is only a mark; **realised P&L** is cash that actually arrived.

### 4. Fair value is the model's opinion

The detail view shows **fair value** (the price with the mispricing term removed) and the **forecast cone**. They are model quantities: what the engine's own equations expect, given today's macro state, the exchange's valuation, and the stock's parameters. They are honest forecasts in the sense that the price really is generated by those equations, but the cone is wide on purpose. The 95% band is where the price will be 19 times out of 20, not a promise.

### 5. Money supply is macro context

**Money supply** (driver panel) is the economy-wide stock of money. It grows with nominal GDP, and faster when the policy rate is below neutral. It is not the sum of anyone's cash in the simulation. It sets context: loose money goes with low rates, high fair P/E and more IPOs. It is kept in log form internally, so it can grow for centuries without overflowing.

### 6. Units and time

* All amounts are in the same nominal dollars. There is no currency conversion between exchanges.
* Rates are annualised (`4.12%` means per year) and accrue continuously in sim time.
* **1 sim day = 90 real minutes at 1x.** A 5% annual interest rate is worth about 0.0137% per sim day, about 0.00915% per real hour at 1x.
* Inflation erodes the *real* value of cash. Holding only cash in a 3% inflation world loses purchasing power, even though the nominal balance grows with interest.

### Practical reading guide

* **Am I making money?** Look at **Total P&L** (net worth minus starting cash), which includes interest, fees, realised and unrealised P&L.
* **Can I spend it?** Only **Cash** is spendable. Positions must be sold first, and selling a large position moves its price against you.
* **Is the market expensive?** Compare each exchange's **P/E** with **Fair P/E** in the drivers panel. Above fair means valuations are stretched and the market factor is being pulled down.
* **Is a recession coming?** Watch **10Y-3M** (inversion), **output gap** and **credit spread**. These feed the regime chain's crash odds.
