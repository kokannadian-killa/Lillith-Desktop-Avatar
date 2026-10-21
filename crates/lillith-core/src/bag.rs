//! Weighted shuffle bag with no immediate repeats, for idle variety.
//!
//! Every entry appears `weight` times per bag; the bag is shuffled, spread
//! so equal neighbours are separated, and refilled when empty. The same
//! clip is never picked twice in a row (also across refills) as long as
//! the pool has at least two distinct entries. Heavier entries simply come
//! up more often per round, so the flirty clips dominate without the same
//! one looping.

use crate::rng::Rng;

/// A weighted, no-immediate-repeat shuffle bag of names.
#[derive(Debug, Clone, Default)]
pub struct ShuffleBag {
    entries: Vec<(String, u32)>,
    bag: Vec<usize>,
    last: Option<usize>,
}

impl ShuffleBag {
    /// `entries` = `(name, weight)`; zero weights and duplicates are dropped.
    pub fn new(entries: impl IntoIterator<Item = (String, u32)>) -> Self {
        let mut v: Vec<(String, u32)> = Vec::new();
        for (n, w) in entries {
            if w == 0 || v.iter().any(|(m, _)| *m == n) {
                continue;
            }
            v.push((n, w));
        }
        Self {
            entries: v,
            bag: Vec::new(),
            last: None,
        }
    }

    /// Equal weights.
    pub fn uniform(names: impl IntoIterator<Item = String>) -> Self {
        Self::new(names.into_iter().map(|n| (n, 1)))
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Distinct names in the pool.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(n, _)| n.as_str())
    }

    /// Weight of `name` (0 = not in the pool).
    pub fn weight(&self, name: &str) -> u32 {
        self.entries
            .iter()
            .find(|(n, _)| n == name)
            .map_or(0, |e| e.1)
    }

    fn refill(&mut self, rng: &mut Rng) {
        let mut b: Vec<usize> = Vec::new();
        for (i, (_, w)) in self.entries.iter().enumerate() {
            b.extend(std::iter::repeat_n(i, *w as usize));
        }
        // Fisher–Yates.
        for i in (1..b.len()).rev() {
            let j = rng.index(i + 1);
            b.swap(i, j);
        }
        // Separate equal neighbours where possible (greedy swap forward).
        for i in 1..b.len() {
            if b[i] == b[i - 1] {
                if let Some(k) = (i + 1..b.len()).find(|&k| b[k] != b[i - 1]) {
                    b.swap(i, k);
                }
            }
        }
        // Popped from the back: reverse so the order above is kept.
        b.reverse();
        self.bag = b;
    }

    /// Next name, never equal to the previous pick when avoidable.
    pub fn next(&mut self, rng: &mut Rng) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }
        if self.bag.is_empty() {
            self.refill(rng);
        }
        let distinct = self.entries.len() > 1;
        let mut i = self.bag.pop()?;
        if distinct && Some(i) == self.last {
            // Swap in the first different entry from the bag (refilling if
            // the rest of the bag is all this entry).
            match self.bag.iter().rposition(|&k| k != i) {
                Some(p) => std::mem::swap(&mut self.bag[p], &mut i),
                None => {
                    self.refill(rng);
                    if let Some(p) = self.bag.iter().rposition(|&k| k != i) {
                        i = self.bag.remove(p);
                    }
                }
            }
        }
        self.last = Some(i);
        Some(self.entries[i].0.as_str())
    }
}

/// Picks an index into `n` items at random, avoiding `last` when `n > 1`.
pub fn pick_no_repeat(rng: &mut Rng, n: usize, last: Option<usize>) -> Option<usize> {
    if n == 0 {
        return None;
    }
    match last {
        Some(l) if n > 1 && l < n => Some((l + 1 + rng.index(n - 1)) % n),
        _ => Some(rng.index(n)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_repeats_immediately_and_respects_weights() {
        let mut rng = Rng::new(42);
        let mut b = ShuffleBag::new([
            ("lip_bite".to_string(), 3),
            ("hip_pop_wink".to_string(), 3),
            ("nod".to_string(), 1),
        ]);
        let mut prev: Option<String> = None;
        let mut counts = std::collections::BTreeMap::<String, u32>::new();
        for _ in 0..7000 {
            let n = b.next(&mut rng).unwrap().to_string();
            assert_ne!(prev.as_deref(), Some(n.as_str()), "immediate repeat");
            *counts.entry(n.clone()).or_default() += 1;
            prev = Some(n);
        }
        // 3:3:1 → ~3000 / 3000 / 1000.
        assert!(counts["lip_bite"] > 2 * counts["nod"]);
        assert!(counts["hip_pop_wink"] > 2 * counts["nod"]);
        assert!(counts["nod"] > 500);
    }

    #[test]
    fn heavy_single_entry_still_alternates() {
        let mut rng = Rng::new(7);
        let mut b = ShuffleBag::new([("a".to_string(), 9), ("b".to_string(), 1)]);
        let mut prev = String::new();
        for _ in 0..200 {
            let n = b.next(&mut rng).unwrap().to_string();
            assert_ne!(n, prev);
            prev = n;
        }
    }

    #[test]
    fn single_and_empty_pools() {
        let mut rng = Rng::new(1);
        let mut one = ShuffleBag::uniform(["only".to_string()]);
        assert_eq!(one.next(&mut rng), Some("only"));
        assert_eq!(one.next(&mut rng), Some("only"));
        let mut none = ShuffleBag::default();
        assert_eq!(none.next(&mut rng), None);
    }

    #[test]
    fn pick_no_repeat_avoids_last() {
        let mut rng = Rng::new(3);
        for _ in 0..500 {
            let i = pick_no_repeat(&mut rng, 4, Some(2)).unwrap();
            assert!(i < 4 && i != 2);
        }
        assert_eq!(pick_no_repeat(&mut rng, 1, Some(0)), Some(0));
        assert_eq!(pick_no_repeat(&mut rng, 0, None), None);
    }
}
