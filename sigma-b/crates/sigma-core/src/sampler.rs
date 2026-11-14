//! Weighted random sampling over every security slot, in O(log n).
//!
//! This is the heart of path B. Each engine tick draws a fixed *budget* of
//! securities to update, with probability proportional to a weight
//! ("how interesting is this stock right now": volatility, whether the user
//! is watching it, whether an order is resting on it...).
//!
//! A Fenwick tree (binary indexed tree) stores prefix sums of the weights:
//! * changing one weight costs O(log n),
//! * drawing one weighted sample costs O(log n),
//! * the total weight is O(1) to read.
//!
//! The capacity is exactly 2^16 = 65,536, the hard cap on securities. A
//! power-of-two size lets the sampling descent walk the tree in exactly 16
//! halving steps with no bounds checks: a happy coincidence of the spec.
//!
//! Alternatives considered:
//! * Alias tables sample in O(1) but must be rebuilt in O(n) whenever a
//!   weight changes, and our weights change on every update.
//! * A flat array with a linear scan is O(n) per draw: 65,536 reads per
//!   sample, far too slow for thousands of samples per tick.

pub const CAPACITY: usize = 1 << 16;

#[derive(Clone)]
pub struct WeightedSampler {
    /// 1-based Fenwick array: `tree[i]` holds the sum of a power-of-two
    /// sized block of weights ending at slot `i - 1`.
    tree: Vec<f64>,
    /// The current weight of each slot, kept so `set` can compute a delta.
    weights: Vec<f64>,
}

impl Default for WeightedSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl WeightedSampler {
    pub fn new() -> Self {
        Self {
            tree: vec![0.0; CAPACITY + 1],
            weights: vec![0.0; CAPACITY],
        }
    }

    pub fn weight(&self, slot: usize) -> f64 {
        self.weights[slot]
    }

    /// Set a slot's weight. Negative or non-finite weights are treated as 0
    /// so a bad value can never corrupt the whole tree.
    pub fn set(&mut self, slot: usize, weight: f64) {
        let w = if weight.is_finite() && weight > 0.0 { weight } else { 0.0 };
        let delta = w - self.weights[slot];
        if delta == 0.0 {
            return;
        }
        self.weights[slot] = w;
        let mut i = slot + 1;
        while i <= CAPACITY {
            self.tree[i] += delta;
            i += i & i.wrapping_neg(); // add the lowest set bit
        }
    }

    /// Sum of all weights. With a power-of-two capacity the root node
    /// `tree[CAPACITY]` covers every slot.
    pub fn total(&self) -> f64 {
        self.tree[CAPACITY]
    }

    /// Draw a slot with probability weight/total, given `u` uniform in
    /// [0, 1). Returns `None` when every weight is zero.
    pub fn sample(&self, u: f64) -> Option<usize> {
        let total = self.total();
        if total <= 0.0 {
            return None;
        }
        let mut target = u * total;
        let mut pos = 0usize;
        let mut step = CAPACITY;
        // Walk down from the root: at each level decide whether the target
        // lies in the left block (stay) or past it (move right).
        while step > 0 {
            let next = pos + step;
            if next <= CAPACITY && self.tree[next] <= target {
                target -= self.tree[next];
                pos = next;
            }
            step >>= 1;
        }
        // `pos` is now the count of slots strictly before the chosen one.
        // Floating point drift can push it past the last live slot, or onto
        // a zero-weight slot; step back to the nearest weighted slot.
        let mut slot = pos.min(CAPACITY - 1);
        while self.weights[slot] <= 0.0 {
            if slot == 0 {
                return None;
            }
            slot -= 1;
        }
        Some(slot)
    }

    /// Rebuild the prefix sums from the stored weights in O(n). Thousands of
    /// `+= delta` updates accumulate floating-point error; the engine calls
    /// this once per simulated day to wipe it out.
    pub fn rebuild(&mut self) {
        self.tree.iter_mut().for_each(|t| *t = 0.0);
        for i in 1..=CAPACITY {
            self.tree[i] += self.weights[i - 1];
            let parent = i + (i & i.wrapping_neg());
            if parent <= CAPACITY {
                self.tree[parent] += self.tree[i];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;

    #[test]
    fn total_tracks_sets() {
        let mut s = WeightedSampler::new();
        s.set(0, 1.0);
        s.set(65_535, 2.0);
        s.set(100, 3.0);
        assert_eq!(s.total(), 6.0);
        s.set(100, 0.0);
        assert_eq!(s.total(), 3.0);
    }

    #[test]
    fn empty_sampler_returns_none() {
        let s = WeightedSampler::new();
        assert_eq!(s.sample(0.5), None);
    }

    #[test]
    fn samples_follow_weights() {
        let mut s = WeightedSampler::new();
        s.set(10, 1.0);
        s.set(20, 3.0);
        s.set(40_000, 6.0);
        let mut rng = Rng::seeded(5);
        let mut counts = [0u32; 3];
        let n = 100_000;
        for _ in 0..n {
            match s.sample(rng.uniform()).unwrap() {
                10 => counts[0] += 1,
                20 => counts[1] += 1,
                40_000 => counts[2] += 1,
                other => panic!("sampled zero-weight slot {other}"),
            }
        }
        let frac = |c: u32| c as f64 / n as f64;
        assert!((frac(counts[0]) - 0.1).abs() < 0.01);
        assert!((frac(counts[1]) - 0.3).abs() < 0.01);
        assert!((frac(counts[2]) - 0.6).abs() < 0.01);
    }

    #[test]
    fn rebuild_matches_incremental() {
        let mut s = WeightedSampler::new();
        let mut rng = Rng::seeded(11);
        for _ in 0..5_000 {
            let slot = rng.below(CAPACITY as u64) as usize;
            s.set(slot, rng.uniform() * 10.0);
        }
        let before = s.total();
        let mut r = s.clone();
        r.rebuild();
        assert!((before - r.total()).abs() < 1e-6 * before);
        for _ in 0..1000 {
            let u = rng.uniform();
            assert_eq!(s.sample(u).is_some(), r.sample(u).is_some());
        }
    }
}
