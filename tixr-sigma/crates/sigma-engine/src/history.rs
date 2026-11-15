//! Multi-resolution chart history ("recalled data").
//!
//! The UI needs eight chart ranges per security (Hour, 1D, 3D, Week, Month,
//! Year, 3Y, 10Y) and each must draw instantly. Re-sampling a long raw tick
//! log on every frame would not be instant, and caching rendered images
//! would cost far more memory than the numbers they show. So the engine
//! *pre-aggregates while it records*: every print is written into a handful
//! of ring buffers ("tiers") of different bar widths, and a chart range is
//! simply "the last N seconds of tier T". Drawing a range is then a copy of
//! at most a few hundred points.
//!
//! ```text
//!   print at time t, value v
//!        │
//!        ├─▶ tier 0: 1-minute bars  (ring of 64)   ─▶ Hour
//!        ├─▶ tier 1: 5-minute bars  (ring of 320)  ─▶ 1D, 3D
//!        ├─▶ tier 2: 30-minute bars (ring of 384)  ─▶ Week, Month
//!        ├─▶ tier 3: daily bars     (ring of 272)  ─▶ Year
//!        └─▶ tier 4: weekly bars    (ring of 528)  ─▶ 3Y, 10Y
//! ```
//!
//! A bar is stored as `(bucket index, last value)`, where the bucket index
//! is `floor(t / width)`. A print in the bucket of the newest bar overwrites
//! it; a print in a later bucket appends. Buckets with no print are simply
//! absent, so closed-market hours cost nothing and need no special casing.
//! Cost per print: one compare-and-write per tier.
//!
//! History is *display data*. Nothing in the simulation reads it, which is
//! why it can live outside the tamper-checked save file (see
//! [`crate::save`]).

use crate::time::{SECS_PER_DAY, SECS_PER_HOUR, SimTime};

/// The eight chart ranges offered for every security.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ChartRange {
    Hour = 0,
    Day,
    ThreeDay,
    Week,
    Month,
    Year,
    ThreeYear,
    TenYear,
}

pub const ALL_RANGES: [ChartRange; 8] = [
    ChartRange::Hour,
    ChartRange::Day,
    ChartRange::ThreeDay,
    ChartRange::Week,
    ChartRange::Month,
    ChartRange::Year,
    ChartRange::ThreeYear,
    ChartRange::TenYear,
];

impl ChartRange {
    pub fn label(self) -> &'static str {
        ["1H", "1D", "3D", "1W", "1M", "1Y", "3Y", "10Y"][self as usize]
    }

    /// Length of the window in sim seconds.
    pub fn window_secs(self) -> f64 {
        match self {
            ChartRange::Hour => SECS_PER_HOUR,
            ChartRange::Day => SECS_PER_DAY,
            ChartRange::ThreeDay => 3.0 * SECS_PER_DAY,
            ChartRange::Week => 7.0 * SECS_PER_DAY,
            ChartRange::Month => 30.0 * SECS_PER_DAY,
            ChartRange::Year => 365.0 * SECS_PER_DAY,
            ChartRange::ThreeYear => 3.0 * 365.0 * SECS_PER_DAY,
            ChartRange::TenYear => 10.0 * 365.0 * SECS_PER_DAY,
        }
    }
}

/// One resolution level: bar width in sim seconds and ring capacity.
#[derive(Clone, Copy, Debug)]
pub struct TierSpec {
    pub width: f64,
    pub cap: usize,
}

const MIN: f64 = 60.0;
const WEEK: f64 = 7.0 * SECS_PER_DAY;

/// What a history tracks; decides the tier layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HistoryKind {
    /// A security price. Sized for exchange sessions (at most 8.5 h a day,
    /// 5 days a week), because bars only exist while the market is open.
    Price = 0,
    /// A market-wide series recorded around the clock (indices, rates).
    Market,
    /// A broker's equity, recorded once per sim day.
    Equity,
}

