//! Stock personas: stacking perks lettered A through P.
//!
//! A persona is a set of up to 12 of the 16 perks, stored as a `u16` bitmask
//! (bit 0 = A ... bit 15 = P). Each perk nudges the security's parameters;
//! stacked perks compose (multipliers multiply, offsets add) and the result is
//! clamped into a safe range, so no combination can destabilise the model.
//!
//! Persona length distribution (0..=12 perks), chosen to match the brief:
//! median 3, mean about 4.1, and 11 or 12 perks in about 3 % of securities.
//! See `LENGTH_WEIGHTS` and the test at the bottom that pins those numbers.

use crate::rng::Rng;

pub const PERK_COUNT: usize = 16;
pub const MAX_PERKS: usize = 12;

/// Relative weight for a persona of length 0, 1, ..., 12 (sums to 100).
pub const LENGTH_WEIGHTS: [u32; MAX_PERKS + 1] = [6, 12, 17, 16, 12, 9, 7, 6, 5, 4, 3, 2, 1];

/// Pairs that contradict each other and are never drawn together.
const EXCLUSIVE_PAIRS: [(Perk, Perk); 2] = [(Perk::Cyclical, Perk::Defensive), (Perk::Liquid, Perk::Illiquid)];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Perk {
    Anchor = 0,
    BlueChip,
    Cyclical,
    Defensive,
    Expansion,
    Frothy,
    Gambler,
    HardAssets,
    Illiquid,
    Junk,
    Kingpin,
    Liquid,
    Momentum,
    Novel,
    Oligopoly,
    Phoenix,
}

pub const ALL_PERKS: [Perk; PERK_COUNT] = [
    Perk::Anchor,
    Perk::BlueChip,
    Perk::Cyclical,
    Perk::Defensive,
    Perk::Expansion,
    Perk::Frothy,
    Perk::Gambler,
    Perk::HardAssets,
    Perk::Illiquid,
    Perk::Junk,
    Perk::Kingpin,
    Perk::Liquid,
    Perk::Momentum,
    Perk::Novel,
    Perk::Oligopoly,
    Perk::Phoenix,
];

impl Perk {
    pub fn letter(self) -> char {
        (b'A' + self as u8) as char
    }

    pub fn from_letter(c: char) -> Option<Perk> {
        let i = (c.to_ascii_uppercase() as u8).checked_sub(b'A')? as usize;
        ALL_PERKS.get(i).copied()
    }

