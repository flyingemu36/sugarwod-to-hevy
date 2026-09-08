// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

use super::model::{RoutineExercise, RoutineSet};

/// One fully-resolved exercise, ready to become a `RoutineExercise` in the Hevy payload —
/// the bridge between `matching::resolver`/`matching::load_calc` output and the wire format.
pub struct MatchedExercise {
    pub hevy_template_id: String,
    pub notes: String,
    pub rest_seconds_after: Option<i64>,
    pub sets: Vec<RoutineSet>,
}

pub fn build_exercises(matched: Vec<MatchedExercise>) -> Vec<RoutineExercise> {
    matched
        .into_iter()
        .map(|m| RoutineExercise {
            exercise_template_id: m.hevy_template_id,
            superset_id: None,
            rest_seconds: m.rest_seconds_after,
            notes: m.notes,
            sets: m.sets,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_exercise_with_rest_seconds_and_sets() {
        let matched = vec![MatchedExercise {
            hevy_template_id: "PUSH-PRESS-ID".to_string(),
            notes: "2 reps @ 65%".to_string(),
            rest_seconds_after: Some(120),
            sets: vec![RoutineSet::normal(), RoutineSet::normal()],
        }];
        let built = build_exercises(matched);
        assert_eq!(built.len(), 1);
        assert_eq!(built[0].exercise_template_id, "PUSH-PRESS-ID");
        assert_eq!(built[0].rest_seconds, Some(120));
        assert_eq!(built[0].superset_id, None);
        assert_eq!(built[0].sets.len(), 2);
    }
}
