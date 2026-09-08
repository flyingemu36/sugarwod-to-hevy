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
use swh_core::config::{affiliate_id, discord_webhook_param, local_timezone};
use swh_core::date::today_yyyymmdd_in;
use swh_core::discord::{build_failure_message, build_summary_message, WorkoutOutcome};
use swh_core::hevy::routine_builder::build_exercises;
use swh_core::hevy::HevyClient;
use swh_core::matching::build_routine_exercises;
use swh_core::secrets::fetch_hevy_api_key;
use swh_core::store::alias_table::DynamoAliasStore;
use swh_core::store::catalog_table::DynamoCatalogStore;
use swh_core::store::personal_max::DynamoMaxStore;
use swh_core::store::routine_map::{normalize_routine_key, RoutineMapStore};
use swh_core::sugarwod::SugarWodClient;

/// Posts `message` to the Discord webhook, if notifications are enabled.
///
/// Two distinct non-events, deliberately logged differently:
///
/// - **Disabled** (no `DISCORD_WEBHOOK_PARAM_NAME`) is the default and is not a problem. Logged
///   at debug, because warning about a feature nobody turned on trains people to ignore warnings.
/// - **Enabled but broken** — parameter missing, or Discord rejecting the post — *is* a problem
///   and warns with the reason. An earlier version discarded both cases with `if let Ok(..)` and
///   `let _ = ..`, so a missing parameter produced no Discord message and no log line either:
///   the summary simply never arrived, with nothing to find in CloudWatch.
///
/// Either way this never fails the invocation. A broken notification must not fail a sync that
/// already wrote the routine to Hevy.
async fn notify(ssm_client: &aws_sdk_ssm::Client, http: &reqwest::Client, message: &str) {
    let Some(param) = discord_webhook_param() else {
        tracing::debug!("discord notifications are disabled; skipping summary post");
        return;
    };
    match swh_core::discord::fetch_webhook_url(ssm_client, &param).await {
        Ok(webhook_url) => {
            if let Err(e) = swh_core::discord::post_message(http, &webhook_url, message).await {
                tracing::warn!("discord post to the webhook failed: {e}");
            }
        }
        Err(e) => {
            tracing::warn!(
                "discord notifications are enabled but the webhook URL could not be read from \
                 SSM parameter {param}: {e} (create it with `aws ssm put-parameter --name \
                 {param} --type SecureString --value <url>`)"
            );
        }
    }
}

async fn handler(_event: LambdaEvent<Value>) -> Result<Value, Error> {
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let ssm_client = aws_sdk_ssm::Client::new(&config);
    let http = reqwest::Client::new();

    match run_sync(&config, &http).await {
        Ok(summary) => {
            notify(&ssm_client, &http, &summary).await;
            Ok(serde_json::json!({ "status": "ok", "summary": summary }))
        }
        Err(e) => {
            let msg = build_failure_message("sync-wod", &e.to_string());
            notify(&ssm_client, &http, &msg).await;
            Err(e)
        }
    }
}

async fn run_sync(config: &aws_config::SdkConfig, http: &reqwest::Client) -> Result<String, Error> {
    let secrets_client = aws_sdk_secretsmanager::Client::new(config);
    let dynamo_client = aws_sdk_dynamodb::Client::new(config);

    let hevy_api_key = fetch_hevy_api_key(&secrets_client).await?;
    let hevy = HevyClient::new(http.clone(), hevy_api_key);
    let sugarwod = SugarWodClient::new(http.clone());

    let aliases = DynamoAliasStore::new(dynamo_client.clone(), env_var("ALIAS_TABLE_NAME")?);
    let catalog = DynamoCatalogStore::new(dynamo_client.clone(), env_var("CATALOG_TABLE_NAME")?);
    let maxes = DynamoMaxStore::new(dynamo_client.clone(), env_var("PERSONAL_MAX_TABLE_NAME")?);
    let routine_map =
        RoutineMapStore::new(dynamo_client.clone(), env_var("ROUTINE_MAP_TABLE_NAME")?);

    let affiliate = affiliate_id()?;
    let date = today_yyyymmdd_in(local_timezone()?);

    let response = sugarwod.fetch_workouts(&affiliate, &date).await?;

    let mut outcomes = Vec::new();

    for workout in &response.data {
        let (matched, report) = build_routine_exercises(
            &workout.description,
            &workout.movements,
            &aliases,
            &catalog,
            &maxes,
        )
        .await?;

        let exercises = build_exercises(matched);
        let exercise_count = exercises.len();

        // Hevy rejects a routine with an empty `exercises` array (HTTP 400, "Array must contain
        // at least 1 element(s)"). If nothing resolved — a rest-only day, or a WOD whose
        // movements are all still unmapped — skip the Hevy write entirely and surface it in the
        // summary, instead of letting a single empty routine hard-fail the whole run (the
        // 2026-08-02 prod incident). Leave any existing RoutineMap/routine untouched.
        if exercises.is_empty() {
            outcomes.push(WorkoutOutcome {
                title: workout.title.clone(),
                exercise_count: 0,
                unmapped: report.unmapped,
                load_unknown: report.load_unknown,
            });
            continue;
        }

        let notes = workout.description.replace("\\n", "\n");
        let routine_key = normalize_routine_key(&workout.title);

        let existing_id = routine_map.get_routine_id(&routine_key).await?;

        let routine_id = match existing_id {
            Some(id) => {
                match hevy
                    .update_routine(&id, &workout.title, &notes, exercises.clone())
                    .await?
                {
                    Some(resp) => resp.id,
                    None => {
                        // Stored id 404'd (e.g. deleted manually in the Hevy app) — fall back
                        // to creating a fresh routine and overwrite the stored mapping.
                        let resp = hevy
                            .create_routine(&workout.title, &notes, exercises)
                            .await?;
                        resp.id
                    }
                }
            }
            None => {
                let resp = hevy
                    .create_routine(&workout.title, &notes, exercises)
                    .await?;
                resp.id
            }
        };

        routine_map
            .upsert(&routine_key, &routine_id, &workout.title)
            .await?;

        outcomes.push(WorkoutOutcome {
            title: workout.title.clone(),
            exercise_count,
            unmapped: report.unmapped,
            load_unknown: report.load_unknown,
        });
    }

    Ok(build_summary_message(&outcomes))
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
