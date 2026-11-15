//! Calendar queue (a bucketed timing wheel) for discrete-event scheduling.
//!
//! # Why not just `BinaryHeap`?
//! A binary heap costs O(log n) per push and pop and jumps around memory. With
//! tens of thousands of securities each re-scheduling itself every few sim
//! seconds, the queue is the hottest data structure in the engine. A calendar
//! queue (Brown, 1988) splits the near future into fixed-width time buckets:
//!
//! ```text
//!   cursor ->[ b0 ][ b1 ][ b2 ] ... [ bN-1 ]   overflow heap (far future)
//!             ^ events with time in [k*w, (k+1)*w)
//! ```
//!
//! Push is "compute bucket index, insert into a short sorted Vec": amortised
//! O(1). Pop takes from the current bucket; when it empties the cursor steps
//! forward. Events beyond the ring's horizon wait in a small heap and migrate
//! into the ring as the cursor approaches them. Hourly macro events and
//! weekend re-opens live there; per-second stock ticks never touch it.
//!
//! Ties are broken by insertion sequence, so pop order is a pure function of
//! the push history. That is what makes whole runs reproducible from a seed.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

#[derive(Debug)]
struct Entry<T> {
    time: f64,
    seq: u64,
    item: T,
}

impl<T> Entry<T> {
    #[inline]
    fn key_cmp(&self, other: &Self) -> Ordering {
        self.time.total_cmp(&other.time).then(self.seq.cmp(&other.seq))
    }
}

// BinaryHeap is a max-heap; reverse the order so the earliest event is on top.
impl<T> PartialEq for Entry<T> {
    fn eq(&self, other: &Self) -> bool {
        self.key_cmp(other) == Ordering::Equal
    }
}
impl<T> Eq for Entry<T> {}
impl<T> PartialOrd for Entry<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<T> Ord for Entry<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        other.key_cmp(self)
    }
}

pub struct CalendarQueue<T> {
    /// Each bucket is sorted *descending* by (time, seq) so the next event to
    /// pop is at the end, making `Vec::pop` the O(1) hot path.
    buckets: Vec<Vec<Entry<T>>>,
    mask: u64,
    width: f64,
    /// Absolute index of the bucket under the cursor. Invariant: the ring
    /// only holds events whose absolute bucket is in `cursor..cursor + N`.
    cursor: u64,
    overflow: BinaryHeap<Entry<T>>,
    in_ring: usize,
    next_seq: u64,
}

