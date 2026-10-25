// SPDX-License-Identifier: AGPL-3.0-or-later
//
// SugarWOD to Hevy: turns a gym's daily Workout of the Day into a ready-to-run
// Hevy routine, with loads filled in from your own lift history.
// Copyright (C) 2026 Eamun Rahimi
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use lambda_runtime::{service_fn, Error, LambdaEvent};
use serde_json::Value;
use swh_core::hevy::HevyClient;
use swh_core::matching::resolver::CatalogStore; // brings `find_by_id` into scope
use swh_core::secrets::fetch_hevy_api_key;
use swh_core::store::alias_table::DynamoAliasStore;
use swh_core::store::catalog_table::DynamoCatalogStore;
use swh_core::store::personal_max::DynamoMaxStore;

async fn handler(_event: LambdaEvent<Value>) -> Result<Value, Error> {
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let secrets_client = aws_sdk_secretsmanager::Client::new(&config);
    let dynamo_client = aws_sdk_dynamodb::Client::new(&config);
    let http = reqwest::Client::new();

    let hevy_api_key = fetch_hevy_api_key(&secrets_client).await?;
    let hevy = HevyClient::new(http, hevy_api_key);

    let aliases = DynamoAliasStore::new(dynamo_client.clone(), env_var("ALIAS_TABLE_NAME")?);
    let catalog = DynamoCatalogStore::new(dynamo_client.clone(), env_var("CATALOG_TABLE_NAME")?);
    let maxes = DynamoMaxStore::new(dynamo_client, env_var("PERSONAL_MAX_TABLE_NAME")?);

    // Bounded to exercises actually referenced by ExerciseAlias, not the whole ~1090-exercise
    // catalog — see the plan's "Personal max derivation" section for why this is both cheaper
    // and more accurate than paginating the user's full workout history.
    let template_ids = aliases.distinct_template_ids().await?;
    let mut updated = 0usize;

    for template_id in &template_ids {
        let history = hevy.fetch_exercise_history(template_id).await?;
        let weights: Vec<f64> = history
            .exercise_history
            .iter()
            .filter_map(|e| e.weight_kg)
            .filter(|w| *w > 0.0)
            .collect();

        let Some(max_kg) = credible_max_kg(&weights) else {
            continue;
        };

        let title = catalog
            .find_by_id(template_id)
            .await?
            .map(|c| c.title)
            .unwrap_or_else(|| template_id.clone());

        if !max_kg.discarded.is_empty() {
            tracing::warn!(
                title = %title,
                template_id = %template_id,
                accepted_kg = max_kg.max_kg,
                discarded_kg = ?max_kg.discarded,
                "discarded implausible logged weight(s) when deriving a personal max; a single \
                 mis-typed set in Hevy would otherwise become the permanent 1RM and drive every \
                 unlabeled prescription for this exercise"
            );
        }
        let max_kg = max_kg.max_kg;

        let source_workout_id = history
            .exercise_history
            .iter()
            .find(|e| e.weight_kg == Some(max_kg))
            .map(|e| e.workout_id.clone())
            .unwrap_or_default();

        maxes
            .upsert(template_id, &title, max_kg, &source_workout_id)
            .await?;
        updated += 1;
    }

    tracing::info!(
        candidates = template_ids.len(),
        updated,
        "refreshed personal maxes"
    );
    Ok(serde_json::json!({ "status": "ok", "candidates": template_ids.len(), "updated": updated }))
}

/// A personal max that survived outlier rejection, plus whatever was thrown away getting there.
struct CredibleMax {
    max_kg: f64,
    /// Values rejected as mis-logged, highest first — logged so a real PR that trips the
    /// heuristic is visible rather than silently ignored.
    discarded: Vec<f64>,
}

