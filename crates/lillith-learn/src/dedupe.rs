//! Duplicate notes (§3 step 5): same normalized title, or token-Jaccard of
//! the titles ≥ 0.8.

use std::collections::HashSet;

/// Merge threshold.
pub const JACCARD_MERGE: f64 = 0.8;

/// Lowercase alphanumeric words joined by single spaces.
pub fn normalize_title(t: &str) -> String {
    t.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn tokens(t: &str) -> HashSet<String> {
    normalize_title(t)
        .split(' ')
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Token Jaccard similarity of two titles (0..1).
pub fn jaccard(a: &str, b: &str) -> f64 {
    let (a, b) = (tokens(a), tokens(b));
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let inter = a.intersection(&b).count() as f64;
    let union = a.union(&b).count() as f64;
    inter / union
}

/// `true` when `a` and `b` name the same note.
pub fn same_note(a: &str, b: &str) -> bool {
    let (na, nb) = (normalize_title(a), normalize_title(b));
    (!na.is_empty() && na == nb) || jaccard(a, b) >= JACCARD_MERGE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        assert!(same_note(
            "Remove orphans: pacman",
            "remove ORPHANS pacman!"
        ));
        assert!(same_note(
            "pacman remove orphan packages quickly now",
            "pacman remove orphan packages quickly"
        ));
        assert!(!same_note(
            "pacman remove orphans",
            "pacman install package"
        ));
        assert!((jaccard("a b", "a c") - 1.0 / 3.0).abs() < 1e-9);
    }
}
