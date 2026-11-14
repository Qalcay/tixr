//! Stock personas: stacking perks A to P.
//!
//! Every security carries a set of up to 12 perks drawn from 16 letters.
//! The set is stored as one `u16` bitmask (bit 0 = A ... bit 15 = P), which
//! is why there are exactly 16 letters: the whole persona costs two bytes
//! and set operations are single CPU instructions.
//!
//! ## Length distribution
//! The spec asks for lengths 0..=12 with a median of 2-3, an average around
//! 4 (low end of "4-7") and rare 11-12. A median below the mean means a
//! right-skewed distribution: most stocks are plain, a long tail of stocks
//! are loaded with character. `LENGTH_WEIGHTS` encodes it directly and the
//! tests check median 3, mean ~4.06, P(11..=12) = 2%.
//!
//! ## Stacking
//! Each perk nudges parameters by a log-multiplier (or an additive amount).
//! Nudges add up, then pass through a soft cap `cap * tanh(sum / cap)`, so
//! two volatile perks are more volatile than one, but twelve perks can never
//! produce an absurd parameter. This soft cap is one of the "forces" that
//! keep sigma stable where the original could run away.

use crate::rng::Rng;

/// The 16 perks. Order matters: it is the bit index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Perk {
    Anchor = 0,
    Bellwether,
    Cyclical,
    Dividend,
    Emerging,
    Fortress,
    Gambit,
    Hype,
    Innovator,
    Junk,
    Kontrarian,
    LowFloat,
    Momentum,
    Network,
    Overseas,
    PricingPower,
}

pub const ALL_PERKS: [Perk; 16] = [
    Perk::Anchor,
    Perk::Bellwether,
    Perk::Cyclical,
    Perk::Dividend,
    Perk::Emerging,
    Perk::Fortress,
    Perk::Gambit,
    Perk::Hype,
    Perk::Innovator,
    Perk::Junk,
    Perk::Kontrarian,
    Perk::LowFloat,
    Perk::Momentum,
    Perk::Network,
    Perk::Overseas,
    Perk::PricingPower,
];

impl Perk {
    pub fn letter(self) -> char {
        (b'A' + self as u8) as char
    }

    pub fn name(self) -> &'static str {
        match self {
            Perk::Anchor => "Anchor",
            Perk::Bellwether => "Bellwether",
            Perk::Cyclical => "Cyclical",
            Perk::Dividend => "Dividend",
            Perk::Emerging => "Emerging",
            Perk::Fortress => "Fortress",
            Perk::Gambit => "Gambit",
            Perk::Hype => "Hype",
            Perk::Innovator => "Innovator",
            Perk::Junk => "Junk credit",
            Perk::Kontrarian => "Kontrarian",
            Perk::LowFloat => "Low float",
            Perk::Momentum => "Momentum",
            Perk::Network => "Network",
            Perk::Overseas => "Overseas",
            Perk::PricingPower => "Pricing power",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Perk::Anchor => "Blue chip: calm, quickly pulled back to fair value, rarely acquired.",
            Perk::Bellwether => "Moves with the whole market (high beta) and trades deep.",
            Perk::Cyclical => "Earnings swing hard with the output gap.",
            Perk::Dividend => "Pays out most of its earnings; grows slowly.",
            Perk::Emerging => "Fast growth, bumpy fundamentals, a small risk premium.",
            Perk::Fortress => "Little debt: hard to bankrupt.",
            Perk::Gambit => "Speculative: frequent, large news jumps.",
            Perk::Hype => "Crowd favourite: big sentiment swings that linger.",
            Perk::Innovator => "Reinvests everything; growth with positive surprises.",
            Perk::Junk => "Heavily indebted: rate-sensitive and fragile.",
            Perk::Kontrarian => "Defensive: low beta, ignores the cycle.",
            Perk::LowFloat => "Few shares trade: orders move the price a lot.",
            Perk::Momentum => "Mispricing persists: trends run longer.",
            Perk::Network => "Growth that fades slowly (network effects).",
            Perk::Overseas => "Sells abroad: less tied to the domestic cycle and sector.",
            Perk::PricingPower => "Passes inflation straight through; steady margins.",
        }
    }

    /// Base draw weight (rarity). Some letters are simply less common.
    fn rarity(self) -> f64 {
        match self {
            Perk::Bellwether | Perk::Cyclical | Perk::Dividend | Perk::Kontrarian => 1.0,
            Perk::LowFloat | Perk::Overseas => 1.0,
            Perk::Anchor | Perk::Fortress | Perk::Momentum | Perk::Network => 0.7,
            Perk::PricingPower => 0.7,
            Perk::Emerging | Perk::Gambit | Perk::Hype | Perk::Innovator | Perk::Junk => 0.5,
        }
    }

    fn bit(self) -> u16 {
        1 << (self as u8)
    }
}

