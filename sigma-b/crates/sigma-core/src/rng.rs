//! Deterministic random numbers, with no external crates.
//!
//! The original tixr pulled in `rand` + `rand_chacha` and then mixed the
//! system clock into the seed, so two runs with the same seed diverged.
//! Here a seed fully determines a run: `SplitMix64` expands the user seed
//! into the 256-bit state of `Xoshiro256++`, the generator used for all
//! simulation draws.
//!
//! Xoshiro256++ is not cryptographic, which is fine: we need speed and good
//! statistical quality, not secrecy. It is the same family `rand` uses for
//! its small fast RNGs.

/// SplitMix64: a tiny generator whose main job is seeding other generators.
/// Every output of a 64-bit counter is passed through a strong mixer, so even
/// seeds like 0, 1, 2 produce unrelated streams.
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

/// The simulation RNG. `Clone` is deliberate: tests clone a generator to
/// replay the exact same stream.
#[derive(Clone, Debug)]
pub struct Rng {
    s: [u64; 4],
    /// The polar method makes normals in pairs; we keep the spare one.
    spare_normal: Option<f64>,
}

impl Rng {
    pub fn seeded(seed: u64) -> Self {
        let mut sm = SplitMix64::new(seed);
        let s = [sm.next_u64(), sm.next_u64(), sm.next_u64(), sm.next_u64()];
        Self { s, spare_normal: None }
    }

    /// Derive an independent child stream. Used so that, for example, IPO
    /// parameter draws do not shift when the number of price updates per
    /// tick changes.
    pub fn fork(&mut self, salt: u64) -> Self {
        Self::seeded(self.next_u64() ^ salt.wrapping_mul(0xA24B_AED4_963E_E407))
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let result = (self.s[0].wrapping_add(self.s[3]))
            .rotate_left(23)
            .wrapping_add(self.s[0]);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Uniform in [0, 1) with 53 bits of precision.
    #[inline]
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Uniform in [lo, hi).
    #[inline]
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.uniform()
    }

    /// Uniform integer in [0, n). Uses Lemire's multiply-shift, which avoids
    /// the bias of `x % n` without a division.
    #[inline]
    pub fn below(&mut self, n: u64) -> u64 {
        debug_assert!(n > 0);
        ((self.next_u64() as u128 * n as u128) >> 64) as u64
    }

    #[inline]
    pub fn chance(&mut self, p: f64) -> bool {
        self.uniform() < p
    }

    /// Standard normal via Marsaglia's polar method.
    pub fn normal(&mut self) -> f64 {
        if let Some(z) = self.spare_normal.take() {
            return z;
        }
        loop {
            let u = 2.0 * self.uniform() - 1.0;
            let v = 2.0 * self.uniform() - 1.0;
            let s = u * u + v * v;
            if s > 0.0 && s < 1.0 {
                let m = (-2.0 * s.ln() / s).sqrt();
                self.spare_normal = Some(v * m);
                return u * m;
            }
        }
    }

    /// Normal clipped to [-limit, limit]. Clipping (rather than resampling)
    /// keeps the number of RNG draws fixed, which keeps runs reproducible.
    pub fn normal_clipped(&mut self, limit: f64) -> f64 {
        self.normal().clamp(-limit, limit)
    }

    /// Poisson count. Knuth's product method for small means, a rounded
    /// normal approximation for large ones (accurate above ~30).
    pub fn poisson(&mut self, mean: f64) -> u32 {
        if mean <= 0.0 {
            return 0;
        }
        if mean < 30.0 {
            let limit = (-mean).exp();
            let mut k = 0u32;
            let mut p = self.uniform();
            while p > limit {
                k += 1;
                p *= self.uniform();
            }
            k
        } else {
            (mean + mean.sqrt() * self.normal()).round().max(0.0) as u32
        }
    }

    /// Log-normal draw with the given median and log-space spread.
    pub fn lognormal(&mut self, median: f64, sigma: f64) -> f64 {
        median * (sigma * self.normal()).exp()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_stream() {
        let mut a = Rng::seeded(42);
        let mut b = Rng::seeded(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = Rng::seeded(1);
        let mut b = Rng::seeded(2);
        let same = (0..64).filter(|_| a.next_u64() == b.next_u64()).count();
        assert_eq!(same, 0);
    }

    #[test]
    fn normal_has_unit_moments() {
        let mut r = Rng::seeded(7);
        let n = 200_000;
        let (mut sum, mut sq) = (0.0, 0.0);
        for _ in 0..n {
            let z = r.normal();
            sum += z;
            sq += z * z;
        }
        let mean = sum / n as f64;
        let var = sq / n as f64 - mean * mean;
        assert!(mean.abs() < 0.01, "mean {mean}");
        assert!((var - 1.0).abs() < 0.02, "var {var}");
    }

    #[test]
    fn poisson_mean_matches() {
        let mut r = Rng::seeded(9);
        for &lambda in &[0.3, 4.0, 80.0] {
            let n = 50_000;
            let total: u64 = (0..n).map(|_| r.poisson(lambda) as u64).sum();
            let mean = total as f64 / n as f64;
            assert!((mean - lambda).abs() < 0.05 * lambda.max(1.0), "{lambda} -> {mean}");
        }
    }

    #[test]
    fn below_stays_in_range() {
        let mut r = Rng::seeded(3);
        for _ in 0..10_000 {
            assert!(r.below(17) < 17);
        }
    }
}