/// How many times the next-heaviest logged weight a value may be before it is treated as a
/// typo rather than a lift.
///
/// Compared against the *next value down* rather than the mean or median, because a lifter who
/// is progressing has a legitimately right-skewed history: real `Ball Slams` history is 3.3x its
/// own median and real `Deadlift (Barbell)` is 1.7x, so a median-based bound rejects real PRs.
/// Against the next value down, every one of the 52 real exercises in `PersonalMax` sits at
/// 1.41x or below — while the one actual typo (a 161 kg `DB Thruster` set logged between 15.9 kg
/// sets of the same workout) sits at 8.9x. 2.5 leaves a wide margin on both sides.
const OUTLIER_RATIO: f64 = 2.5;

/// Minimum distinct logged weights before outlier rejection is attempted at all. With one or two
/// data points there is no "next value down" to judge against, and a genuine heavy single would
/// be indistinguishable from a typo — so the raw max is taken and left to the athlete's eye.
const MIN_SAMPLES_FOR_OUTLIER_REJECTION: usize = 3;

/// Picks the heaviest *credible* weight from an exercise's logged history.
///
/// Hevy's history is hand-entered, so one fat-fingered set becomes a permanent 1RM under a plain
/// `max()` — and because an unlabeled loaded movement is prescribed at
/// `DEFAULT_UNLABELED_PERCENT` of that max, the typo silently propagates into every future
/// routine (the 2026-09-11 `DB Thruster` case: one 161 kg set produced 180 lb thrusters in a
/// synced HYROX routine, reported as a clean success). This walks down from the top and discards
/// a value only while it towers over the next one down, so a lone typo is dropped while a real
/// PR — which sits close to the athlete's next-best lift — is kept.
fn credible_max_kg(weights: &[f64]) -> Option<CredibleMax> {
    let mut sorted: Vec<f64> = weights.iter().copied().filter(|w| *w > 0.0).collect();
    if sorted.is_empty() {
        return None;
    }
    // Descending. `total_cmp` rather than `partial_cmp().unwrap()` so a NaN in the feed sorts
    // instead of panicking.
    sorted.sort_by(|a, b| b.total_cmp(a));

    if sorted.len() < MIN_SAMPLES_FOR_OUTLIER_REJECTION {
        return Some(CredibleMax {
            max_kg: sorted[0],
            discarded: Vec::new(),
        });
    }

    let mut discarded = Vec::new();
    for (i, &candidate) in sorted.iter().enumerate() {
        // Stop at the last two: below this there is no longer a population to judge against.
        let Some(&next) = sorted.get(i + 1) else {
            break;
        };
        if sorted.len() - i < MIN_SAMPLES_FOR_OUTLIER_REJECTION {
            break;
        }
        if candidate > next * OUTLIER_RATIO {
            discarded.push(candidate);
            continue;
        }
        return Some(CredibleMax {
            max_kg: candidate,
            discarded,
        });
    }

    // Everything looked like an outlier relative to the next value (a history that decays by
    // more than the ratio at every step). Fall back to the raw max and discard nothing rather
    // than invent a number.
    Some(CredibleMax {
        max_kg: sorted[0],
        discarded: Vec::new(),
    })
}

fn env_var(name: &str) -> Result<String, Error> {
    std::env::var(name).map_err(|_| format!("missing required env var {name}").into())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        // Default to `info` rather than `EnvFilter`'s built-in `error`. With the built-in
        // default, an unset RUST_LOG silently discarded every `warn!` — including the ones that
        // report a broken Discord webhook, which is exactly the failure they exist to surface.
        // Still overridable: set RUST_LOG on the function to raise or lower this.
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .without_time()
        .init();
    lambda_runtime::run(service_fn(handler)).await
}

/// Outlier-rejection tests, driven by weights actually logged in the Hevy account this syncs
/// into rather than invented numbers — the whole point of the heuristic is to separate a real
/// right-skewed lifting history from a typo, and only real histories exercise that.
#[cfg(test)]
mod tests {
    use super::*;

    fn max_of(weights: &[f64]) -> f64 {
        credible_max_kg(weights).unwrap().max_kg
    }