impl HistoryKind {
    pub fn tiers(self) -> &'static [TierSpec] {
        const PRICE: [TierSpec; 5] = [
            TierSpec { width: MIN, cap: 64 },
            TierSpec { width: 5.0 * MIN, cap: 320 },
            TierSpec { width: 30.0 * MIN, cap: 384 },
            TierSpec { width: SECS_PER_DAY, cap: 272 },
            TierSpec { width: WEEK, cap: 528 },
        ];
        const MARKET: [TierSpec; 6] = [
            TierSpec { width: MIN, cap: 64 },
            TierSpec { width: 5.0 * MIN, cap: 300 },
            TierSpec { width: 30.0 * MIN, cap: 340 },
            TierSpec { width: 4.0 * SECS_PER_HOUR, cap: 190 },
            TierSpec { width: SECS_PER_DAY, cap: 372 },
            TierSpec { width: WEEK, cap: 528 },
        ];
        const EQUITY: [TierSpec; 2] = [TierSpec { width: SECS_PER_DAY, cap: 372 }, TierSpec { width: WEEK, cap: 160 }];
        match self {
            HistoryKind::Price => &PRICE,
            HistoryKind::Market => &MARKET,
            HistoryKind::Equity => &EQUITY,
        }
    }

    /// Which tier serves a chart range.
    pub fn tier_for(self, range: ChartRange) -> usize {
        use ChartRange::*;
        match self {
            HistoryKind::Price => match range {
                Hour => 0,
                Day | ThreeDay => 1,
                Week | Month => 2,
                Year => 3,
                ThreeYear | TenYear => 4,
            },
            HistoryKind::Market => match range {
                Hour => 0,
                Day => 1,
                ThreeDay | Week => 2,
                Month => 3,
                Year => 4,
                ThreeYear | TenYear => 5,
            },
            HistoryKind::Equity => match range {
                ThreeYear | TenYear => 1,
                _ => 0,
            },
        }
    }

    pub fn from_u8(v: u8) -> Option<HistoryKind> {
        [HistoryKind::Price, HistoryKind::Market, HistoryKind::Equity].get(v as usize).copied()
    }
}

/// One bar: bucket index (`floor(t / width)`) and the last value seen in it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    pub bucket: u32,
    pub value: f32,
}

/// Fixed-capacity ring buffer of bars, oldest evicted first.
///
/// A plain `Vec` plus a head index instead of `VecDeque`: `VecDeque` rounds
/// its allocation up to a power of two, which would waste up to half the
/// memory across ~8,000 securities x 5 tiers.
#[derive(Clone, Debug, Default)]
pub struct Ring {
    bars: Vec<Bar>,
    /// Index of the oldest bar once the ring is full.
    head: usize,
}

impl Ring {
    pub fn len(&self) -> usize {
        self.bars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bars.is_empty()
    }

    fn newest_mut(&mut self) -> Option<&mut Bar> {
        let n = self.bars.len();
        if n == 0 {
            return None;
        }
        let i = (self.head + n - 1) % n;
        self.bars.get_mut(i)
    }

    fn push(&mut self, bar: Bar, cap: usize) {
        if self.bars.len() < cap {
            if self.bars.len() == self.bars.capacity() {
                // Grow geometrically but never past the cap.
                let extra = self.bars.len().max(8).min(cap - self.bars.len());
                self.bars.reserve_exact(extra);
            }
            self.bars.push(bar);
        } else {
            self.bars[self.head] = bar;
            self.head = (self.head + 1) % cap;
        }
    }

    /// Bars from oldest to newest.
    pub fn iter(&self) -> impl Iterator<Item = Bar> + '_ {
        let (newer, older) = self.bars.split_at(self.head.min(self.bars.len()));
        older.iter().chain(newer.iter()).copied()
    }

    fn scale(&mut self, f: f32) {
        for b in &mut self.bars {
            b.value *= f;
        }
    }
}

/// Tiered history of one value.
#[derive(Clone, Debug)]
pub struct History {
    kind: HistoryKind,
    tiers: Vec<Ring>,
}

impl History {
    pub fn new(kind: HistoryKind) -> Self {
        Self { kind, tiers: vec![Ring::default(); kind.tiers().len()] }
    }

    pub fn kind(&self) -> HistoryKind {
        self.kind
    }

    /// Record `value` at sim time `now` into every tier.
    pub fn record(&mut self, now: SimTime, value: f64) {
        let v = value as f32;
        for (ring, spec) in self.tiers.iter_mut().zip(self.kind.tiers()) {
            let bucket = (now.max(0.0) / spec.width) as u32;
            match ring.newest_mut() {
                // Same bar (or a clock that did not advance): overwrite.
                Some(last) if last.bucket >= bucket => last.value = v,
                _ => ring.push(Bar { bucket, value: v }, spec.cap),
            }
        }
    }

    /// Points `(sim time, value)` for a chart range, oldest first. The time
    /// of a bar is the end of its bucket, capped at `now` for the open bar.
    pub fn points(&self, range: ChartRange, now: SimTime) -> Vec<(f64, f32)> {
        let tier = self.kind.tier_for(range);
        let width = self.kind.tiers()[tier].width;
        let first = ((now - range.window_secs()).max(0.0) / width) as u32;
        self.tiers[tier]
            .iter()
            .filter(|b| b.bucket >= first)
            .map(|b| (((b.bucket as f64 + 1.0) * width).min(now), b.value))
            .collect()
    }

