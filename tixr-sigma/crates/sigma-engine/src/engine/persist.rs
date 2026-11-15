//! Saving and loading an [`Engine`].
//!
//! What goes where follows one rule: **if the simulation reads it, it is
//! state; if only the screen reads it, it is history.**
//!
//! * State (`state.tixr`, sealed): clock, RNG streams, economy, exchanges,
//!   every security's model state, every broker's book, the player ledger,
//!   counters, and the pending event schedule.
//! * History (`history.bin`, unsealed): the chart tiers of securities,
//!   brokers and market series.
//!
//! Saving the event schedule (about one pending event per security and per
//! broker) is what makes a loaded game continue *exactly* as the original
//! would have: same seed, same commands, same market, with or without a
//! save/load in the middle. The test at the bottom checks precisely that,
//! and it doubles as the guard against forgetting a field: anything the
//! simulation depends on that is not saved makes the two runs diverge.
//!
//! This is a child module of `engine` on purpose: child modules can see
//! their parent's private fields, so the engine's internals stay private to
//! the rest of the crate while the save code can still reach them.

use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use super::*;
use crate::history::Bar;
use crate::save::{self, FLAG_DEV_TIME, Persist, Reader, Result, SaveError, SaveHeader, Writer};

crate::record!(Stats {
    events,
    sec_ticks,
    closed_ticks,
    broker_decisions,
    ipos,
    bankruptcies,
    penny_delistings,
    acquisitions,
    fund_closures,
    splits,
    reverse_splits,
    rescues,
    brokers_broke,
    brokers_founded,
    repairs,
    trades,
});
crate::record!(Player { port, starting_cash, fills, companies });

impl Persist for Ev {
    fn put(&self, w: &mut Writer) {
        match *self {
            Ev::SecTick(id, epoch) => {
                0u8.put(w);
                id.put(w);
                epoch.put(w);
            }
            Ev::BrokerDecide(id) => {
                1u8.put(w);
                id.put(w);
            }
            Ev::ExchangeStep => 2u8.put(w),
            Ev::MacroStep => 3u8.put(w),
            Ev::DailyRoll => 4u8.put(w),
            Ev::Open(ex) => {
                5u8.put(w);
                ex.put(w);
            }
            Ev::Close(ex) => {
                6u8.put(w);
                ex.put(w);
            }
        }
    }
    fn get(r: &mut Reader) -> Result<Self> {
        let exchange = |r: &mut Reader| -> Result<u8> {
            let ex = u8::get(r)?;
            if (ex as usize) < EXCHANGE_COUNT { Ok(ex) } else { Err(SaveError::Corrupt("event exchange")) }
        };
        Ok(match u8::get(r)? {
            0 => Ev::SecTick(Persist::get(r)?, Persist::get(r)?),
            1 => Ev::BrokerDecide(Persist::get(r)?),
            2 => Ev::ExchangeStep,
            3 => Ev::MacroStep,
            4 => Ev::DailyRoll,
            5 => Ev::Open(exchange(r)?),
            6 => Ev::Close(exchange(r)?),
            _ => return Err(SaveError::Corrupt("event")),
        })
    }
}

