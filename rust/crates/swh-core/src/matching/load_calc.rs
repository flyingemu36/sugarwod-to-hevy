// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

use super::{Resolution, SetSpec};
use crate::error::Result;
use crate::hevy::model::{RoutineSet, SupportedSetFields};
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
    /// Prescribed set fields (`"weight"`, `"reps"`, `"distance"`, `"duration"`) that the resolved
    /// template's Hevy `type` has no slot for, in that fixed order, each listed once. Hevy accepts
    /// such a routine without error and silently discards the field, so without this a
    /// "1000 m SKI" resolved to the `duration`-only `Skiing` template syncs as an empty set and is
    /// reported as a success. The sets themselves are left untouched — this reports the loss, it
    /// doesn't paper over it.
    pub unsupported_fields: Vec<&'static str>,
}

/// Turns parsed `SetSpec`s into final Hevy `RoutineSet`s, applying %1RM weight math where
/// needed. `resolution` must already be `Resolution::Resolved` — callers filter out
/// `Skip`/`Unmapped` before reaching this step.
pub async fn compute_sets(
    set_specs: &[SetSpec],
    resolution: &Resolution,
    maxes: &impl MaxStore,
) -> Result<LoadCalcResult> {
    let (hevy_template_id, is_loaded_type, kind) = match resolution {
        Resolution::Resolved {
            hevy_template_id,
            is_loaded_type,
            kind,
        } => (hevy_template_id.as_str(), *is_loaded_type, kind.as_str()),
        _ => {
            return Ok(LoadCalcResult {
                sets: vec![],
                load_unknown: false,
                unsupported_fields: vec![],
            })
        }
    };

    // Permissive for a type we don't recognize, so an unknown type is never reported as lossy.
    let supported = SupportedSetFields::for_kind_or_permissive(kind);
    // Strict: only a type *known* to pair a weight with a distance (a loaded carry or sled) lets
    // a distance coexist with the unlabeled-load default. Unknown types keep the old behavior of a
    // distance blocking the default load.
    let known = SupportedSetFields::for_kind(kind);
    let carries_weight_over_distance = known.weight && known.distance;

    // Only fetched once per exercise, reused across all of its sets.
    let max_kg = maxes.get_max_kg(hevy_template_id).await?;

    let mut load_unknown = false;
    let mut out_sets = Vec::with_capacity(set_specs.len());
    let mut lost = SupportedSetFields {
        weight: false,
        reps: false,
        distance: false,
        duration: false,
    };

    for spec in set_specs {
        let mut routine_set = RoutineSet::normal();
        routine_set.reps = spec.reps;
        routine_set.distance_meters = spec.distance_meters;
        routine_set.duration_seconds = spec.duration_seconds;

        if let Some(explicit_lb) = spec.explicit_weight_lb {
            // A literal Rx weight from the description text (e.g. "(95/65)") — used as-is,
            // not rounded (rounding is only for %1RM-*computed* weight, decision #7).
            routine_set.weight_kg = Some(lb_to_kg(explicit_lb));
            record_unsupported(&routine_set, &supported, &mut lost);
            out_sets.push(routine_set);
            continue;
        }

        // A distance or duration normally marks a cardio prescription that must not pick up a
        // load (a "20 Cal Row" is a flat 60s/0m set). The exception is a type built to hold a
        // weight *and* a distance — "200 m Farmer carry" on `Farmers Walk`
        // (`short_distance_weight`) — where the distance is part of a loaded carry.
        let distance_blocks_load = spec.distance_meters.is_some() && !carries_weight_over_distance;
        let needs_default_load =
            is_loaded_type && !distance_blocks_load && spec.duration_seconds.is_none();

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

        record_unsupported(&routine_set, &supported, &mut lost);
        out_sets.push(routine_set);
    }

    let unsupported_fields = [
        (lost.weight, "weight"),
        (lost.reps, "reps"),
        (lost.distance, "distance"),
        (lost.duration, "duration"),
    ]
    .into_iter()
    .filter_map(|(was_lost, name)| was_lost.then_some(name))
    .collect();

    Ok(LoadCalcResult {
        sets: out_sets,
        load_unknown,
        unsupported_fields,
    })
}