    pub fn name(self) -> &'static str {
        match self {
            Perk::Anchor => "Anchor",
            Perk::BlueChip => "Blue-chip",
            Perk::Cyclical => "Cyclical",
            Perk::Defensive => "Defensive",
            Perk::Expansion => "Expansion",
            Perk::Frothy => "Frothy",
            Perk::Gambler => "Gambler",
            Perk::HardAssets => "Hard assets",
            Perk::Illiquid => "Illiquid",
            Perk::Junk => "Junk",
            Perk::Kingpin => "Kingpin",
            Perk::Liquid => "Liquid",
            Perk::Momentum => "Momentum",
            Perk::Novel => "Novel",
            Perk::Oligopoly => "Oligopoly",
            Perk::Phoenix => "Phoenix",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Perk::Anchor => "Snaps back to fair value fast (strong mean reversion).",
            Perk::BlueChip => "Large, steady, hard to kill; trades often with low impact.",
            Perk::Cyclical => "Amplifies the market and widens with credit spreads.",
            Perk::Defensive => "Damps the market; fewer shocks.",
            Perk::Expansion => "Growth story: higher drift, very rate sensitive.",
            Perk::Frothy => "Sentiment runs hot; slow to return to fair value.",
            Perk::Gambler => "Meme name: frequent jumps, heavy retail attention.",
            Perk::HardAssets => "Gains when inflation rises; less rate sensitive.",
            Perk::Illiquid => "Prints rarely; every order moves it a lot.",
            Perk::Junk => "Leveraged: closer to default, credit sensitive.",
            Perk::Kingpin => "Mature payer: low growth, safe, bond-like.",
            Perk::Liquid => "Prints constantly; orders barely move it.",
            Perk::Momentum => "Trends persist; momentum brokers love it.",
            Perk::Novel => "Innovator: volatile fundamentals, upside jumps.",
            Perk::Oligopoly => "Moat: stable fundamentals, slight edge.",
            Perk::Phoenix => "Survives deep drawdowns; never penny-delisted.",
        }
    }

    pub fn effect(self) -> PerkEffect {
        let e = PerkEffect::NEUTRAL;
        match self {
            Perk::Anchor => PerkEffect { kappa_mul: 1.8, sigma_u_mul: 0.8, ..e },
            Perk::BlueChip => PerkEffect { sigma_f_mul: 0.7, barrier_add: 0.8, rate_mul: 1.3, impact_mul: 0.6, ..e },
            Perk::Cyclical => PerkEffect { beta_add: 0.35, credit_add: 1.5, ..e },
            Perk::Defensive => PerkEffect { beta_add: -0.30, sigma_f_mul: 0.85, jump_rate_mul: 0.7, ..e },
            Perk::Expansion => PerkEffect { alpha_add: 0.04, sigma_f_mul: 1.2, duration_add: 4.0, ..e },
            Perk::Frothy => PerkEffect { sigma_u_mul: 1.5, kappa_mul: 0.6, ..e },
            Perk::Gambler => {
                PerkEffect { jump_rate_mul: 2.5, jump_mean_add: 0.03, rate_mul: 1.6, sigma_u_mul: 1.3, ..e }
            }
            Perk::HardAssets => PerkEffect { inflation_add: 3.0, beta_add: -0.1, duration_add: -1.0, ..e },
            Perk::Illiquid => PerkEffect { rate_mul: 0.35, impact_mul: 2.5, ..e },
            Perk::Junk => PerkEffect { barrier_add: -0.6, credit_add: 3.0, beta_add: 0.2, sigma_f_mul: 1.15, ..e },
            Perk::Kingpin => {
                PerkEffect { alpha_add: -0.01, barrier_add: 0.4, sigma_f_mul: 0.8, duration_add: 1.5, ..e }
            }
            Perk::Liquid => PerkEffect { rate_mul: 1.8, impact_mul: 0.5, ..e },
            Perk::Momentum => PerkEffect { kappa_mul: 0.7, ..e },
            Perk::Novel => PerkEffect {
                sigma_f_mul: 1.4,
                jump_rate_mul: 1.5,
                jump_mean_add: 0.04,
                alpha_add: 0.02,
                barrier_add: -0.2,
                ..e
            },
            Perk::Oligopoly => PerkEffect { sigma_f_mul: 0.75, alpha_add: 0.01, barrier_add: 0.5, kappa_mul: 1.2, ..e },
            Perk::Phoenix => PerkEffect { barrier_add: -0.5, jump_mean_add: 0.02, ..e },
        }
    }
}

/// How one perk changes a security's parameters. Multipliers compose by
/// multiplication, offsets by addition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PerkEffect {
    pub beta_add: f64,
    pub alpha_add: f64,
    pub sigma_f_mul: f64,
    pub kappa_mul: f64,
    pub sigma_u_mul: f64,
    pub jump_rate_mul: f64,
    pub jump_mean_add: f64,
    pub rate_mul: f64,
    pub impact_mul: f64,
    pub barrier_add: f64,
    pub duration_add: f64,
    pub inflation_add: f64,
    pub credit_add: f64,
}

impl PerkEffect {
    pub const NEUTRAL: PerkEffect = PerkEffect {
        beta_add: 0.0,
        alpha_add: 0.0,
        sigma_f_mul: 1.0,
        kappa_mul: 1.0,
        sigma_u_mul: 1.0,
        jump_rate_mul: 1.0,
        jump_mean_add: 0.0,
        rate_mul: 1.0,
        impact_mul: 1.0,
        barrier_add: 0.0,
        duration_add: 0.0,
        inflation_add: 0.0,
        credit_add: 0.0,
    };

    fn stack(self, o: PerkEffect) -> PerkEffect {
        PerkEffect {
            beta_add: self.beta_add + o.beta_add,
            alpha_add: self.alpha_add + o.alpha_add,
            sigma_f_mul: self.sigma_f_mul * o.sigma_f_mul,
            kappa_mul: self.kappa_mul * o.kappa_mul,
            sigma_u_mul: self.sigma_u_mul * o.sigma_u_mul,
            jump_rate_mul: self.jump_rate_mul * o.jump_rate_mul,
            jump_mean_add: self.jump_mean_add + o.jump_mean_add,
            rate_mul: self.rate_mul * o.rate_mul,
            impact_mul: self.impact_mul * o.impact_mul,
            barrier_add: self.barrier_add + o.barrier_add,
            duration_add: self.duration_add + o.duration_add,
            inflation_add: self.inflation_add + o.inflation_add,
            credit_add: self.credit_add + o.credit_add,
        }
    }
}

