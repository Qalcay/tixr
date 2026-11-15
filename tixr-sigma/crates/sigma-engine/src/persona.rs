//! Stock personas: stacking perks lettered A through P.
//!
//! A persona is a set of up to 12 of the 16 perks, stored as a `u16` bitmask
//! (bit 0 = A ... bit 15 = P). The letter of each perk is the first letter of
//! its name, so a persona string such as `"DGK"` reads Data + Global + Key.
//!
//! # Design rule: one mechanism per perk
//! Every perk owns exactly one *signature mechanism* that no other perk
//! touches (an inflation hedge, contract wins, a verdict date, ...). The
//! remaining numbers on a perk are small flavour nudges. That is what keeps
//! two perks from feeling alike, and it is also what makes stacking safe:
//! signature mechanisms are independent processes that add up, and the shared
//! nudges are clamped in [`crate::security::Params::draw`].
//!
//! | perk          | signature mechanism                                   |
//! |---------------|-------------------------------------------------------|
//! | A Assets      | inflation exposure (hard-asset hedge)                 |
//! | B Builder     | contract wins: lumpy permanent up-steps               |
//! | C Crypto      | fat-tailed, symmetric fundamental jumps               |
//! | D Data        | steady compounding drift, muted downside jumps        |
//! | E Expansion   | long duration: growth that lives and dies by rates    |
//! | F Finance     | business-cycle factor: drift follows the output gap   |
//! | G Global      | loads on the world factor instead of its own exchange |
//! | H Hype        | one-off listing hump that unwinds slowly              |
//! | I Innovate    | frequent positive sentiment catalysts that fade       |
//! | J Junk        | leverage: credit exposure, close to the default line  |
//! | K Key         | cannot be delisted: rescued and restructured instead  |
//! | L Legacy      | drift that decays with company age                    |
//! | M Meager      | everything damped, drift pulled toward zero           |
//! | N Novel       | a verdict date: one large make-or-break move          |
//! | O Opportunity | sentiment crashes that recover                        |
//! | P Profit      | post-crisis recovery factor: outperforms after stress |
//!
//! Persona length distribution (0..=12 perks): median 3, mean about 4.1, and
//! 11 or 12 perks in about 3 % of securities. See `LENGTH_WEIGHTS`.

use crate::rng::Rng;

pub const PERK_COUNT: usize = 16;
pub const MAX_PERKS: usize = 12;

/// Relative weight for a persona of length 0, 1, ..., 12 (sums to 100).
pub const LENGTH_WEIGHTS: [u32; MAX_PERKS + 1] = [6, 12, 17, 16, 12, 9, 7, 6, 5, 4, 3, 2, 1];

