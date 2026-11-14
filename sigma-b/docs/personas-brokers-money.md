# Stock Personas, Brokers & Firms, and How "Money" Should Be Understood in Tixr-sigma

This document explains three ideas that make Tixr-sigma behave like a market
rather than a set of random walks: every stock has a **persona**, the market is
populated by **brokers that can also be firms**, and **money** follows explicit
rules about where it comes from and where it goes.

It describes path B (the sampled engine). Code references point into
`crates/sigma-core/src`.

---

## 1. Stock personas (perks A to P)

### What a persona is

Every security carries a set of **perks**, each named by a letter from A to P.
A persona is the combination, written in alphabetical order: `ADP` is an
Anchor, Dividend payer, with Pricing power. A persona with no perks shows `-`.

Sixteen letters is not an accident: a persona is stored as a single `u16`
bitmask (`persona::PerkSet`), one bit per letter. That costs two bytes per
stock, and "does this stock have perk X" is one CPU instruction.

| Letter | Perk | What it does to the stock |
|---|---|---|
| A | Anchor | Blue chip: calmer fundamentals and sentiment, pulled back to fair value faster, rarely taken over, draws attention |
| B | Bellwether | Higher market beta, deeper liquidity, draws attention |
| C | Cyclical | Earnings swing roughly twice as hard with the output gap |
| D | Dividend | Pays out a larger share of earnings, grows a bit slower |
| E | Emerging | Faster growth, bumpier fundamentals, a small extra risk premium |
| F | Fortress | Less debt, much lower default risk |
| G | Gambit | Speculative: news jumps 2.5x as often and 1.5x as large |
| H | Hype | Large sentiment swings that fade slowly, very high attention |
| I | Innovator | Reinvests (low payout), fast growth, positive news skew |
| J | Junk credit | Heavy debt: higher default risk, higher cost of equity |
| K | Kontrarian | Defensive: low beta, ignores the business cycle |
| L | Low float | Few shares trade, so orders move the price much more |
| M | Momentum | Mispricing corrects at half speed, so trends run longer |
| N | Network | Extra growth that fades slowly (network effects) |
| O | Overseas | Sells abroad: less tied to the domestic cycle and its sector |
| P | Pricing power | Passes inflation straight through to earnings; steadier margins |

### Conflicting pairs

Four pairs cannot share a persona: **A/G** (a blue chip is not a gamble),
**F/J** (a fortress balance sheet is not junk), **B/K** (a bellwether cannot be
contrarian) and **D/I** (a dividend payer does not reinvest everything).
Removing one letter from each pair leaves 12 compatible letters, which is
exactly why the maximum persona length is 12.

### How many perks a stock gets

The request was: lengths 0 to 12, "average around 4 to 7", rare 11 to 12,
median 2 to 3. A median below the mean means a **right-skewed** distribution:
most stocks are fairly plain, and a long tail are loaded with character. The
weights are written out explicitly in `persona::LENGTH_WEIGHTS`:

| Perks | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Share | 5% | 13% | 20% | 15% | 10% | 8% | 7% | 7% | 6% | 4% | 3% | 1.5% | 0.5% |

That gives a median of 3, a mean of about 4.06 and a 2% chance of 11 or 12. A
unit test (`length_distribution_matches_spec`) draws 100,000 personas and
checks all three numbers, so a future tweak cannot silently break the spec.

Letters are not equally common. B, C, D, K, L and O are common; A, F, M, N
and P less so; E, G, H, I and J are rare. The stock's **sector** then tilts the
odds: a tech listing is 2.5 times as likely to be an Innovator, a utility 2.5
times as likely to pay Dividends. So personas also tell you something about
the sector they come from.

### How perks stack

Each perk nudges a stock's parameters: volatility, beta, growth, payout,
leverage, jump rate, liquidity and so on. Nudges **add up in log space**
(multipliers multiply) and then pass through a **soft cap**:

```
effect = cap * tanh(sum_of_nudges / cap)
```

Near zero this is just the sum. As the sum grows it flattens out, so every
multiplier stays inside [1/3, 3]. Two volatile perks are more volatile than
one, but twelve perks can never create a stock with absurd parameters. This is
one of the stabilising forces that tixr alpha lacked.

### Where you see it

The list shows each stock's letters, and the Perks filter box keeps only
stocks that have all the letters you type (for example `GH` for speculative
crowd favourites). The detail panel shows each perk as a chip; hover a chip to
read what it does.

---

## 2. Brokers and firms

### Brokers

A broker is a **portfolio, a strategy and a life cycle** (`broker.rs`). There
are 160 broker seats. Each broker is launched with capital drawn from a wide
distribution (a few tens of millions to tens of billions; index funds are
larger) and one of six strategies:

| Strategy | Buys | Sells | Role in the market |
|---|---|---|---|
| Value | stocks trading 4%+ below model value | when the gap closes or after a 40% gain | The arbitrage force that pulls prices back to fair value |
| Momentum | today's winners (up 1%+) | on a 2% down day or an 8% loss | Pushes trends further |
| Index | the largest companies | only to raise cash | Slow, steady demand for big caps |
| Income | dividend yields above 3% | when the yield falls below 1.5% | Supports dividend payers |
| Speculator | the most volatile names, doubly so for G and H perks | at +15% or -10% | Adds noise and froth |
| Market maker | stocks down 1.5%+ today | stocks up 1.5%+ or after a 3% gain | Leans against intraday moves, provides liquidity |

Brokers are **sampled**, just like stocks. Each tick a Poisson-distributed
number of brokers is drawn to act, so on average each one decides every ten
simulated minutes during market hours. When a broker acts it collects
interest and dividends, marks its portfolio to market, sells whatever its
strategy says to sell, and considers one new purchase from eight candidates
drawn from the same weighted sampler the price engine uses.