/// A set of perks. `Copy` and 2 bytes, so it is cheap to put in snapshots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Persona(pub u16);

impl Persona {
    pub const EMPTY: Persona = Persona(0);

    pub fn has(self, p: Perk) -> bool {
        self.0 & (1 << p as u16) != 0
    }

    pub fn with(self, p: Perk) -> Persona {
        Persona(self.0 | (1 << p as u16))
    }

    pub fn without(self, p: Perk) -> Persona {
        Persona(self.0 & !(1 << p as u16))
    }

    pub fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn perks(self) -> impl Iterator<Item = Perk> {
        ALL_PERKS.into_iter().filter(move |&p| self.has(p))
    }

    /// Letters in alphabetical order, e.g. `"BGL"`; `"-"` for no perks.
    pub fn letters(self) -> String {
        if self.is_empty() {
            return "-".to_string();
        }
        self.perks().map(Perk::letter).collect()
    }

    /// Parse `"bgl"` / `"B G L"`; unknown characters are ignored, and the
    /// result is trimmed to `MAX_PERKS`.
    pub fn parse(s: &str) -> Persona {
        let mut p = Persona::EMPTY;
        for c in s.chars() {
            if let Some(perk) = Perk::from_letter(c)
                && p.len() < MAX_PERKS
            {
                p = p.with(perk);
            }
        }
        p
    }

    /// The combined effect of all perks.
    pub fn effect(self) -> PerkEffect {
        self.perks().fold(PerkEffect::NEUTRAL, |acc, p| acc.stack(p.effect()))
    }

    /// Draw a random persona using [`LENGTH_WEIGHTS`].
    pub fn random(rng: &mut Rng) -> Persona {
        let target = rng.weighted_index(&LENGTH_WEIGHTS);
        // Partial Fisher-Yates over the 16 perks.
        let mut pool = ALL_PERKS;
        let mut persona = Persona::EMPTY;
        let mut i = 0;
        while persona.len() < target && i < PERK_COUNT {
            let j = i + rng.below(PERK_COUNT - i);
            pool.swap(i, j);
            let candidate = pool[i];
            i += 1;
            let clashes = EXCLUSIVE_PAIRS
                .iter()
                .any(|&(a, b)| (candidate == a && persona.has(b)) || (candidate == b && persona.has(a)));
            if !clashes {
                persona = persona.with(candidate);
            }
        }
        persona
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_round_trip() {
        let p = Persona::parse("lgb");
        assert_eq!(p.letters(), "BGL");
        assert_eq!(Persona::parse(&p.letters()), p);
        assert_eq!(Persona::EMPTY.letters(), "-");
        assert_eq!(Perk::from_letter('p'), Some(Perk::Phoenix));
        assert_eq!(Perk::from_letter('Q'), None);
    }

    #[test]
    fn length_distribution_matches_brief() {
        let mut rng = Rng::seed_from_u64(5);
        let n = 100_000;
        let mut lens: Vec<usize> = (0..n).map(|_| Persona::random(&mut rng).len()).collect();
        lens.sort_unstable();
        let median = lens[n / 2];
        let mean = lens.iter().sum::<usize>() as f64 / n as f64;
        let rare = lens.iter().filter(|&&l| l >= 11).count() as f64 / n as f64;
        assert!(lens.iter().all(|&l| l <= MAX_PERKS));
        assert!((2..=3).contains(&median), "median {median}");
        assert!((3.8..=5.0).contains(&mean), "mean {mean}");
        assert!(rare > 0.01 && rare < 0.05, "rare share {rare}");
    }

    #[test]
    fn exclusive_pairs_never_drawn_together() {
        let mut rng = Rng::seed_from_u64(6);
        for _ in 0..20_000 {
            let p = Persona::random(&mut rng);
            assert!(!(p.has(Perk::Liquid) && p.has(Perk::Illiquid)));
            assert!(!(p.has(Perk::Cyclical) && p.has(Perk::Defensive)));
        }
    }

    #[test]
    fn effects_stack() {
        let e = Persona::parse("AF").effect();
        assert!((e.kappa_mul - 1.8 * 0.6).abs() < 1e-12);
        assert!((e.sigma_u_mul - 0.8 * 1.5).abs() < 1e-12);
    }
}
