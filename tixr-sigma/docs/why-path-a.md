# Why path A (event-driven) over path B (budgeted sampling)

Both paths answer the same brief: a stock is updated only when it is randomly picked, and a stock that has not been picked for a while must not be "behind". Both solve the second half the same way, with price models that can jump any length of time in one exact step. They differ in **who does the picking**.

* **Path A (this directory):** every stock, broker and exchange owns a random clock. All clocks sit in one event queue, and the engine simply plays events in time order.
* **Path B (`sigma-b/`, compared at commit `36e0267`):** a loop runs 30 times per real second. Each pass it has a budget of updates and spends it by weighted random draws, with on-screen stocks weighted 8x and held stocks 20x. A sweep every 30 sim minutes catches up everything that was not drawn.

This document argues for A. Section 3 lists where B is better, because a one-sided comparison is not useful for choosing.

---

## 1. Where A's choices are better

### The market does not depend on the computer or the viewer
In A, the simulation is a function of the seeds and the player's orders. Which stock you are looking at, how fast your CPU is, and how many frames were drawn do not enter it. In B, the budget adapts to real time and the weights depend on what is on screen, so the sequence of random draws depends on the machine and on the viewer. B's statistics stay correct, because its catch-up is exact too, but two runs of the same seed do not produce the same market.

This matters for three things you asked for.

* **Time-locked consistency.** You want a real game to be "forever time-locked for consistency". A's world is the same world on every machine at 1x. The test `same_seed_same_market_regardless_of_chunking` pins it. (The one caveat: if a machine cannot keep up, A thins out how often prices *print*. That changes the exact path, never the statistics. At the locked 1x pace the engine uses a few percent of one core, so this does not trigger in practice.)
* **Save and load.** A saves the pending event queue along with the state, so a loaded game continues *exactly* as the original would have. The test `loaded_game_continues_exactly_like_the_original` runs a game, saves it, runs on, then loads the save and checks the two futures match. B can restore state, but the continuation is a different random future, so that test cannot be written for it.
* **Bug reports.** "Seed 42, day 317, stock XYZ did something odd" can be replayed in A. In B it cannot be replayed exactly.

### Each stock keeps its own pace
In A, how often a stock prints is a property of the stock (large caps print often, small ones rarely, Key and Crypto perks print more, Novel less) and of its exchange's hours. In B, how often a stock updates is mostly a property of the budget and of the viewer's attention. Watching a stock makes it update more. That is a sensible display optimisation, but it means the tape of a stock you open differs from the tape it had a second earlier.

### Idle time costs nothing, busy time is bounded
A does work only when an event is due. Overnight and at weekends, when every exchange is shut, the queue is nearly empty and the engine thread sleeps. B runs its 30 passes per second regardless. In the other direction, A's daily work is fixed by the model (about 12,000 events per real second at 1x with 8,700 stocks and 9,600 brokers and firms), so performance is predictable and does not need a feedback loop to stay inside a frame.

### Sessions, closes and charts are exact without a sweep
A catches every stock on an exchange up to the instant of each open and close, as an event. Daily closes, the eight chart ranges, penny checks and splits therefore use exact values for every stock, including ones nobody looked at. B needs its periodic sweep to bound staleness, which is a second update mechanism with its own cost and its own tuning.

### Every formula is closed-form
All of A's price terms have exact transitions, including the time-varying ones added in this revision (listing hype, legacy decline, novel verdict). B approximates its growth integral with a trapezoid rule, which is accurate for short gaps and less so for long ones. Small, but it is the kind of term that is hard to notice when it drifts.

### One mechanism for everything
Stock prints, broker decisions, macro steps, opens, closes, IPOs and the midnight roll are all the same thing in A: an event with a time. Adding a new behaviour means adding an event type. In B, things that are not "update a stock" need their own cadence beside the sampling loop.

---

## 2. The same comparison as a table

| Question | Path A | Path B |
|---|---|---|
| Who picks the next stock? | Its own Poisson clock | A weighted draw from a per-pass budget |
| Same seed gives the same market? | Yes (tested) | No; depends on real time and on-screen weights |
| Exact continuation after load? | Yes (queue is saved; tested) | State only |
| Does watching a stock change its update rate? | No | Yes (8x on screen, 20x held) |
| Work while all exchanges are shut | Near zero | 30 passes per second |
| Staleness bound | Session opens and closes, as events | 30-minute sweep |
| Time integrals | Closed-form | Trapezoid for growth |
| Scheduler | Calendar queue, O(1) | Fenwick tree, O(log n) per draw |

---

## 3. Where path B is better

* **Simpler main loop.** "Each pass, draw k stocks and update them" is easier to read and to debug than an event queue with epochs and stale-event handling. A's queue needs a differential test to trust; B's loop mostly does not.
* **Load control is built in.** B's budget *is* its CPU limit. A needs a separate feedback controller (print density) for fast-forward in dev builds.
* **Freshness where you look.** Under heavy load, B keeps the stocks you watch and hold fresh by design. A treats all stocks alike and thins all prints together.
* **Richer market structure today.** B has 11 sectors and a variance-time model (volatility clustering). A has 4 exchanges, 7 shared factors and perks, but no sectors and no clustering. Both could be added to A (a sector is one more factor; variance time is a time change on `advance`), but they are not there now.
* **Fewer dependencies.** B's graph is 125 crates; A's default build is 151 (124 with X11 only).

## 4. Recommendation

If the game is single-player, time-locked, and saved to disk, reproducibility is worth more than loop simplicity, and A gives it for free. If you later want sectors and volatility clustering, port those two ideas from B into A's model rather than adopting B's scheduler: they are model features, and they do not depend on how stocks are picked.