    #[test]
    fn rejects_the_real_db_thruster_typo() {
        // Real history, 2026-09-12: 43 sets between 6.8 and 18.1 kg plus one 161.03 kg set
        // logged between 15.88 kg sets of the same workout. A plain max() made that the 1RM,
        // and the 50% unlabeled default then prescribed 180 lb thrusters into a live routine.
        let mut history = vec![161.02548, 18.14371637742559, 15.8757515, 15.8757515];
        history.extend(std::iter::repeat_n(13.607787283069193, 9));
        history.push(6.8038936415345965);

        let result = credible_max_kg(&history).unwrap();
        assert_eq!(result.discarded, vec![161.02548]);
        assert!(
            (result.max_kg - 18.14371637742559).abs() < 1e-9,
            "expected the next real set (18.1 kg), got {}",
            result.max_kg
        );
    }

    #[test]
    fn keeps_a_real_barbell_deadlift_max() {
        // Real history: a genuine 161 kg top set sits just above a 156.5 kg second — 1.03x, so
        // it must survive. This is the case a naive "reject anything far above the median"
        // rule gets wrong, since the median here is only 93 kg.
        let history = vec![161.02548, 156.48945, 93.0, 93.0, 88.45045, 70.30682];
        let result = credible_max_kg(&history).unwrap();
        assert!(result.discarded.is_empty());
        assert_eq!(result.max_kg, 161.02548);
    }

    #[test]
    fn keeps_a_real_ball_slams_history_that_is_3x_its_own_median() {
        // Real history: max 22.7 kg against a 6.8 kg median (3.34x the median) but a 22.7 kg
        // second value (1.00x). Rejecting on median distance would wrongly discard this.
        let history = vec![22.68, 22.68, 13.6, 6.8, 6.8, 6.8];
        let result = credible_max_kg(&history).unwrap();
        assert!(result.discarded.is_empty());
        assert_eq!(result.max_kg, 22.68);
    }

    #[test]
    fn keeps_a_real_dumbbell_rdl_double_that_is_a_plausible_progression() {
        // Real history: 13.6 kg over a 6.8 kg second — exactly 2.0x, the highest ratio of any
        // real exercise in the account. 30 lb after 15 lb is a normal jump, so the 2.5 ratio
        // must leave it alone.
        let history = vec![13.607787283069193, 6.8038936415345965, 6.8038936415345965];
        let result = credible_max_kg(&history).unwrap();
        assert!(result.discarded.is_empty());
        assert_eq!(result.max_kg, 13.607787283069193);
    }

    #[test]
    fn no_logged_weight_yields_no_max() {
        assert!(credible_max_kg(&[]).is_none());
        // Bodyweight/unloaded movements come back as zeros, not absent entries.
        assert!(credible_max_kg(&[0.0, 0.0]).is_none());
    }

    #[test]
    fn too_few_samples_takes_the_raw_max_rather_than_guessing() {
        // One or two sets ever: a heavy single is indistinguishable from a typo, so don't try.
        assert_eq!(max_of(&[100.0]), 100.0);
        assert_eq!(max_of(&[100.0, 5.0]), 100.0);
    }

    #[test]
    fn two_stacked_typos_are_both_discarded() {
        let result = credible_max_kg(&[500.0, 180.0, 20.0, 20.0, 18.0]).unwrap();
        assert_eq!(result.discarded, vec![500.0, 180.0]);
        assert_eq!(result.max_kg, 20.0);
    }

    #[test]
    fn a_history_that_decays_at_every_step_falls_back_to_the_raw_max() {
        // Nothing to anchor against — every value towers over the next. Better to keep the raw
        // max than to walk all the way down to the lightest warm-up set.
        let result = credible_max_kg(&[100.0, 10.0, 1.0]).unwrap();
        assert!(result.discarded.is_empty());
        assert_eq!(result.max_kg, 100.0);
    }

    #[test]
    fn a_nan_in_the_feed_does_not_panic() {
        let result = credible_max_kg(&[f64::NAN, 20.0, 20.0, 18.0]).unwrap();
        assert!(result.max_kg.is_finite());
    }
}
