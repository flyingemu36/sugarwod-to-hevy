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
        let max_kg = history
            .exercise_history
            .iter()
            .filter_map(|e| e.weight_kg)
            .fold(0.0_f64, f64::max);

        if max_kg <= 0.0 {
            continue;
        }

        let title = catalog
            .find_by_id(template_id)
            .await?
            .map(|c| c.title)
            .unwrap_or_else(|| template_id.clone());

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
