// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

/// Lowercase, trim, strip punctuation (keep alphanumerics and single spaces) — the shared key
/// form used for `ExerciseAlias` PKs and `HevyExerciseCatalog.title_normalized`.
pub fn normalize(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut last_was_space = false;
    for c in lower.chars() {
        if c.is_alphanumeric() {
            out.push(c);
            last_was_space = false;
        } else if !last_was_space {
            out.push(' ');
            last_was_space = true;
        }
    }
    out.trim().to_string()
}

/// Token-overlap containment score in `[0.0, 1.0]`: the fraction of `needle`'s tokens present
/// in `haystack`. Deliberately simple (no edit-distance crate) — see `correlator` tests for
/// why this suffices for the real fixture data.
pub fn token_overlap(needle: &str, haystack: &str) -> f64 {
    let needle_tokens: Vec<&str> = needle.split_whitespace().collect();
    if needle_tokens.is_empty() {
        return 0.0;
    }
    let haystack_tokens: std::collections::HashSet<&str> = haystack.split_whitespace().collect();
    let matched = needle_tokens
        .iter()
        .filter(|t| haystack_tokens.contains(*t))
        .count();
    matched as f64 / needle_tokens.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_punctuation_and_lowercases() {
        assert_eq!(normalize("Pull-Ups"), "pull ups");
        assert_eq!(normalize("Sled  push"), "sled push");
        assert_eq!(normalize("  Deadlift  "), "deadlift");
        assert_eq!(normalize("Squat (Barbell)"), "squat barbell");
    }

    #[test]
    fn token_overlap_full_and_partial() {
        assert_eq!(token_overlap("wall ball shot", "wall ball shot"), 1.0);
        assert!(token_overlap("wall balls", "wall ball shot") > 0.0);
        assert_eq!(token_overlap("snatch", "bike road"), 0.0);
    }
}