/// Pairs whose descriptions contradict each other. They are never drawn
/// together, and [`Persona::sanitized`] removes the later letter of a pair
/// from hand-written personas (the player's launch dialog).
///
/// * Crypto vs Meager: "extreme boom-or-bust" vs "consistent low volatility".
/// * Crypto vs Data: "catastrophic collapses" vs "rarely declines".
/// * Legacy vs Novel: "established incumbent" vs "unproven concept".
/// * Legacy vs Expansion: "slowly loses competitiveness" vs a growth story.
/// * Legacy vs Hype: "stable early on" vs a listing-day mania.
pub const EXCLUSIVE_PAIRS: [(Perk, Perk); 5] = [
    (Perk::Crypto, Perk::Meager),
    (Perk::Crypto, Perk::Data),
    (Perk::Legacy, Perk::Novel),
    (Perk::Legacy, Perk::Expansion),
    (Perk::Legacy, Perk::Hype),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Perk {
    Assets = 0,
    Builder,
    Crypto,
    Data,
    Expansion,
    Finance,
    Global,
    Hype,
    Innovate,
    Junk,
    Key,
    Legacy,
    Meager,
    Novel,
    Opportunity,
    Profit,
}

pub const ALL_PERKS: [Perk; PERK_COUNT] = [
    Perk::Assets,
    Perk::Builder,
    Perk::Crypto,
    Perk::Data,
    Perk::Expansion,
    Perk::Finance,
    Perk::Global,
    Perk::Hype,
    Perk::Innovate,
    Perk::Junk,
    Perk::Key,
    Perk::Legacy,
    Perk::Meager,
    Perk::Novel,
    Perk::Opportunity,
    Perk::Profit,
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
            Perk::Assets => "Assets",
            Perk::Builder => "Builder",
            Perk::Crypto => "Crypto",
            Perk::Data => "Data",
            Perk::Expansion => "Expansion",
            Perk::Finance => "Finance",
            Perk::Global => "Global",
            Perk::Hype => "Hype",
            Perk::Innovate => "Innovate",
            Perk::Junk => "Junk",
            Perk::Key => "Key",
            Perk::Legacy => "Legacy",
            Perk::Meager => "Meager",
            Perk::Novel => "Novel",
            Perk::Opportunity => "Opportunity",
            Perk::Profit => "Profit",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Perk::Assets => "Asset-rich: rises with inflation, shrugs off rate moves.",
            Perk::Builder => "Wins big contracts: sudden steps up, quiet stretches between.",
            Perk::Crypto => "Boom or bust: huge jumps in either direction.",
            Perk::Data => "Compounds steadily; bad surprises are rare and small.",
            Perk::Expansion => "Growth story: fast drift, but very sensitive to rates.",
            Perk::Finance => "Follows the economy: beats in expansions, lags in recessions.",
            Perk::Global => "Diversified: follows the world market, less hit by local crises.",
            Perk::Hype => "Mania at listing, then a long unwind; rarely sees its high again.",
            Perk::Innovate => "Frequent product pops that fade over the following weeks.",
            Perk::Junk => "Leveraged: lives close to default and tracks credit spreads.",
            Perk::Key => "Systemically important: rescued and restructured, never delisted.",
            Perk::Legacy => "Incumbent: stable for years, then a slow decline with age.",
            Perk::Meager => "Dependable and dull: low volatility, hugs fair value.",
            Perk::Novel => "Unproven: one verdict day decides it, up or down, 50/50.",
            Perk::Opportunity => "Crash-prone, but the crashes are sentiment and it recovers.",
            Perk::Profit => "Cash machine: outperforms for months after a market crisis ends.",
        }
    }

    pub fn effect(self) -> PerkEffect {
        let e = PerkEffect::NEUTRAL;
        match self {
            Perk::Assets => PerkEffect { inflation_add: 2.5, duration_add: -1.5, sigma_f_mul: 0.9, ..e },
            Perk::Builder => PerkEffect { wins: 1.0, barrier_add: 0.1, ..e },
            Perk::Crypto => PerkEffect {
                jump_rate_mul: 3.0,
                jump_sd_mul: 2.2,
                jump_mean_add: 0.02,
                sigma_u_mul: 1.5,
                beta_add: -0.2,
                duration_add: -1.5,
                barrier_add: -0.35,
                rate_mul: 1.5,
                ..e
            },
            Perk::Data => PerkEffect {
                alpha_add: 0.03,
                sigma_f_mul: 0.8,
                jump_rate_mul: 0.6,
                jump_mean_add: 0.015,
                barrier_add: 0.5,
                kappa_mul: 1.2,
                ..e
            },
            Perk::Expansion => PerkEffect { alpha_add: 0.045, sigma_f_mul: 1.2, duration_add: 4.5, ..e },
            Perk::Finance => PerkEffect { cycle: 1.0, beta_add: 0.25, credit_add: 1.5, ..e },
            Perk::Global => {
                PerkEffect { global_share: 0.6, inflation_mul: 0.3, regime_sens_mul: 0.5, sigma_f_mul: 0.9, ..e }
            }
            Perk::Hype => {
                PerkEffect { hype: 1.0, sigma_u_mul: 1.5, kappa_mul: 0.6, alpha_add: -0.02, rate_mul: 1.4, ..e }
            }
            Perk::Innovate => PerkEffect { catalysts: 1.0, alpha_add: 0.01, sigma_f_mul: 1.1, ..e },
            Perk::Junk => PerkEffect { barrier_add: -0.55, credit_add: 3.0, beta_add: 0.2, sigma_f_mul: 1.15, ..e },
            Perk::Key => {
                PerkEffect { rescue: true, barrier_add: 0.6, rate_mul: 1.5, impact_mul: 0.55, sigma_f_mul: 0.9, ..e }
            }
            Perk::Legacy => {
                PerkEffect { decline: 1.0, sigma_f_mul: 0.8, beta_add: -0.1, jump_rate_mul: 0.8, barrier_add: 0.3, ..e }
            }
            Perk::Meager => PerkEffect {
                alpha_mul: 0.4,
                sigma_f_mul: 0.65,
                sigma_u_mul: 0.7,
                kappa_mul: 1.7,
                beta_add: -0.25,
                jump_rate_mul: 0.5,
                ..e
            },
            Perk::Novel => PerkEffect { verdict: 1.0, barrier_add: -0.15, impact_mul: 1.5, rate_mul: 0.6, ..e },
            Perk::Opportunity => PerkEffect { crashes: 1.0, alpha_add: 0.01, ..e },
            Perk::Profit => PerkEffect { recovery: 1.0, alpha_add: 0.01, barrier_add: 0.2, credit_add: -0.3, ..e },
        }
    }
}