    /// Like [`points`](Self::points), but never empty-handed while the
    /// market is shut: if the clock window holds fewer than a handful of
    /// bars (a Monday morning looking at "1D"), it returns the most recent
    /// bars instead, as many as the range normally spans in trading time.
    /// So "1D" on a weekend shows Friday's session rather than a flat line.
    pub fn session_points(&self, range: ChartRange, now: SimTime) -> Vec<(f64, f32)> {
        const ENOUGH: usize = 12;
        let pts = self.points(range, now);
        if pts.len() >= ENOUGH || range == ChartRange::TenYear {
            return pts;
        }
        let tier = self.kind.tier_for(range);
        let width = self.kind.tiers()[tier].width;
        // Trading time in the window: 6.5 h a day intraday, and 5 days in
        // 7 once the window is long enough to contain a weekend.
        let weekdays = if range.window_secs() >= WEEK { 5.0 / 7.0 } else { 1.0 };
        let trading = match range {
            ChartRange::Hour => SECS_PER_HOUR,
            _ if width < SECS_PER_DAY => range.window_secs() * (6.5 / 24.0) * weekdays,
            _ => range.window_secs() * weekdays,
        };
        let want = ((trading / width).ceil() as usize).max(ENOUGH);
        let ring = &self.tiers[tier];
        ring.iter()
            .skip(ring.len().saturating_sub(want))
            .map(|b| (((b.bucket as f64 + 1.0) * width).min(now), b.value))
            .collect()
    }

    /// Rescale every stored value (stock splits keep charts continuous).
    pub fn scale(&mut self, factor: f64) {
        for r in &mut self.tiers {
            r.scale(factor as f32);
        }
    }

    pub fn total_bars(&self) -> usize {
        self.tiers.iter().map(Ring::len).sum()
    }

    /// Raw tiers, oldest bar first (used by the history sidecar file).
    pub fn export(&self) -> Vec<Vec<Bar>> {
        self.tiers.iter().map(|r| r.iter().collect()).collect()
    }

    /// Rebuild from [`export`](Self::export) output. Extra tiers or bars
    /// beyond the layout's capacity are dropped (oldest first), so a file
    /// written by a build with different capacities still loads.
    pub fn import(kind: HistoryKind, tiers: Vec<Vec<Bar>>) -> Self {
        let mut h = History::new(kind);
        for ((ring, spec), bars) in h.tiers.iter_mut().zip(kind.tiers()).zip(tiers) {
            let skip = bars.len().saturating_sub(spec.cap);
            ring.bars = bars[skip..].to_vec();
            ring.head = 0;
        }
        h
    }
}