/// Marks in `lost` every field this set prescribes that `supported` has no slot for. A zero
/// distance doesn't count as prescribed: calorie targets are encoded as a flat 60s/0m set, and
/// that `0` placeholder carries no information to lose.
fn record_unsupported(
    set: &RoutineSet,
    supported: &SupportedSetFields,
    lost: &mut SupportedSetFields,
) {
    lost.weight |= set.weight_kg.is_some() && !supported.weight;
    lost.reps |= set.reps.is_some() && !supported.reps;
    lost.distance |= set.distance_meters.is_some_and(|d| d > 0) && !supported.distance;
    lost.duration |= set.duration_seconds.is_some() && !supported.duration;
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

    /// A resolution with no known Hevy type — permissive, so these tests pin the load math
    /// exactly as it behaved before type-aware field checks existed.
    fn resolved(id: &str, is_loaded_type: bool) -> Resolution {
        resolved_kind(id, is_loaded_type, "")
    }

    fn resolved_kind(id: &str, is_loaded_type: bool, kind: &str) -> Resolution {
        Resolution::Resolved {
            hevy_template_id: id.to_string(),
            is_loaded_type,
            kind: kind.to_string(),
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

    #[tokio::test]
    async fn distance_on_a_duration_only_template_is_reported_not_silent() {
        // Real 2026-09-12 HYROX: "1000 m SKI" resolved to Skiing (`duration`). Hevy accepted the
        // routine and discarded the distance, so it synced as an empty set under a clean "✅".
        let maxes = InMemoryMaxStore::default();
        let specs = vec![SetSpec {
            distance_meters: Some(1000),
            ..Default::default()
        }];
        let res = resolved_kind("SKI-ID", false, "duration");
        let result = compute_sets(&specs, &res, &maxes).await.unwrap();
        assert_eq!(result.unsupported_fields, vec!["distance"]);
        // Reported, not rewritten: the payload is left exactly as parsed.
        assert_eq!(result.sets[0].distance_meters, Some(1000));
    }

    #[tokio::test]
    async fn each_lost_field_is_listed_once_across_many_sets() {
        let maxes = InMemoryMaxStore::default();
        let spec = SetSpec {
            distance_meters: Some(50),
            ..Default::default()
        };
        let res = resolved_kind("SLED-ID", false, "reps_only");
        let result = compute_sets(&vec![spec; 5], &res, &maxes).await.unwrap();
        assert_eq!(result.unsupported_fields, vec!["distance"]);
    }

    #[tokio::test]
    async fn a_matching_type_reports_nothing() {
        let maxes = InMemoryMaxStore::default();
        let specs = vec![SetSpec {
            distance_meters: Some(1000),
            ..Default::default()
        }];
        let res = resolved_kind("RUN-ID", false, "distance_duration");
        let result = compute_sets(&specs, &res, &maxes).await.unwrap();
        assert!(result.unsupported_fields.is_empty());
    }

    #[tokio::test]
    async fn calorie_placeholder_zero_distance_is_not_reported_as_lost() {
        // "20 Cal Bike" is encoded as 60s/0m; the 0 is a placeholder, not a prescription.
        let maxes = InMemoryMaxStore::default();
        let specs = vec![SetSpec {
            duration_seconds: Some(60),
            distance_meters: Some(0),
            ..Default::default()
        }];
        let res = resolved_kind("BIKE-ID", false, "duration");
        let result = compute_sets(&specs, &res, &maxes).await.unwrap();
        assert!(result.unsupported_fields.is_empty());
    }

    #[tokio::test]
    async fn loaded_carry_over_a_distance_now_gets_its_default_load() {
        // Farmers Walk is `short_distance_weight` with a real 45.4 kg (100 lb) PersonalMax. The
        // loaded-type check used to test for nonexistent type names, and a distance always
        // blocked the default load, so "200 m Farmer carry" never got a weight.
        let maxes = InMemoryMaxStore::default();
        maxes
            .maxes_kg
            .lock()
            .unwrap()
            .insert("FW-ID".to_string(), lb_to_kg(100.0));
        let specs = vec![SetSpec {
            distance_meters: Some(200),
            ..Default::default()
        }];
        let kind = "short_distance_weight";
        let res = resolved_kind(
            "FW-ID",
            crate::hevy::model::is_loaded_exercise_type(kind),
            kind,
        );
        let result = compute_sets(&specs, &res, &maxes).await.unwrap();
        let lb = crate::units::kg_to_lb(result.sets[0].weight_kg.expect("carry should be loaded"));
        assert!((lb - 50.0).abs() < 1e-6, "got {lb}");
        assert!(result.unsupported_fields.is_empty());
    }

    #[tokio::test]
    async fn distance_on_a_plain_loaded_type_still_blocks_the_default_load() {
        // "150m Dual Dumbbell Farmers Carry" aliased onto a `weight_reps` template must not start
        // prescribing 50% of that lift's max just because the carry fix exists.
        let maxes = InMemoryMaxStore::default();
        maxes
            .maxes_kg
            .lock()
            .unwrap()
            .insert("DBROW-ID".to_string(), lb_to_kg(65.0));
        let specs = vec![SetSpec {
            distance_meters: Some(150),
            ..Default::default()
        }];
        let res = resolved_kind("DBROW-ID", true, "weight_reps");
        let result = compute_sets(&specs, &res, &maxes).await.unwrap();
        assert_eq!(result.sets[0].weight_kg, None);
        assert_eq!(result.unsupported_fields, vec!["distance"]);
    }

    #[test]
    fn loaded_type_names_are_the_ones_hevy_actually_emits() {
        use crate::hevy::model::is_loaded_exercise_type;
        assert!(is_loaded_exercise_type("weight_reps"));
        assert!(is_loaded_exercise_type("short_distance_weight"));
        assert!(is_loaded_exercise_type("bodyweight_weighted"));
        // Assistance weight takes load *off*; a % of max would prescribe the opposite.
        assert!(!is_loaded_exercise_type("bodyweight_assisted"));
        assert!(!is_loaded_exercise_type("reps_only"));
        assert!(!is_loaded_exercise_type("duration"));
    }
}