/// Pairs that cannot share a persona. Four pairs out of 16 letters cap the
/// maximum length at exactly 16 - 4 = 12, matching the spec.
const CONFLICTS: [(Perk, Perk); 4] = [
    (Perk::Anchor, Perk::Gambit),
    (Perk::Fortress, Perk::Junk),
    (Perk::Bellwether, Perk::Kontrarian),
    (Perk::Dividend, Perk::Innovator),
];

/// P(length = k) for k = 0..=12. Sums to 1.0.
pub const LENGTH_WEIGHTS: [f64; 13] = [
    0.05, 0.13, 0.20, 0.15, 0.10, 0.08, 0.07, 0.07, 0.06, 0.04, 0.03, 0.015, 0.005,
];

/// A persona: a set of perks packed into a `u16`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PerkSet(pub u16);

impl PerkSet {
    pub fn has(self, p: Perk) -> bool {
        self.0 & p.bit() != 0
    }

    pub fn len(self) -> u32 {
        self.0.count_ones()
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = Perk> {
        ALL_PERKS.into_iter().filter(move |p| self.has(*p))
    }

    /// Letters in alphabetical order, e.g. "BDK". Empty personas show "-".
    pub fn letters(self) -> String {
        if self.is_empty() {
            return "-".into();
        }
        self.iter().map(Perk::letter).collect()
    }

    pub fn is_valid(self) -> bool {
        CONFLICTS.iter().all(|(a, b)| !(self.has(*a) && self.has(*b))) && self.len() <= 12
    }

    /// Draw a persona. `affinity` lets a sector tilt the odds (a tech stock
    /// is more likely to be an Innovator than a utility is).
    pub fn draw(rng: &mut Rng, affinity: &[(Perk, f64)]) -> Self {
        let target = draw_length(rng);
        let mut weights = [0.0f64; 16];
        for p in ALL_PERKS {
            weights[p as usize] = p.rarity();
        }
        for &(p, m) in affinity {
            weights[p as usize] *= m;
        }
        let mut set = PerkSet(0);
        for _ in 0..target {
            let total: f64 = weights.iter().sum();
            if total <= 0.0 {
                break;
            }
            let mut u = rng.uniform() * total;
            let mut pick = None;
            for (i, w) in weights.iter().enumerate() {
                if *w <= 0.0 {
                    continue;
                }
                if u < *w {
                    pick = Some(i);
                    break;
                }
                u -= w;
            }
            let Some(i) = pick.or_else(|| weights.iter().rposition(|w| *w > 0.0)) else {
                break;
            };
            let perk = ALL_PERKS[i];
            set.0 |= perk.bit();
            weights[i] = 0.0;
            // Remove the partner of any conflict pair we just entered.
            for (a, b) in CONFLICTS {
                if a == perk {
                    weights[b as usize] = 0.0;
                }
                if b == perk {
                    weights[a as usize] = 0.0;
                }
            }
        }
        set
    }

    /// Fold the perks into parameter modifiers (see module docs).
    pub fn modifiers(self) -> Modifiers {
        let mut acc = Modifiers::NEUTRAL_LOG;
        for p in self.iter() {
            apply(&mut acc, p);
        }
        acc.finish()
    }
}

fn draw_length(rng: &mut Rng) -> u32 {
    let mut u = rng.uniform();
    for (k, w) in LENGTH_WEIGHTS.iter().enumerate() {
        if u < *w {
            return k as u32;
        }
        u -= w;
    }
    12
}

/// Parameter modifiers produced by a persona. Multipliers are 1.0 when
/// neutral, additive terms 0.0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Modifiers {
    /// Volatility of fundamentals (earnings).
    pub fund_vol: f64,
    /// Volatility of sentiment (mispricing).
    pub sent_vol: f64,
    /// Speed at which mispricing is arbitraged away.
    pub reversion: f64,
    /// Market beta.
    pub beta: f64,
    /// Sector beta.
    pub sector_beta: f64,
    /// Sensitivity of earnings to the output gap.
    pub cyclicality: f64,
    /// Added to excess growth (per year).
    pub growth: f64,
    /// Multiplies how fast excess growth fades.
    pub growth_fade: f64,
    /// Added to payout ratio.
    pub payout: f64,
    /// Added to leverage (debt / assets).
    pub leverage: f64,
    /// Multiplies default hazard.
    pub default_risk: f64,
    /// Jump frequency and size multipliers, and an added jump mean.
    pub jump_rate: f64,
    pub jump_size: f64,
    pub jump_mean: f64,
    /// Inflation pass-through to earnings (0..1, added).
    pub pass_through: f64,
    /// Liquidity (depth) multiplier; low = orders move price more.
    pub liquidity: f64,
    /// Added to cost of equity (per year).
    pub premium: f64,
    /// Takeover hazard multiplier.
    pub takeover: f64,
    /// Sampling attention multiplier.
    pub attention: f64,
}