/// Chart tiers of one entity, keyed by its raw id.
type HistoryEntry = (u32, Vec<Vec<Bar>>);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SaveReport {
    pub state_bytes: usize,
    pub history_bytes: usize,
    pub millis: u128,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoadReport {
    pub state_bytes: usize,
    /// Number of securities, brokers and series whose charts were restored;
    /// `Err` explains why history was skipped (the game loads regardless).
    pub history: std::result::Result<usize, SaveError>,
    pub millis: u128,
}

impl Engine {
    // ---------------------------------------------------------------- state

    /// Serialise everything the simulation reads. Field order here must
    /// match [`decode_state`](Self::decode_state).
    pub fn encode_state(&self) -> Vec<u8> {
        let mut w = Writer::default();
        self.now.put(&mut w);
        self.epoch_days.put(&mut w);
        self.dev_time_used.put(&mut w);
        self.rng.put(&mut w);
        self.name_rng.put(&mut w);
        self.macro_state.put(&mut w);
        self.exchanges.put(&mut w);
        self.slots.put(&mut w);
        self.generations.put(&mut w);
        self.free_slots.put(&mut w);
        self.live.put(&mut w);
        self.brokers.put(&mut w);
        self.broker_generations.put(&mut w);
        self.broker_free.put(&mut w);
        self.player.put(&mut w);
        self.carrying_capacity.put(&mut w);
        self.tick_epoch.put(&mut w);
        self.median_log_cap.put(&mut w);
        self.stats.put(&mut w);
        self.sector_equity.put(&mut w);
        self.feed.put(&mut w);
        self.queue.pending().put(&mut w);
        w.buf
    }

    /// Rebuild an engine from [`encode_state`](Self::encode_state) output.
    /// Derived data (ticker index, live positions, name index, factor
    /// cache, head counts) is recomputed rather than trusted from the file.
    pub fn decode_state(header: &SaveHeader, payload: &[u8]) -> Result<Engine> {
        let r = &mut Reader::new(payload);
        let now = f64::get(r)?;
        let epoch_days = u64::get(r)?;
        let dev_time_used = bool::get(r)?;
        let rng = Rng::get(r)?;
        let name_rng = Rng::get(r)?;
        let macro_state = MacroState::get(r)?;
        let exchanges = Vec::<Exchange>::get(r)?;
        let slots = Vec::<Option<Security>>::get(r)?;
        let generations = Vec::<u16>::get(r)?;
        let free_slots = Vec::<u16>::get(r)?;
        let live = Vec::<u16>::get(r)?;
        let brokers = Vec::<Option<Broker>>::get(r)?;
        let broker_generations = Vec::<u16>::get(r)?;
        let broker_free = Vec::<u16>::get(r)?;
        let player = Player::get(r)?;
        let carrying_capacity = f64::get(r)?;
        let tick_epoch = u32::get(r)?;
        let median_log_cap = f64::get(r)?;
        let stats = Stats::get(r)?;
        let sector_equity = <[f64; 2]>::get(r)?;
        let feed = <VecDeque<FeedItem> as Persist>::get(r)?;
        let pending = Vec::<(f64, Ev)>::get(r)?;
        r.finish()?;

        // Structural checks: a file can carry a valid seal and still come
        // from a buggy build, and indexing must never panic on load.
        let bad = |what: &'static str| Err(SaveError::Corrupt(what));
        if !now.is_finite() || now < 0.0 {
            return bad("clock");
        }
        if exchanges.len() != EXCHANGE_COUNT || exchanges.iter().enumerate().any(|(i, e)| e.id as usize != i) {
            return bad("exchanges");
        }
        if slots.len() > MAX_SECURITIES || generations.len() != slots.len() {
            return bad("security table");
        }
        if brokers.len() > MAX_PARTICIPANTS || broker_generations.len() != brokers.len() {
            return bad("broker table");
        }
        let mut live_pos = vec![u32::MAX; slots.len()];
        for (pos, &slot) in live.iter().enumerate() {
            let ok = slots.get(slot as usize).is_some_and(|s| s.as_ref().is_some_and(|s| s.id.slot() == slot as usize));
            if !ok || live_pos[slot as usize] != u32::MAX {
                return bad("live list");
            }
            live_pos[slot as usize] = pos as u32;
        }
        let listed = slots.iter().flatten().count();
        if listed != live.len() || free_slots.len() + listed != slots.len() {
            return bad("security slots");
        }
        if free_slots.iter().any(|&s| slots.get(s as usize).is_none_or(|x| x.is_some())) {
            return bad("free list");
        }
        if slots.iter().flatten().any(|s| s.exchange as usize >= EXCHANGE_COUNT) {
            return bad("security exchange");
        }
        if brokers.iter().enumerate().any(|(i, b)| b.as_ref().is_some_and(|b| b.id.slot() != i)) {
            return bad("broker ids");
        }
        let alive = brokers.iter().flatten().count();
        if broker_free.len() + alive != brokers.len()
            || broker_free.iter().any(|&s| brokers.get(s as usize).is_none_or(|x| x.is_some()))
        {
            return bad("broker slots");
        }
        if pending.iter().any(|(t, _)| !t.is_finite()) {
            return bad("event time");
        }

        let tickers: HashSet<Ticker> = slots.iter().flatten().map(|s| s.ticker).collect();
        let broker_names: HashSet<String> = brokers.iter().flatten().map(|b| b.name.clone()).collect();
        let firm_count = brokers.iter().flatten().filter(|b| b.is_firm()).count();
        let mut queue = CalendarQueue::new(4_096, 0.25);
        for (t, ev) in pending {
            queue.push(t, ev);
        }
        let mut eng = Engine {
            now,
            seed: header.world_seed,
            game_seed: header.game_seed,
            epoch_days,
            dev_time_used: dev_time_used || header.flags & FLAG_DEV_TIME != 0,
            prehistory: false,
            rng,
            name_rng,
            queue,
            macro_state,
            exchanges,
            factors: [[0.0; FACTOR_COUNT]; EXCHANGE_COUNT],
            slots,
            generations,
            free_slots,
            live,
            live_pos,
            tickers,
            brokers,
            broker_generations,
            broker_free,
            broker_names,
            firm_count,
            plain_count: alive - firm_count,
            decision_density: 1.0,
            player,
            market: (0..MARKET_SERIES_COUNT).map(|_| History::new(HistoryKind::Market)).collect(),
            sector_equity,
            broker_rows: Arc::new(Vec::new()),
            broker_rows_at: f64::NEG_INFINITY,
            selected_series: None,
            carrying_capacity,
            tick_density: 1.0,
            tick_epoch,
            median_log_cap,
            stats,
            feed,
            notices: Vec::new(),
            selected: None,
            focused_broker: None,
        };
        eng.refresh_factors();
        eng.health_check().map_err(|_| SaveError::Corrupt("non-finite values"))?;
        Ok(eng)
    }

    // -------------------------------------------------------------- history

    pub fn encode_history(&self) -> Vec<u8> {
        let securities: Vec<HistoryEntry> = self
            .live
            .iter()
            .filter_map(|&s| self.slots[s as usize].as_ref())
            .map(|s| (s.id.0, s.history.export()))
            .collect();
        let brokers: Vec<HistoryEntry> = self.brokers.iter().flatten().map(|b| (b.id.0, b.history.export())).collect();
        let market: Vec<Vec<Vec<Bar>>> = self.market.iter().map(History::export).collect();
        let mut w = Writer::default();
        securities.put(&mut w);
        brokers.put(&mut w);
        market.put(&mut w);
        w.buf
    }

    /// Attach chart history to the entities that still exist. Returns how
    /// many charts were restored. Entries for ids that are no longer alive
    /// are skipped, so a mismatched file can only cost you charts.
    pub fn apply_history(&mut self, body: &[u8]) -> Result<usize> {
        let r = &mut Reader::new(body);
        let securities = Vec::<HistoryEntry>::get(r)?;
        let brokers = Vec::<HistoryEntry>::get(r)?;
        let market = Vec::<Vec<Vec<Bar>>>::get(r)?;
        r.finish()?;
        let mut restored = 0;
        for (id, tiers) in securities {
            if let Some(s) = self.security_mut(SecurityId(id)) {
                s.history = History::import(HistoryKind::Price, tiers);
                restored += 1;
            }
        }
        for (id, tiers) in brokers {
            let id = BrokerId(id);
            if let Some(b) = self.brokers.get_mut(id.slot()).and_then(|b| b.as_mut()).filter(|b| b.id == id) {
                b.history = History::import(HistoryKind::Equity, tiers);
                restored += 1;
            }
        }
        for (h, tiers) in self.market.iter_mut().zip(market) {
            *h = History::import(HistoryKind::Market, tiers);
            restored += 1;
        }
        Ok(restored)
    }

    // ---------------------------------------------------------------- files

    fn save_header(&self, save_id: u64) -> SaveHeader {
        SaveHeader {
            world_seed: self.seed,
            game_seed: self.game_seed,
            save_id,
            flags: if self.dev_time_used { FLAG_DEV_TIME } else { 0 },
        }
    }

    /// The sealed state file as bytes (no disk involved; used by tests).
    pub fn to_sealed_bytes(&self, save_id: u64) -> Vec<u8> {
        save::seal(self.save_header(save_id), &self.encode_state())
    }

    pub fn from_sealed_bytes(file: &[u8]) -> Result<(Engine, SaveHeader)> {
        let (header, payload) = save::unseal(file)?;
        Ok((Engine::decode_state(&header, &payload)?, header))
    }

    /// Write this game to `root/slot/`: the sealed state, then the history
    /// sidecar, each atomically, under the slot lock.
    pub fn save_to(&self, root: &Path, slot: &str) -> Result<SaveReport> {
        let started = Instant::now();
        let dir = save::slot_dir(root, slot)?;
        let _lock = save::SlotLock::acquire(&dir)?;
        let save_id = save::fresh_seed();
        let state = self.to_sealed_bytes(save_id);
        let history = save::wrap_history(save_id, &self.encode_history());
        // History first: if we are interrupted between the two writes, the
        // old state file is still paired with a sidecar it will simply
        // ignore (different save id), never with a newer state.
        save::write_atomic(&dir.join(save::HISTORY_FILE), &history)?;
        save::write_atomic(&dir.join(save::STATE_FILE), &state)?;
        Ok(SaveReport { state_bytes: state.len(), history_bytes: history.len(), millis: started.elapsed().as_millis() })
    }

    /// Load `root/slot/`. A bad state file is an error; bad or missing
    /// history only costs the charts and is reported in the result.
    pub fn load_from(root: &Path, slot: &str) -> Result<(Engine, LoadReport)> {
        let started = Instant::now();
        let dir = save::slot_dir(root, slot)?;
        if !dir.join(save::STATE_FILE).is_file() {
            return Err(SaveError::Io(format!("no save in slot '{slot}'")));
        }
        let _lock = save::SlotLock::acquire(&dir)?;
        let state = std::fs::read(dir.join(save::STATE_FILE))?;
        let (mut eng, header) = Engine::from_sealed_bytes(&state)?;
        let history = std::fs::read(dir.join(save::HISTORY_FILE))
            .map_err(SaveError::from)
            .and_then(|file| save::unwrap_history(&file, header.save_id).and_then(|body| eng.apply_history(body)));
        Ok((eng, LoadReport { state_bytes: state.len(), history, millis: started.elapsed().as_millis() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console;
    use crate::history::ChartRange;

    fn small(seed: u64) -> Engine {
        console::set_level(Level::Off);
        Engine::new(EngineConfig {
            seed,
            game_seed: Some(1234),
            initial_securities: 1_200,
            brokers: 150,
            firms: 30,
            warmup: false,
            ..Default::default()
        })
    }

    /// Everything a player could observe about the market's state.
    fn fingerprint(e: &Engine) -> Vec<u64> {
        let mut v = vec![e.now.to_bits(), e.stats.events, e.stats.trades, e.live_count() as u64];
        v.extend(e.live_ids().map(|id| e.security(id).unwrap().price.to_bits()));
        v.extend(e.brokers().map(|b| b.port.cash.to_bits()));
        v.push(e.player.port.cash.to_bits());
        v.push(e.macro_state.policy_rate.to_bits());
        v.extend(e.exchanges.iter().map(|x| x.factor.to_bits()));
        v
    }

    #[test]
    fn loaded_game_continues_exactly_like_the_original() {
        let mut original = small(21);
        // Sparse prints keep the test quick; the density is not part of a
        // save (the runner tunes it live), so it is set on both engines.
        original.tick_density = 0.05;
        original.advance_to(1.6 * SECS_PER_DAY, None);
        let id = original.live_ids().find(|&id| original.security(id).unwrap().exchange == 0).unwrap();
        original.apply(&Command::Order { id, side: Side::Buy, size: OrderSize::Notional(25_000.0) });
        let file = original.to_sealed_bytes(1);

        let (mut loaded, header) = Engine::from_sealed_bytes(&file).unwrap();
        loaded.tick_density = original.tick_density;
        assert_eq!((header.world_seed, header.game_seed), (21, 1234));
        assert_eq!(fingerprint(&loaded), fingerprint(&original));
        assert!(loaded.player.port.shares_of(id) > 0.0, "player ledger restored");

        // Two more days, through closes, a daily roll and broker turnover.
        original.advance_to(3.7 * SECS_PER_DAY, None);
        loaded.advance_to(3.7 * SECS_PER_DAY, None);
        assert_eq!(fingerprint(&loaded), fingerprint(&original));
        loaded.health_check().unwrap();
    }

    #[test]
    fn saving_twice_gives_identical_state() {
        let mut e = small(22);
        e.tick_density = 0.05;
        e.advance_to(0.8 * SECS_PER_DAY, None);
        let first = e.encode_state();
        let (loaded, _) = Engine::from_sealed_bytes(&e.to_sealed_bytes(5)).unwrap();
        assert_eq!(loaded.encode_state(), first);
    }

    #[test]
    fn edited_state_is_refused() {
        let e = small(23);
        let mut file = e.to_sealed_bytes(1);
        let middle = file.len() / 2;
        file[middle] ^= 0x40;
        assert_eq!(Engine::from_sealed_bytes(&file).err(), Some(SaveError::Tampered));
    }

    #[test]
    fn slot_round_trip_on_disk_and_history_is_optional() {
        let root = std::env::temp_dir().join(format!("tixr-save-test-{}", save::fresh_seed()));
        let mut e = small(24);
        e.tick_density = 0.05;
        e.advance_to(0.7 * SECS_PER_DAY, None);
        let report = e.save_to(&root, "slot1").unwrap();
        assert!(report.state_bytes > 10_000 && report.history_bytes > 10_000);
        assert_eq!(save::list_slots(&root).len(), 1);

        let (loaded, load) = Engine::load_from(&root, "slot1").unwrap();
        assert!(load.history.clone().unwrap() > 1_000, "{:?}", load.history);
        let id = e.live_ids().next().unwrap();
        let chart = |x: &Engine| x.security(id).unwrap().history.points(ChartRange::Day, x.now);
        assert!(!chart(&e).is_empty());
        assert_eq!(chart(&loaded), chart(&e));

        // Damage the sidecar: the game still loads, only charts are gone.
        let side = save::slot_dir(&root, "slot1").unwrap().join(save::HISTORY_FILE);
        let mut bytes = std::fs::read(&side).unwrap();
        let n = bytes.len();
        bytes[n / 2] ^= 1;
        std::fs::write(&side, bytes).unwrap();
        let (loaded, load) = Engine::load_from(&root, "slot1").unwrap();
        assert!(load.history.is_err());
        assert_eq!(fingerprint(&loaded), fingerprint(&e));
        assert!(chart(&loaded).is_empty());

        // Damage the state: refused outright.
        let state = save::slot_dir(&root, "slot1").unwrap().join(save::STATE_FILE);
        let mut bytes = std::fs::read(&state).unwrap();
        bytes[100] ^= 1;
        std::fs::write(&state, bytes).unwrap();
        assert_eq!(Engine::load_from(&root, "slot1").err(), Some(SaveError::Tampered));
        assert!(Engine::load_from(&root, "empty").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
