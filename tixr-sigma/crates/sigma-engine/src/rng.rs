//! Random number generation, hand-rolled so the engine needs no `rand` crate.
//!
//! * [`SplitMix64`] expands one `u64` seed into well-mixed state words.
//! * [`Rng`] is xoshiro256++ (Blackman & Vigna, 2019): 256-bit state, fast,
//!   passes BigCrush, and is the generator `rand` itself uses for `SmallRng`
//!   on 64-bit targets. Not cryptographic, which a simulator does not need.
//!
//! Everything is seeded explicitly. The alpha version mixed the wall clock
//! into its seed, which made runs impossible to reproduce; here the same seed
//! always produces the same market.

/// SplitMix64: a tiny generator used only to seed [`Rng`].
#[derive(Clone, Debug)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// xoshiro256++ with the distribution helpers the simulation needs.
#[derive(Clone, Debug)]
pub struct Rng {
    s: [u64; 4],
    /// Box-Muller makes normals in pairs; keep the spare one.
    spare_normal: Option<f64>,
}

impl Rng {
    pub fn seed_from_u64(seed: u64) -> Self {
        let mut sm = SplitMix64::new(seed);
        let s = [sm.next_u64(), sm.next_u64(), sm.next_u64(), sm.next_u64()];
        Self { s, spare_normal: None }
    }

    /// Derive an independent child stream (used so that e.g. ticker-name
    /// generation does not shift the price stream when names collide).
    pub fn fork(&mut self, salt: u64) -> Self {
        Self::seed_from_u64(self.next_u64() ^ salt.wrapping_mul(0xD1B5_4A32_D192_ED03))
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let result = (self.s[0].wrapping_add(self.s[3])).rotate_left(23).wrapping_add(self.s[0]);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Uniform in `[0, 1)` with 53 bits of precision.
    #[inline]
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Uniform in `(0, 1]`, safe to take `ln` of.
    #[inline]
    pub fn uniform_open0(&mut self) -> f64 {
        1.0 - self.uniform()
    }

    #[inline]
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.uniform()
    }

    /// Uniform integer in `0..n` (Lemire's multiply-shift; bias is < 2^-32
    /// for the n we use, which is far below anything observable here).
    #[inline]
    pub fn below(&mut self, n: usize) -> usize {
        debug_assert!(n > 0);
        ((self.next_u64() >> 32).wrapping_mul(n as u64) >> 32) as usize
    }

    #[inline]
    pub fn chance(&mut self, p: f64) -> bool {
        self.uniform() < p
    }

    /// Standard normal via Box-Muller.
    pub fn normal(&mut self) -> f64 {
        if let Some(z) = self.spare_normal.take() {
            return z;
        }
        let u1 = self.uniform_open0();
        let u2 = self.uniform();
        let r = (-2.0 * u1.ln()).sqrt();
        let theta = std::f64::consts::TAU * u2;
        self.spare_normal = Some(r * theta.sin());
        r * theta.cos()
    }

    /// Exponential with the given rate (mean `1 / rate`). This is the
    /// inter-arrival time of a Poisson process, the heart of the scheduler.
    #[inline]
    pub fn exponential(&mut self, rate: f64) -> f64 {
        debug_assert!(rate > 0.0);
        -self.uniform_open0().ln() / rate
    }

    /// Poisson-distributed count with mean `lambda`.
    /// Knuth's product method for small means, normal approximation above 30.
    pub fn poisson(&mut self, lambda: f64) -> u32 {
        if lambda <= 0.0 {
            return 0;
        }
        if lambda > 30.0 {
            let x = lambda + lambda.sqrt() * self.normal();
            return x.round().max(0.0) as u32;
        }
        let limit = (-lambda).exp();
        let mut k = 0u32;
        let mut p = self.uniform_open0();
        while p > limit {
            k += 1;
            p *= self.uniform_open0();
        }
        k
    }

    /// Pick an index with probability proportional to `weights[i]`.
    pub fn weighted_index(&mut self, weights: &[u32]) -> usize {
        let total: u32 = weights.iter().sum();
        let mut target = self.below(total as usize) as u32;
        for (i, &w) in weights.iter().enumerate() {
            if target < w {
                return i;
            }
            target -= w;
        }
        weights.len() - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_stream() {
        let mut a = Rng::seed_from_u64(42);
        let mut b = Rng::seed_from_u64(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn uniform_is_in_range_and_centred() {
        let mut r = Rng::seed_from_u64(1);
        let n = 200_000;
        let mut sum = 0.0;
        for _ in 0..n {
            let u = r.uniform();
            assert!((0.0..1.0).contains(&u));
            sum += u;
        }
        let mean = sum / n as f64;
        assert!((mean - 0.5).abs() < 0.005, "mean {mean}");
    }

    #[test]
    fn normal_has_unit_variance() {
        let mut r = Rng::seed_from_u64(7);
        let n = 200_000;
        let (mut s, mut s2) = (0.0, 0.0);
        for _ in 0..n {
            let z = r.normal();
            s += z;
            s2 += z * z;
        }
        let mean = s / n as f64;
        let var = s2 / n as f64 - mean * mean;
        assert!(mean.abs() < 0.01, "mean {mean}");
        assert!((var - 1.0).abs() < 0.02, "var {var}");
    }

    #[test]
    fn exponential_mean_matches_rate() {
        let mut r = Rng::seed_from_u64(9);
        let n = 100_000;
        let mean: f64 = (0..n).map(|_| r.exponential(4.0)).sum::<f64>() / n as f64;
        assert!((mean - 0.25).abs() < 0.005, "mean {mean}");
    }

    #[test]
    fn poisson_mean_matches_lambda() {
        let mut r = Rng::seed_from_u64(11);
        for &lambda in &[0.1, 2.5, 12.0, 80.0] {
            let n = 50_000;
            let mean = (0..n).map(|_| r.poisson(lambda) as f64).sum::<f64>() / n as f64;
            assert!((mean - lambda).abs() < lambda.max(1.0) * 0.03, "lambda {lambda} mean {mean}");
        }
    }

    #[test]
    fn below_never_reaches_n() {
        let mut r = Rng::seed_from_u64(3);
        for n in 1..50 {
            for _ in 0..200 {
                assert!(r.below(n) < n);
            }
        }
    }
}
