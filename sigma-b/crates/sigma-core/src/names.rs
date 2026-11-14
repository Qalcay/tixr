//! Unique ticker symbols.

use crate::rng::Rng;
use crate::security::Ticker;
use std::collections::HashSet;

/// Draw a ticker of 3-5 letters not currently in use. With up to 65,536
/// live securities and 26^4 = 456,976 four-letter symbols alone, a few
/// retries always suffice; the loop bound is a safety net.
pub fn draw_ticker(rng: &mut Rng, used: &mut HashSet<Ticker>) -> Ticker {
    for attempt in 0..256 {
        let len = match (rng.uniform(), attempt) {
            (_, a) if a > 64 => 5,
            (u, _) if u < 0.3 => 3,
            (u, _) if u < 0.8 => 4,
            _ => 5,
        };
        let mut t = Ticker { bytes: [0; 5], len };
        for b in t.bytes.iter_mut().take(len as usize) {
            *b = b'A' + rng.below(26) as u8;
        }
        if used.insert(t) {
            return t;
        }
    }
    unreachable!("ticker space exhausted")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tickers_are_unique_and_alphabetic() {
        let mut rng = Rng::seeded(4);
        let mut used = HashSet::new();
        for _ in 0..20_000 {
            let t = draw_ticker(&mut rng, &mut used);
            assert!(t.as_str().bytes().all(|b| b.is_ascii_uppercase()));
            assert!((3..=5).contains(&t.as_str().len()));
        }
        assert_eq!(used.len(), 20_000);
    }
}