impl Modifiers {
    /// Accumulator state: multipliers held as log sums.
    const NEUTRAL_LOG: Modifiers = Modifiers {
        fund_vol: 0.0,
        sent_vol: 0.0,
        reversion: 0.0,
        beta: 0.0,
        sector_beta: 0.0,
        cyclicality: 0.0,
        growth: 0.0,
        growth_fade: 0.0,
        payout: 0.0,
        leverage: 0.0,
        default_risk: 0.0,
        jump_rate: 0.0,
        jump_size: 0.0,
        jump_mean: 0.0,
        pass_through: 0.0,
        liquidity: 0.0,
        premium: 0.0,
        takeover: 0.0,
        attention: 0.0,
    };

    fn finish(self) -> Modifiers {
        // Multipliers: soft-capped to [1/3, 3]. Additive terms: soft-capped
        // to their own sensible ranges.
        let m = |x: f64| soft(x, 3f64.ln()).exp();
        Modifiers {
            fund_vol: m(self.fund_vol),
            sent_vol: m(self.sent_vol),
            reversion: m(self.reversion),
            beta: m(self.beta),
            sector_beta: m(self.sector_beta),
            cyclicality: m(self.cyclicality),
            growth: soft(self.growth, 0.08),
            growth_fade: m(self.growth_fade),
            payout: soft(self.payout, 0.4),
            leverage: soft(self.leverage, 0.4),
            default_risk: m(self.default_risk),
            jump_rate: m(self.jump_rate),
            jump_size: m(self.jump_size),
            jump_mean: soft(self.jump_mean, 0.02),
            pass_through: soft(self.pass_through, 0.3),
            liquidity: m(self.liquidity),
            premium: soft(self.premium, 0.04),
            takeover: m(self.takeover),
            attention: m(self.attention),
        }
    }
}

impl Default for Modifiers {
    fn default() -> Self {
        Modifiers::NEUTRAL_LOG.finish()
    }
}

/// `cap * tanh(x / cap)`: linear near zero, flattens out at +-cap.
fn soft(x: f64, cap: f64) -> f64 {
    cap * (x / cap).tanh()
}