/// How one perk changes a security's parameters.
///
/// Three composition rules, by field suffix:
/// * `_add`: offsets, summed across perks.
/// * `_mul`: multipliers, multiplied across perks.
/// * no suffix (`wins`, `hype`, ...): strength of a signature mechanism,
///   0 = off. Summed, but each has a single owner today so it is 0 or 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PerkEffect {
    pub beta_add: f64,
    pub alpha_add: f64,
    pub alpha_mul: f64,
    pub sigma_f_mul: f64,
    pub kappa_mul: f64,
    pub sigma_u_mul: f64,
    pub jump_rate_mul: f64,
    pub jump_mean_add: f64,
    pub jump_sd_mul: f64,
    pub rate_mul: f64,
    pub impact_mul: f64,
    pub barrier_add: f64,
    pub duration_add: f64,
    pub inflation_add: f64,
    pub inflation_mul: f64,
    pub credit_add: f64,
    pub regime_sens_mul: f64,
    /// Fraction of market beta moved from the home exchange to the world.
    pub global_share: f64,
    pub cycle: f64,
    pub recovery: f64,
    pub wins: f64,
    pub catalysts: f64,
    pub crashes: f64,
    pub hype: f64,
    pub decline: f64,
    pub verdict: f64,
    pub rescue: bool,
}

impl PerkEffect {
    pub const NEUTRAL: PerkEffect = PerkEffect {
        beta_add: 0.0,
        alpha_add: 0.0,
        alpha_mul: 1.0,
        sigma_f_mul: 1.0,
        kappa_mul: 1.0,
        sigma_u_mul: 1.0,
        jump_rate_mul: 1.0,
        jump_mean_add: 0.0,
        jump_sd_mul: 1.0,
        rate_mul: 1.0,
        impact_mul: 1.0,
        barrier_add: 0.0,
        duration_add: 0.0,
        inflation_add: 0.0,
        inflation_mul: 1.0,
        credit_add: 0.0,
        regime_sens_mul: 1.0,
        global_share: 0.0,
        cycle: 0.0,
        recovery: 0.0,
        wins: 0.0,
        catalysts: 0.0,
        crashes: 0.0,
        hype: 0.0,
        decline: 0.0,
        verdict: 0.0,
        rescue: false,
    };