/// A tier on disk. (`Bar` itself deliberately has no `Persist` impl, so
/// this hand-written one is the only way a `Vec<Bar>` can be encoded.)
/// Bars are in time order, so each bucket is stored as its
/// distance from the previous one in a variable-length integer (LEB128).
/// Consecutive bars are usually one bucket apart, which makes that a single
/// byte instead of four and shrinks the history file by about a third.
impl crate::save::Persist for Vec<Bar> {
    fn put(&self, w: &mut crate::save::Writer) {
        (self.len() as u32).put(w);
        let mut prev = 0u32;
        for bar in self {
            let mut delta = bar.bucket.wrapping_sub(prev);
            prev = bar.bucket;
            while delta >= 0x80 {
                w.buf.push((delta as u8 & 0x7f) | 0x80);
                delta >>= 7;
            }
            w.buf.push(delta as u8);
            bar.value.put(w);
        }
    }
    fn get(r: &mut crate::save::Reader) -> crate::save::Result<Self> {
        let n = u32::get(r)? as usize;
        // Each bar takes at least 5 bytes; refuse impossible lengths.
        if n > r.remaining() / 5 {
            return Err(crate::save::SaveError::Corrupt("history length"));
        }
        let mut bars = Vec::with_capacity(n);
        let mut prev = 0u32;
        for _ in 0..n {
            let (mut delta, mut shift) = (0u32, 0u32);
            loop {
                let byte = u8::get(r)?;
                delta |= ((byte & 0x7f) as u32) << shift;
                if byte & 0x80 == 0 {
                    break;
                }
                shift += 7;
                if shift > 28 {
                    return Err(crate::save::SaveError::Corrupt("history bucket"));
                }
            }
            prev = prev.wrapping_add(delta);
            bars.push(Bar { bucket: prev, value: f32::get(r)? });
        }
        Ok(bars)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_bucket_overwrites_and_new_bucket_appends() {
        let mut h = History::new(HistoryKind::Price);
        h.record(10.0, 1.0);
        h.record(50.0, 2.0); // same minute
        h.record(70.0, 3.0); // next minute
        let pts = h.points(ChartRange::Hour, 70.0);
        assert_eq!(pts.len(), 2);
        assert_eq!(pts[0].1, 2.0);
        assert_eq!(pts[1], (70.0, 3.0));
    }

    #[test]
    fn ring_evicts_oldest_and_stays_ordered() {
        let mut h = History::new(HistoryKind::Price);
        for m in 0..200 {
            h.record(m as f64 * 60.0, m as f64);
        }
        let bars = &h.export()[0];
        assert_eq!(bars.len(), 64);
        assert_eq!(bars[0].value, 136.0);
        assert_eq!(bars[63].value, 199.0);
        assert!(bars.windows(2).all(|w| w[0].bucket < w[1].bucket));
    }

    #[test]
    fn every_range_is_covered_by_its_tier() {
        // A market series prints around the clock; a price only in session.
        // In both cases the ring must hold the whole window.
        for kind in [HistoryKind::Market, HistoryKind::Equity] {
            for range in ALL_RANGES {
                // Broker equity keeps three years; few brokers live longer.
                if kind == HistoryKind::Equity && range == ChartRange::TenYear {
                    continue;
                }
                let spec = kind.tiers()[kind.tier_for(range)];
                let needed = (range.window_secs() / spec.width).ceil() as usize;
                assert!(spec.cap >= needed, "{kind:?} {range:?}: cap {} < {needed}", spec.cap);
            }
        }
        for range in ALL_RANGES {
            let spec = HistoryKind::Price.tiers()[HistoryKind::Price.tier_for(range)];
            // Longest session is 8.5 h; 5 trading days out of 7.
            let open_fraction = if spec.width < SECS_PER_DAY { 8.5 / 24.0 * 5.0 / 7.0 } else { 5.0 / 7.0 };
            let open_fraction = if spec.width >= WEEK { 1.0 } else { open_fraction };
            let window = if range == ChartRange::Hour { SECS_PER_HOUR } else { range.window_secs() * open_fraction };
            let needed = (window / spec.width).ceil() as usize;
            assert!(spec.cap >= needed, "Price {range:?}: cap {} < {needed}", spec.cap);
        }
    }

    #[test]
    fn export_import_round_trip_and_scale() {
        let mut h = History::new(HistoryKind::Market);
        for m in 0..5_000 {
            h.record(m as f64 * 60.0, 100.0 + m as f64);
        }
        let mut back = History::import(HistoryKind::Market, h.export());
        assert_eq!(back.export(), h.export());
        back.scale(0.5);
        assert_eq!(back.export()[0][0].value, h.export()[0][0].value * 0.5);
        // Recording continues correctly after an import.
        back.record(5_000.0 * 60.0, 1.0);
        assert_eq!(back.export()[0].last().unwrap().value, 1.0);
    }

    #[test]
    fn closed_market_falls_back_to_the_last_session() {
        let mut h = History::new(HistoryKind::Price);
        // One session of 5-minute prints on a Friday, then a weekend.
        let friday = 4.0 * SECS_PER_DAY + 14.0 * SECS_PER_HOUR;
        for k in 0..100 {
            h.record(friday + k as f64 * 300.0, 100.0 + k as f64);
        }
        let monday = 7.0 * SECS_PER_DAY + 8.0 * SECS_PER_HOUR;
        assert!(h.points(ChartRange::Day, monday).is_empty());
        let pts = h.session_points(ChartRange::Day, monday);
        assert_eq!(pts.len(), 78, "one 6.5-hour session of 5-minute bars");
        assert_eq!(pts.last().unwrap().1, 199.0);
        // While the market is open the clock window wins.
        assert_eq!(h.session_points(ChartRange::Day, friday + 100.0 * 300.0).len(), 100);
    }

    #[test]
    fn bars_survive_the_compact_disk_encoding() {
        use crate::save::{Persist, Reader, Writer};
        let bars = vec![
            Bar { bucket: 5_000_000, value: 1.5 },
            Bar { bucket: 5_000_001, value: -2.0 },
            Bar { bucket: 5_000_200, value: 3.25 },
            Bar { bucket: u32::MAX, value: 0.0 },
        ];
        let mut w = Writer::default();
        bars.put(&mut w);
        assert_eq!(Vec::<Bar>::get(&mut Reader::new(&w.buf)).unwrap(), bars);
        // 4 (count) + 4 x 4 (values) + 4 + 1 + 2 + 5 (bucket deltas).
        assert_eq!(w.buf.len(), 32);
        assert!(Vec::<Bar>::get(&mut Reader::new(&w.buf[..w.buf.len() - 1])).is_err());
    }

    #[test]
    fn full_price_history_stays_small() {
        let caps: usize = HistoryKind::Price.tiers().iter().map(|t| t.cap).sum();
        assert_eq!(caps * std::mem::size_of::<Bar>(), 12_544); // bytes per security, worst case
    }
}
