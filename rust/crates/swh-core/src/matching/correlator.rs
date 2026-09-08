// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

use super::normalize::{normalize, token_overlap};
use super::ParsedExerciseLine;
use crate::sugarwod::model::SugarWodMovement;

/// Minimum token-overlap score for a `movements[]` entry to be considered corroboration for a
/// parsed line, rather than noise (see the plan's correction #1 — SugarWOD's `movements[]` is
/// often empty and never a reliable subset/superset of what's actually in `description`).
const CORRELATION_THRESHOLD: f64 = 0.5;

/// Returns the resolution key to look up for each parsed line, in the same order: the
/// normalized name of a correlated `movements[]` entry if one crosses the threshold, else the
/// line's own normalized text. Correctly returns all-fallback keys when `movements` is empty,
/// which real production data shows is the common case, not the edge case.
pub fn resolution_keys(
    lines: &[ParsedExerciseLine],
    movements: &[SugarWodMovement],
) -> Vec<String> {
    let normalized_movements: Vec<String> = movements.iter().map(|m| normalize(&m.name)).collect();

    lines
        .iter()
        .map(|line| {
            let line_norm = normalize(&line.name_text);
            let best = normalized_movements
                .iter()
                .map(|m| {
                    (
                        m,
                        token_overlap(&line_norm, m).max(token_overlap(m, &line_norm)),
                    )
                })
                .filter(|(_, score)| *score >= CORRELATION_THRESHOLD)
                .max_by(|a, b| a.1.total_cmp(&b.1));

            match best {
                Some((movement_norm, _)) => movement_norm.clone(),
                None => line_norm,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn movement(name: &str) -> SugarWodMovement {
        SugarWodMovement {
            id: "x".to_string(),
            name: name.to_string(),
        }
    }

    fn line(name: &str) -> ParsedExerciseLine {
        ParsedExerciseLine {
            name_text: name.to_string(),
            sets: vec![],
            rpe_note: None,
            rest_seconds_after: None,
        }
    }

    #[test]
    fn uncorrelated_movements_are_discarded_real_fixture_case() {
        // Real workout-1 fixture: movements[] lists exercises that never appear in the
        // description at all — every line must fall back to its own text, not a bogus movement.
        let movements = vec![
            movement("Bike (Road)"),
            movement("Jump Rope (Double Unders)"),
            movement("Muscle Snatch"),
            movement("Muscle-Up"),
            movement("Snatch"),
        ];
        let lines = vec![
            line("Run"),
            line("Burpee Broad Jumps"),
            line("Farmers Carry"),
            line("Toes to Bar"),
            line("Farmers Walking Lunge"),
            line("Pull-Ups"),
        ];
        let keys = resolution_keys(&lines, &movements);
        assert_eq!(
            keys,
            vec![
                "run",
                "burpee broad jumps",
                "farmers carry",
                "toes to bar",
                "farmers walking lunge",
                "pull ups",
            ]
        );
    }

    #[test]
    fn empty_movements_resolves_every_line_via_fallback() {
        let lines = vec![line("Run"), line("SKI"), line("Sled push")];
        let keys = resolution_keys(&lines, &[]);
        assert_eq!(keys, vec!["run", "ski", "sled push"]);
    }

    #[test]
    fn correlated_movement_wins_over_raw_line_text() {
        let movements = vec![movement("Wall Ball Shot")];
        let lines = vec![line("100 Wall balls".trim_start_matches("100 "))]; // "Wall balls"
        let keys = resolution_keys(&lines, &movements);
        assert_eq!(keys, vec!["wall ball shot"]);
    }
}