    fn stack(self, o: PerkEffect) -> PerkEffect {
        PerkEffect {
            beta_add: self.beta_add + o.beta_add,
            alpha_add: self.alpha_add + o.alpha_add,
            alpha_mul: self.alpha_mul * o.alpha_mul,
            sigma_f_mul: self.sigma_f_mul * o.sigma_f_mul,
            kappa_mul: self.kappa_mul * o.kappa_mul,
            sigma_u_mul: self.sigma_u_mul * o.sigma_u_mul,
            jump_rate_mul: self.jump_rate_mul * o.jump_rate_mul,
            jump_mean_add: self.jump_mean_add + o.jump_mean_add,
            jump_sd_mul: self.jump_sd_mul * o.jump_sd_mul,
            rate_mul: self.rate_mul * o.rate_mul,
            impact_mul: self.impact_mul * o.impact_mul,
            barrier_add: self.barrier_add + o.barrier_add,
            duration_add: self.duration_add + o.duration_add,
            inflation_add: self.inflation_add + o.inflation_add,
            inflation_mul: self.inflation_mul * o.inflation_mul,
            credit_add: self.credit_add + o.credit_add,
            regime_sens_mul: self.regime_sens_mul * o.regime_sens_mul,
            global_share: self.global_share + o.global_share,
            cycle: self.cycle + o.cycle,
            recovery: self.recovery + o.recovery,
            wins: self.wins + o.wins,
            catalysts: self.catalysts + o.catalysts,
            crashes: self.crashes + o.crashes,
            hype: self.hype + o.hype,
            decline: self.decline + o.decline,
            verdict: self.verdict + o.verdict,
            rescue: self.rescue || o.rescue,
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

    /// Would adding `p` put two contradicting perks in the same persona?
    pub fn clashes_with(self, p: Perk) -> bool {
        EXCLUSIVE_PAIRS.iter().any(|&(a, b)| (p == a && self.has(b)) || (p == b && self.has(a)))
    }

    /// No contradicting pair and at most [`MAX_PERKS`] perks.
    pub fn is_valid(self) -> bool {
        self.len() <= MAX_PERKS && !EXCLUSIVE_PAIRS.iter().any(|&(a, b)| self.has(a) && self.has(b))
    }

    /// Drop the alphabetically later perk of each contradicting pair, then
    /// trim to [`MAX_PERKS`]. Idempotent; a valid persona is returned as is.
    pub fn sanitized(self) -> Persona {
        let mut p = self;
        for &(a, b) in &EXCLUSIVE_PAIRS {
            if p.has(a) && p.has(b) {
                p = p.without(if (a as u8) > (b as u8) { a } else { b });
            }
        }
        while p.len() > MAX_PERKS {
            let last = p.perks().last().expect("non-empty");
            p = p.without(last);
        }
        p
    }

    /// Letters in alphabetical order, e.g. `"BGL"`; `"-"` for no perks.
    pub fn letters(self) -> String {
        if self.is_empty() {
            return "-".to_string();
        }
        self.perks().map(Perk::letter).collect()
    }

    /// Parse `"bgl"` / `"B G L"`; unknown characters are ignored, and the
    /// result is [`sanitized`](Self::sanitized).
    pub fn parse(s: &str) -> Persona {
        let mut p = Persona::EMPTY;
        for c in s.chars() {
            if let Some(perk) = Perk::from_letter(c) {
                p = p.with(perk);
            }
        }
        p.sanitized()
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
            if !persona.clashes_with(candidate) {
                persona = persona.with(candidate);
            }
        }
        persona
    }
}

impl crate::save::Persist for Persona {
    fn put(&self, w: &mut crate::save::Writer) {
        self.0.put(w);
    }
    fn get(r: &mut crate::save::Reader) -> crate::save::Result<Self> {
        Ok(Persona(u16::get(r)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_round_trip() {
        let p = Persona::parse("kgd");
        assert_eq!(p.letters(), "DGK");
        assert_eq!(Persona::parse(&p.letters()), p);
        assert_eq!(Persona::EMPTY.letters(), "-");
        assert_eq!(Perk::from_letter('p'), Some(Perk::Profit));
        assert_eq!(Perk::from_letter('Q'), None);
    }

    #[test]
    fn letter_is_first_letter_of_name() {
        for p in ALL_PERKS {
            assert_eq!(p.name().chars().next(), Some(p.letter()));
        }
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
        assert!(rare > 0.005 && rare < 0.05, "rare share {rare}");
    }

    #[test]
    fn contradicting_perks_are_never_drawn_together() {
        let mut rng = Rng::seed_from_u64(6);
        for _ in 0..50_000 {
            assert!(Persona::random(&mut rng).is_valid());
        }
    }

    #[test]
    fn sanitize_drops_the_later_letter_and_is_idempotent() {
        // C clashes with D and M; L clashes with E, H and N.
        let p = Persona::parse("CDMLEHN");
        assert_eq!(p.letters(), "CEH");
        assert!(p.is_valid());
        assert_eq!(p.sanitized(), p);
        // All 16 letters: still at most 12 and still valid.
        let all = Persona(u16::MAX).sanitized();
        assert!(all.is_valid() && all.len() == MAX_PERKS, "{}", all.letters());
    }

    /// Every perk must own a mechanism nobody else has; this is the
    /// "no two perks feel alike" rule made executable.
    #[test]
    fn every_perk_has_a_unique_signature() {
        let signature = |e: &PerkEffect| -> Vec<bool> {
            vec![
                e.inflation_add != 0.0,
                e.wins != 0.0,
                e.jump_sd_mul != 1.0,
                e.jump_rate_mul < 1.0 && e.alpha_add > 0.02,
                e.duration_add > 0.0,
                e.cycle != 0.0,
                e.global_share != 0.0,
                e.hype != 0.0,
                e.catalysts != 0.0,
                e.credit_add > 2.0,
                e.rescue,
                e.decline != 0.0,
                e.alpha_mul != 1.0,
                e.verdict != 0.0,
                e.crashes != 0.0,
                e.recovery != 0.0,
            ]
        };
        for (i, p) in ALL_PERKS.iter().enumerate() {
            let sig = signature(&p.effect());
            assert!(sig[i], "{} lacks its own signature", p.name());
            assert_eq!(sig.iter().filter(|&&b| b).count(), 1, "{} borrows another perk's signature", p.name());
        }
    }

    #[test]
    fn effects_stack() {
        let e = Persona::parse("DM").effect();
        assert!((e.kappa_mul - 1.2 * 1.7).abs() < 1e-12);
        assert!((e.sigma_f_mul - 0.8 * 0.65).abs() < 1e-12);
        assert!((e.alpha_add - 0.03).abs() < 1e-12);
    }
}
