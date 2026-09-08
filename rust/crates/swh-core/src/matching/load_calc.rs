// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

use super::{Resolution, SetSpec};
use crate::error::Result;
use crate::hevy::model::RoutineSet;
use crate::units::{kg_to_lb, lb_to_kg, round_to_nearest};
use async_trait::async_trait;

/// A workout may require some rounding, given what the 1RM may be. Round any %1RM-computed
/// weight to the nearest 5 lbs (e.g. 175lbs 1RM * 65% = 113.75lbs, so we'll want 115lbs).
const ROUNDING_INCREMENT_LB: f64 = 5.0;

/// Default for a loaded movement with no explicit `@N%` or weight is set to 50% of the 1 rep max.
const DEFAULT_UNLABELED_PERCENT: f64 = 0.50;

/// Abstraction over `PersonalMax` so `load_calc` can be unit-tested without a real DynamoDB
/// table.
#[async_trait]
pub trait MaxStore {
    async fn get_max_kg(&self, hevy_template_id: &str) -> Result<Option<f64>>;
}

pub struct LoadCalcResult {
    pub sets: Vec<RoutineSet>,
    /// True if at least one set needed a load (explicit `@%` or the loaded-type default) but
    /// `PersonalMax` had no entry for this exercise — surfaced in the Discord report.
    pub load_unknown: bool,
}

/// Turns parsed `SetSpec`s into final Hevy `RoutineSet`s, applying %1RM weight math where
/// needed. `resolution` must already be `Resolution::Resolved` — callers filter out
/// `Skip`/`Unmapped` before reaching this step.
pub async fn compute_sets(
    set_specs: &[SetSpec],
    resolution: &Resolution,
    maxes: &impl MaxStore,
) -> Result<LoadCalcResult> {
    let (hevy_template_id, is_loaded_type) = match resolution {
        Resolution::Resolved {
            hevy_template_id,
            is_loaded_type,
        } => (hevy_template_id.as_str(), *is_loaded_type),
        _ => {
            return Ok(LoadCalcResult {
                sets: vec![],
                load_unknown: false,
            })
        }
    };

    // Only fetched once per exercise, reused across all of its sets.
    let max_kg = maxes.get_max_kg(hevy_template_id).await?;

    let mut load_unknown = false;
    let mut out_sets = Vec::with_capacity(set_specs.len());

    for spec in set_specs {
        let mut routine_set = RoutineSet::normal();
        routine_set.reps = spec.reps;
        routine_set.distance_meters = spec.distance_meters;
        routine_set.duration_seconds = spec.duration_seconds;

        if let Some(explicit_lb) = spec.explicit_weight_lb {
            // A literal Rx weight from the description text (e.g. "(95/65)") — used as-is,
            // not rounded (rounding is only for %1RM-*computed* weight, decision #7).
            routine_set.weight_kg = Some(lb_to_kg(explicit_lb));
            out_sets.push(routine_set);
            continue;
        }

        let needs_default_load =
            is_loaded_type && spec.distance_meters.is_none() && spec.duration_seconds.is_none();

        if spec.percent_1rm.is_some() || needs_default_load {
            let percent = spec.percent_1rm.unwrap_or(DEFAULT_UNLABELED_PERCENT);
            match max_kg {
                Some(max_kg_value) => {
                    let target_lb =
                        round_to_nearest(kg_to_lb(max_kg_value) * percent, ROUNDING_INCREMENT_LB);
                    routine_set.weight_kg = Some(lb_to_kg(target_lb));
                }
                None => {
                    load_unknown = true;
                }
            }
        }

        out_sets.push(routine_set);
    }

    Ok(LoadCalcResult {
        sets: out_sets,
        load_unknown,
    })
}

#[cfg(test)]
pub mod test_doubles {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct InMemoryMaxStore {
        pub maxes_kg: Mutex<HashMap<String, f64>>,
    }

    #[async_trait]
    impl MaxStore for InMemoryMaxStore {
        async fn get_max_kg(&self, hevy_template_id: &str) -> Result<Option<f64>> {
            Ok(self.maxes_kg.lock().unwrap().get(hevy_template_id).copied())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_doubles::InMemoryMaxStore;
    use super::*;
    use crate::units::lb_to_kg;

    fn resolved(id: &str, is_loaded_type: bool) -> Resolution {
        Resolution::Resolved {
            hevy_template_id: id.to_string(),
            is_loaded_type,
        }
    }

    #[tokio::test]
    async fn explicit_percent_rounds_to_nearest_five_lb() {
        let maxes = InMemoryMaxStore::default();
        maxes
            .maxes_kg
            .lock()
            .unwrap()
            .insert("PUSH-PRESS-ID".to_string(), lb_to_kg(175.0));

        let specs = vec![SetSpec {
            reps: Some(2),
            percent_1rm: Some(0.65),
            ..Default::default()
        }];
        let res = resolved("PUSH-PRESS-ID", true);
        let result = compute_sets(&specs, &res, &maxes).await.unwrap();

        assert_eq!(result.sets.len(), 1);
        assert!(!result.load_unknown);
        let weight_lb = crate::units::kg_to_lb(result.sets[0].weight_kg.unwrap());
        assert!((weight_lb - 115.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn unlabeled_loaded_movement_defaults_to_fifty_percent() {
        let maxes = InMemoryMaxStore::default();
        maxes
            .maxes_kg
            .lock()
            .unwrap()
            .insert("SNATCH-ID".to_string(), lb_to_kg(60.0));

        let specs = vec![SetSpec {
            reps: Some(12),
            ..Default::default()
        }];
        let res = resolved("SNATCH-ID", true);
        let result = compute_sets(&specs, &res, &maxes).await.unwrap();

        let weight_lb = crate::units::kg_to_lb(result.sets[0].weight_kg.unwrap());
        assert!((weight_lb - 30.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn bodyweight_type_never_gets_a_weight() {
        let maxes = InMemoryMaxStore::default();
        maxes
            .maxes_kg
            .lock()
            .unwrap()
            .insert("PUSHUP-ID".to_string(), lb_to_kg(200.0)); // present but must be ignored

        let specs = vec![SetSpec {
            reps: Some(12),
            ..Default::default()
        }];
        let res = resolved("PUSHUP-ID", false); // reps_only, not a loaded type
        let result = compute_sets(&specs, &res, &maxes).await.unwrap();

        assert_eq!(result.sets[0].weight_kg, None);
        assert!(!result.load_unknown);
    }

    #[tokio::test]
    async fn missing_max_is_reported_as_load_unknown() {
        let maxes = InMemoryMaxStore::default(); // no entry for this id
        let specs = vec![SetSpec {
            reps: Some(12),
            ..Default::default()
        }];
        let res = resolved("NEW-MOVEMENT-ID", true);
        let result = compute_sets(&specs, &res, &maxes).await.unwrap();

        assert_eq!(result.sets[0].weight_kg, None);
        assert!(result.load_unknown);
    }

    #[tokio::test]
    async fn cal_based_duration_set_never_needs_a_load_even_if_loaded_type() {
        let maxes = InMemoryMaxStore::default();
        let specs = vec![SetSpec {
            duration_seconds: Some(60),
            distance_meters: Some(0),
            ..Default::default()
        }];
        // is_loaded_type true is nonsensical for a cardio movement in practice, but this proves
        // the distance/duration guard prevents a bogus default-load attempt regardless.
        let res = resolved("ROW-ID", true);
        let result = compute_sets(&specs, &res, &maxes).await.unwrap();

        assert_eq!(result.sets[0].weight_kg, None);
        assert!(!result.load_unknown);
    }
}
