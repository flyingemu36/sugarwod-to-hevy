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
use swh_core::secrets::fetch_hevy_api_key;
use swh_core::store::catalog_table::DynamoCatalogStore;

async fn handler(_event: LambdaEvent<Value>) -> Result<Value, Error> {
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let secrets_client = aws_sdk_secretsmanager::Client::new(&config);
    let dynamo_client = aws_sdk_dynamodb::Client::new(&config);
    let http = reqwest::Client::new();

    let hevy_api_key = fetch_hevy_api_key(&secrets_client).await?;
    let hevy = HevyClient::new(http, hevy_api_key);

    let table_name = std::env::var("CATALOG_TABLE_NAME")
        .map_err(|_| "missing required env var CATALOG_TABLE_NAME")?;
    let catalog = DynamoCatalogStore::new(dynamo_client, table_name);

    let templates = hevy.fetch_all_exercise_templates().await?;
    let count = templates.len();
    catalog.batch_upsert(&templates).await?;

    tracing::info!(count, "refreshed Hevy exercise template catalog");
    Ok(serde_json::json!({ "status": "ok", "count": count }))
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