impl<T> CalendarQueue<T> {
    /// `bucket_count` is rounded up to a power of two; `bucket_width` is in
    /// the same unit as event times (sim seconds here).
    pub fn new(bucket_count: usize, bucket_width: f64) -> Self {
        assert!(bucket_width > 0.0 && bucket_width.is_finite());
        let n = bucket_count.max(2).next_power_of_two();
        Self {
            buckets: (0..n).map(|_| Vec::new()).collect(),
            mask: n as u64 - 1,
            width: bucket_width,
            cursor: 0,
            overflow: BinaryHeap::new(),
            in_ring: 0,
            next_seq: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.in_ring + self.overflow.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    fn bucket_of(&self, time: f64) -> u64 {
        (time / self.width).floor().max(0.0) as u64
    }

    #[inline]
    fn horizon(&self) -> u64 {
        self.cursor + self.mask + 1
    }

    /// Schedule `item` at `time`. Times in the past are allowed and simply
    /// come out first (they land in the current bucket).
    pub fn push(&mut self, time: f64, item: T) {
        debug_assert!(time.is_finite(), "non-finite event time");
        let seq = self.next_seq;
        self.next_seq += 1;
        let entry = Entry { time, seq, item };
        let b = self.bucket_of(time).max(self.cursor);
        if b < self.horizon() {
            self.insert_ring(b, entry);
        } else {
            self.overflow.push(entry);
        }
    }

    #[inline]
    fn insert_ring(&mut self, abs_bucket: u64, entry: Entry<T>) {
        let bucket = &mut self.buckets[(abs_bucket & self.mask) as usize];
        // Descending order: find the first element that is <= the new one.
        let pos = bucket.partition_point(|e| e.key_cmp(&entry) == Ordering::Greater);
        bucket.insert(pos, entry);
        self.in_ring += 1;
    }

    fn migrate_overflow(&mut self) {
        let horizon = self.horizon();
        while let Some(top) = self.overflow.peek() {
            let b = self.bucket_of(top.time).max(self.cursor);
            if b >= horizon {
                break;
            }
            let entry = self.overflow.pop().expect("peeked");
            self.insert_ring(b, entry);
        }
    }

    /// Pop the earliest event whose time is `<= limit`, or `None` if the
    /// earliest pending event lies after `limit`.
    pub fn pop_due(&mut self, limit: f64) -> Option<(f64, T)> {
        loop {
            let slot = (self.cursor & self.mask) as usize;
            if let Some(last) = self.buckets[slot].last() {
                // The current bucket is the earliest non-empty one, and the
                // overflow only holds later events, so `last` is the global min.
                if last.time <= limit {
                    let e = self.buckets[slot].pop().expect("non-empty");
                    self.in_ring -= 1;
                    return Some((e.time, e.item));
                }
                return None;
            }
            // Current bucket empty: advance, unless that would pass `limit`.
            let next_start = (self.cursor + 1) as f64 * self.width;
            if next_start > limit {
                return None;
            }
            if self.in_ring == 0 {
                // Nothing in the ring: jump straight to the next interesting
                // bucket instead of stepping through empty ones (weekends).
                let limit_bucket = self.bucket_of(limit);
                let target = match self.overflow.peek() {
                    Some(e) => self.bucket_of(e.time).min(limit_bucket),
                    None => limit_bucket,
                };
                self.cursor = target.max(self.cursor + 1);
            } else {
                self.cursor += 1;
            }
            self.migrate_overflow();
        }
    }

    /// Every pending event in pop order, without disturbing the queue.
    /// Pushing the result into a fresh queue reproduces the same pop order
    /// (ties keep their relative sequence), which is how a saved game
    /// restores its schedule.
    pub fn pending(&self) -> Vec<(f64, T)>
    where
        T: Clone,
    {
        let mut all: Vec<&Entry<T>> = self.buckets.iter().flatten().chain(self.overflow.iter()).collect();
        all.sort_by(|a, b| a.key_cmp(b));
        all.into_iter().map(|e| (e.time, e.item.clone())).collect()
    }

    /// Time of the earliest pending event (O(N) worst case; diagnostics only).
    pub fn peek_time(&self) -> Option<f64> {
        let ring_min = self.buckets.iter().filter_map(|b| b.last().map(|e| e.time)).min_by(|a, b| a.total_cmp(b));
        let ovf_min = self.overflow.peek().map(|e| e.time);
        match (ring_min, ovf_min) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;
    use std::cmp::Reverse;

    /// Reference model: a plain heap keyed on (time bits, seq).
    struct Reference {
        heap: BinaryHeap<Reverse<(u64, u64, u32)>>,
        seq: u64,
    }
    impl Reference {
        fn push(&mut self, t: f64, item: u32) {
            // Non-negative f64s order the same as their bit patterns.
            self.heap.push(Reverse((t.to_bits(), self.seq, item)));
            self.seq += 1;
        }
        fn pop_due(&mut self, limit: f64) -> Option<(f64, u32)> {
            match self.heap.peek() {
                Some(Reverse((tb, _, _))) if f64::from_bits(*tb) <= limit => {
                    let Reverse((tb, _, it)) = self.heap.pop().unwrap();
                    Some((f64::from_bits(tb), it))
                }
                _ => None,
            }
        }
    }

    #[test]
    fn matches_binary_heap_under_random_load() {
        let mut rng = Rng::seed_from_u64(2024);
        let mut q = CalendarQueue::new(64, 0.5);
        let mut r = Reference { heap: BinaryHeap::new(), seq: 0 };
        let mut now = 0.0f64;
        let mut next_id = 0u32;
        for _round in 0..2_000 {
            // Push a burst: mostly near-future, some far-future (overflow).
            for _ in 0..rng.below(20) {
                let dt = if rng.chance(0.1) { rng.range(0.0, 5_000.0) } else { rng.exponential(0.8) };
                q.push(now + dt, next_id);
                r.push(now + dt, next_id);
                next_id += 1;
            }
            now += rng.exponential(0.5);
            loop {
                let a = q.pop_due(now);
                let b = r.pop_due(now);
                assert_eq!(a, b, "diverged at now={now}");
                match a {
                    // Events re-schedule themselves, like stock ticks do.
                    Some((t, id)) if rng.chance(0.5) => {
                        let t2 = t + rng.exponential(1.0);
                        q.push(t2, id);
                        r.push(t2, id);
                    }
                    Some(_) => {}
                    None => break,
                }
            }
        }
        assert_eq!(q.len(), r.heap.len());
    }

    #[test]
    fn equal_times_pop_in_insertion_order() {
        let mut q = CalendarQueue::new(8, 1.0);
        for i in 0..10 {
            q.push(3.0, i);
        }
        let order: Vec<_> = std::iter::from_fn(|| q.pop_due(10.0).map(|(_, i)| i)).collect();
        assert_eq!(order, (0..10).collect::<Vec<_>>());
    }

    #[test]
    fn jumps_over_long_empty_stretches() {
        let mut q = CalendarQueue::new(16, 1.0);
        q.push(1_000_000.0, "far");
        assert_eq!(q.pop_due(999_999.0), None);
        assert_eq!(q.pop_due(1_000_000.0), Some((1_000_000.0, "far")));
        assert!(q.is_empty());
    }

    #[test]
    fn past_events_come_out_first() {
        let mut q = CalendarQueue::new(16, 1.0);
        q.push(50.0, 1);
        assert_eq!(q.pop_due(40.0), None); // cursor advances to ~40
        q.push(10.0, 2); // "late" event
        assert_eq!(q.pop_due(60.0), Some((10.0, 2)));
        assert_eq!(q.pop_due(60.0), Some((50.0, 1)));
    }
}