fn apply(m: &mut Modifiers, p: Perk) {
    let ln = f64::ln;
    match p {
        Perk::Anchor => {
            m.fund_vol += ln(0.7);
            m.sent_vol += ln(0.7);
            m.reversion += ln(1.4);
            m.takeover += ln(0.5);
            m.attention += ln(1.5);
        }
        Perk::Bellwether => {
            m.beta += ln(1.35);
            m.liquidity += ln(1.6);
            m.attention += ln(1.4);
        }
        Perk::Cyclical => {
            m.cyclicality += ln(2.2);
            m.beta += ln(1.15);
        }
        Perk::Dividend => {
            m.payout += 0.25;
            m.growth -= 0.015;
            m.fund_vol += ln(0.9);
        }
        Perk::Emerging => {
            m.growth += 0.04;
            m.fund_vol += ln(1.4);
            m.premium += 0.01;
        }
        Perk::Fortress => {
            m.leverage -= 0.2;
            m.default_risk += ln(0.4);
            m.fund_vol += ln(0.85);
        }
        Perk::Gambit => {
            m.jump_rate += ln(2.5);
            m.jump_size += ln(1.5);
            m.sent_vol += ln(1.4);
            m.attention += ln(1.6);
        }
        Perk::Hype => {
            m.sent_vol += ln(1.6);
            m.reversion += ln(0.7);
            m.attention += ln(2.5);
        }
        Perk::Innovator => {
            m.growth += 0.05;
            m.payout -= 0.2;
            m.fund_vol += ln(1.3);
            m.jump_mean += 0.01;
        }
        Perk::Junk => {
            m.leverage += 0.3;
            m.default_risk += ln(2.0);
            m.beta += ln(1.1);
            m.premium += 0.02;
        }
        Perk::Kontrarian => {
            m.beta += ln(0.55);
            m.cyclicality += ln(0.4);
        }
        Perk::LowFloat => {
            m.liquidity += ln(0.3);
            m.sent_vol += ln(1.25);
        }
        Perk::Momentum => {
            m.reversion += ln(0.5);
        }
        Perk::Network => {
            m.growth += 0.03;
            m.growth_fade += ln(0.5);
        }
        Perk::Overseas => {
            m.cyclicality += ln(0.7);
            m.sector_beta += ln(0.6);
            m.fund_vol += ln(1.1);
        }
        Perk::PricingPower => {
            m.pass_through += 0.3;
            m.fund_vol += ln(0.85);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_sum_to_one() {
        let s: f64 = LENGTH_WEIGHTS.iter().sum();
        assert!((s - 1.0).abs() < 1e-12);
    }

    #[test]
    fn length_distribution_matches_spec() {
        let mut rng = Rng::seeded(2024);
        let n = 100_000;
        let mut lens: Vec<u32> = (0..n).map(|_| PerkSet::draw(&mut rng, &[]).len()).collect();
        lens.sort_unstable();
        let median = lens[n / 2];
        let mean = lens.iter().map(|&l| l as f64).sum::<f64>() / n as f64;
        let rare = lens.iter().filter(|&&l| l >= 11).count() as f64 / n as f64;
        assert!((2..=3).contains(&median), "median {median}");
        assert!((3.8..=4.4).contains(&mean), "mean {mean}");
        assert!(rare > 0.005 && rare < 0.04, "rare {rare}");
        assert!(*lens.last().unwrap() <= 12);
    }

    #[test]
    fn personas_respect_conflicts() {
        let mut rng = Rng::seeded(1);
        for _ in 0..20_000 {
            let p = PerkSet::draw(&mut rng, &[(Perk::Gambit, 3.0)]);
            assert!(p.is_valid(), "{}", p.letters());
        }
    }

    #[test]
    fn stacking_is_soft_capped() {
        // Every multiplier stays within [1/3, 3] even for a 12-perk persona.
        let all = PerkSet(0xFFFF);
        let m = all.modifiers();
        for v in [m.fund_vol, m.sent_vol, m.reversion, m.beta, m.liquidity, m.attention] {
            assert!((1.0 / 3.0..=3.0).contains(&v), "{v}");
        }
        assert!(m.growth.abs() <= 0.08);
    }

    #[test]
    fn letters_render() {
        let p = PerkSet(Perk::Anchor.bit() | Perk::Dividend.bit() | Perk::PricingPower.bit());
        assert_eq!(p.letters(), "ADP");
        assert_eq!(PerkSet(0).letters(), "-");
    }
}