Brokers cannot short and cannot borrow. A trade that would make cash negative
is refused. Their orders move prices through the **square-root impact law**
(impact grows with the square root of order size relative to daily volume),
and that impact is added to the stock's mispricing, so it fades over the
following weeks like real price pressure does.

### Life cycle: active, broke, new

A broker is declared **broke** when its equity falls below a quarter of the
capital it started with. Its positions are sold at market, whatever cash is
left goes to its creditors (it leaves the simulation), and the seat stays
empty for one to five simulated days before a new broker with a new name,
strategy and capital takes it. Over a simulated year a handful of brokers go
broke, which is the intended churn.

### Brokers that are firms

About a quarter of brokers are also **listed firms**. When such a broker
launches, a security is listed for it in the Financials sector (with a persona
drawn using the Financials affinities). That stock is valued differently from
operating companies, using the **residual income** view banks and brokers are
usually valued with:

```
price-to-book = 1 + (ROE - cost of equity) / (cost of equity - growth)   (clamped to 0.3..4)
fair value    = broker equity / shares * price-to-book
```

So a broker firm's share price rises when its trading makes it richer and
trades above book only while its return on equity beats its cost of equity.
Market sentiment and the firm's own mispricing still apply on top.

The link runs both ways for failure: **if a listed broker goes broke, its
stock defaults** and is removed from the market. Every portfolio holding it,
including other brokers', takes the loss. A broker never picks its own stock
as a purchase candidate.

The Brokers & firms window lists every seat with its strategy, ticker (if
listed), equity, performance against starting capital, return on equity, cash
share, positions and trade count. Click a broker to see its top holdings.

---

## 3. How "money" should be understood

A market simulation gets nonsensical quickly if money appears from nowhere or
vanishes without a trace. Tixr-sigma separates three things that are easy to
mix up.

### 3.1 Cash, wealth and market capitalisation are different

* **Cash** is money held by the player and by brokers. It is the only thing
  you can spend.
* **Market capitalisation** is price times shares, summed over all
  securities. It is a valuation, not money. When prices rise, market cap rises,
  but nobody has more cash until they sell, and when they sell, the buyer has
  less.
* **Wealth (equity)** of a participant is cash plus their shares at today's
  price. It moves with the market even when no cash changes hands.

The "Money in the system" table in the Brokers window shows total cash and
total market cap side by side. The gap between them is the point: most of
the market's "value" exists only as prices.

### 3.2 Trading moves cash, it does not create it

When a broker buys from the market, its cash goes down by the order value plus
a 5 basis point fee and its shares go up. The counterparty in Tixr-sigma is
the market itself (the simulation does not match individual orders), so a
trade transfers cash between a participant and "the market" at the current
price, and the price impact of the trade shifts the stock's mispricing.

### 3.3 Where cash enters and leaves

Every place cash is created or destroyed is named and counted
(`engine.rs`, struct `Money`):

| Flow | Direction | Why |
|---|---|---|
| Interest on cash | in | Cash earns the central bank policy rate, continuously compounded. This is the risk-free return any investment has to beat. |
| Dividends | in | Companies pay out `payout ratio x earnings`, accrued daily on calendar time. |
| Takeover payouts | in | An acquirer pays a 15 to 45% premium in cash to everyone holding the target. |
| New broker capital | in | A new broker arrives with fresh capital raised outside the market. |
| Fees | out | 5 bp on every trade, plus 1% when the player lists a company. |
| Bankrupt broker leftovers | out | Whatever a broke broker still has goes to its creditors outside the market. |
| Defaults | out (as wealth) | Shares in a defaulted company become worth nothing. No cash moves, wealth disappears. |
| Delistings | partial | Shares that fall under 1.00 for 30 closes are delisted and cashed out at half the last price (the over-the-counter discount). |
| Player IPO | out (into the company) | The capital you invest becomes the company's; you get shares in return. |

### 3.4 Lazy settlement, not lost money

Dividends and exits are settled **lazily**, using the same anchor trick the
price engine uses. Each security keeps a running total of dividends paid per
share since listing. Each position remembers that total from the last time it
was settled, and is owed the difference times its shares. When a security
exits, its final payout per share is recorded, and holders are paid the next
time they act (brokers) or immediately (the player); a daily sweep settles any
broker that did not act. Nothing is lost, it is just paid when someone looks.

### 3.5 Base shares and splits

When a stock trades above 1,000 it splits 10 for 1. Positions are stored in
**base shares** (shares before any split), and each security keeps its
cumulative split factor, so a split changes the display, never anyone's
wealth, and never needs to touch a portfolio.

### 3.6 Nominal versus real

Prices are **nominal**: they include inflation. Company earnings rise with the
consumer price level (fully for Pricing-power stocks, about 70% for others)
and with real GDP. A portfolio that merely keeps pace with inflation has
gained nothing real. The CPI index in the drivers panel lets you deflate any
number: real value = nominal value x 100 / CPI index.

### 3.7 Why nothing can go to infinity

* Cash cannot go negative (no shorting, no margin), so equity cannot either.
* Every macro variable and every stock's mispricing and volatility state is a
  mean-reverting process with a stationary distribution.
* Valuation multiples are bounded (3x to 45x earnings, 0.3x to 4x book).
* Every perk effect is soft-capped.
* Prices are clamped to a finite range as a final guard, and an audit checks
  every number every five seconds and reports anything non-finite on the
  console.

The soak test (`cargo run -p sigma-core --release --example soak -- 1000`)
runs 1,000 simulated days at full scale and fails loudly if any of this is
violated.
