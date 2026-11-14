# Stock Personas, Brokers & Firms, and how "Money" should be understood in Tixr-sigma

This document covers three things: what makes each security behave differently, who trades in the simulation, and what the dollar figures on screen actually mean.

---

## Part 1: Stock personas (perks A-P)

Every stock carries a **persona**: a set of 0 to 12 of the 16 perks lettered **A** to **P**, stored as a 16-bit mask. It appears as a short string such as `BGL` in the detail view (hover for the meaning of each letter), and you can search the list by perk letters.

### How many perks a stock gets

The length is drawn from a fixed, right-skewed table (`persona::LENGTH_WEIGHTS`):

| Perks | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Weight % | 6 | 12 | 17 | 16 | 12 | 9 | 7 | 6 | 5 | 4 | 3 | 2 | 1 |

That gives a **median of 3**, a **mean of about 4.1**, and **11-12 perks in about 3%** of stocks. The brief asked for an average of about 4-7 and a median of 2-3. Both can hold only with a long right tail, which is what this table has: most stocks are simple, and a few are "characters". The test `length_distribution_matches_brief` pins these numbers.

Letters are then picked at random without repeats. Two contradictory pairs are never drawn together: **Cyclical/Defensive** and **Liquid/Illiquid**. Index funds get a plain persona (just **L**). When you launch your own company you can pick its perks by hand.

### What each perk does

Perks change the security's model parameters when it lists. Multipliers multiply, offsets add, and the result is clamped to a safe range, so no combination of 12 perks can destabilise the model.

| | Perk | Effect on the model | In plain words |
|---|---|---|---|
| **A** | Anchor | reversion speed x1.8, mispricing vol x0.8 | Snaps back to fair value fast |
| **B** | Blue-chip | fundamental vol x0.7, default barrier +0.8, prints x1.3, impact x0.6 | Large, steady, hard to kill |
| **C** | Cyclical | beta +0.35, credit exposure +1.5 | Amplifies the market; hurt by widening credit spreads |
| **D** | Defensive | beta -0.30, fundamental vol x0.85, jumps x0.7 | Damps the market |
| **E** | Expansion | growth +4%/yr, fundamental vol x1.2, rate duration +4 | Growth story; very sensitive to the 10Y yield |
| **F** | Frothy | mispricing vol x1.5, reversion x0.6 | Sentiment runs hot and stays hot |
| **G** | Gambler | jump rate x2.5, jump mean +3%, prints x1.6, mispricing vol x1.3 | Meme stock |
| **H** | Hard assets | inflation exposure +3, beta -0.1, duration -1 | Commodity-like; gains when inflation rises |
| **I** | Illiquid | prints x0.35, impact x2.5 | Rarely prints; orders move it a lot |
| **J** | Junk | default barrier -0.6, credit exposure +3, beta +0.2, fundamental vol x1.15 | Leveraged; first to go in a credit crunch |
| **K** | Kingpin | growth -1%/yr, barrier +0.4, fundamental vol x0.8, duration +1.5 | Mature dividend payer; bond-like |
| **L** | Liquid | prints x1.8, impact x0.5 | Constant prints; deep market |
| **M** | Momentum | reversion x0.7 (and momentum brokers weight it 1.5x) | Trends persist |
| **N** | Novel | fundamental vol x1.4, jumps x1.5, jump mean +4%, growth +2%, barrier -0.2 | Innovator with upside jumps |
| **O** | Oligopoly | fundamental vol x0.75, growth +1%, barrier +0.5, reversion x1.2 | Moat |
| **P** | Phoenix | barrier -0.5, jump mean +2%, never penny-delisted | Survives deep drawdowns |

### Parameter glossary

* **beta**: loading on its exchange's market factor. A 1% index move gives about beta% in the stock.
* **duration**: loading on minus the 10Y yield. With duration 4, a +1 point rise in the 10Y costs about 4%.
* **inflation / credit exposure**: loadings on inflation and on minus the credit spread.
* **growth (alpha)** and **fundamental vol (sigma_f)**: drift and volatility of the firm's own earnings power `y`, plus compound-Poisson **jumps** (earnings surprises).
* **mispricing (u)**: how far the price is from model fair value, mean-reverting with half-life `ln 2 / kappa`.
* **default barrier**: the firm is bankrupt when its fair value falls this many log units below its listing level. A barrier of 1.6 means a fall to about 20% of the listing fair value.
* **prints**: how often its price updates while its exchange is open. This affects only how often you see a new price, not how the price behaves.

---

## Part 2: Brokers & Firms

### Who they are

256 simulated participants. About a quarter start as **firms**; the rest are **brokers**. A firm is a broker with a balance sheet: it starts with 5-25x more capital, has lower operating costs, and can also act as an **issuer**.

* **Underwriting.** About 40% of IPOs are underwritten by a firm. The firm earns a 2% fee on a 25% float and buys a 5% stake at the offer price. The feed and the Brokers window show this ("listings underwritten").
* **Index funds.** Firms that have grown to 1.5x their starting capital occasionally launch an index fund (ticker ends in `X`, beta 1 to its exchange, very low idiosyncratic risk). If the firm fails, its funds are wound up at their last price.

So "brokers can be firms too" holds literally: a firm trades exactly like a broker, and issuing is an extra capability.

### Strategies

| Strategy | Buys when | Sells when | Typical pace |
|---|---|---|---|
| **Value** | estimated mispricing < -3% | mispricing > +1.5%, or stop/take-profit | every 5-30 sim min |
| **Whale** | mispricing < -6%, in large size | mispricing > +3% | every 1-4 sim h |
| **Momentum** | 5-session return > +3% (x1.5 on perk M) | trend turns, or a tight 10% stop | every 5-30 sim min |
| **Contrarian** | sharp daily drop (> 4%) | sharp daily rally, or +12% profit | every 5-30 sim min |
| **Index** | funds and large caps | small caps; very loose stop | every 0.5-2 sim h |
| **Noise** | at random | at random | every 3-15 sim min |

Every broker also has risk rules: a maximum number of names (20-60 by strategy), a maximum weight per name (10%, or 25% for Index), a cap of 5% of a stock's daily dollar volume per order, a stop-loss and a take-profit.

### Life cycle: active, broke, new

Brokers pay **operating costs** every sim hour: 4-12% of starting capital per year for brokers, 2-5% for firms. That is the steady bleed that sinks participants without an edge. A broker **goes broke** when its equity falls below 50% of its starting capital, or below 30% of its peak (clients withdraw). Its slot is freed, any funds it sponsored close, and a new broker or firm is founded in that slot a few sim days later with a fresh name, strategy and capital. Ids are generational, so nothing of the old broker carries over.

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

When you buy, the counterparty is the **public float**: the anonymous mass of investors outside the 256 named participants. Their aggregate behaviour is what the model's noise and mispricing terms represent. A price is the rate at which that float will trade with you right now, and your own order moves it (square-root impact) before it fades.

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
