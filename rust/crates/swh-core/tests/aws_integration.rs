// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

//! Integration tests against REAL AWS resources (a deployed gamma stack) and REAL external APIs
//! (SugarWOD, and Hevy's read-only GET endpoints). Ignored by default — run explicitly with:
//!
//!   cargo test --workspace -- --ignored --test-threads=1
//!
//! after exporting the env vars printed by `cdk/scripts/run-integration-tests.sh` (same env var
//! names the deployed Lambda binaries read, sourced from the gamma stack's CfnOutputs). Serial
//! (`--test-threads=1`) because several tests write real rows to shared gamma tables.
//!
//! Deliberately does NOT call Hevy's `POST`/`PUT /v1/routines` — every test here is either
//! AWS-account-local or a safe, side-effect-free GET, per the "mocked Hevy writes" decision.
//! Routine create/update against the real Hevy account is verified manually instead, against
//! `test-`-prefixed `RoutineMap` keys.

use aws_sdk_dynamodb::Client as DynamoClient;
use std::time::{SystemTime, UNIX_EPOCH};
use swh_core::date::today_yyyymmdd_in;
use swh_core::hevy::model::ExerciseTemplate;
use swh_core::hevy::HevyClient;
use swh_core::matching::load_calc::MaxStore;
use swh_core::matching::resolver::{AliasEntry, AliasStore, CatalogStore};
use swh_core::secrets::fetch_hevy_api_key;
use swh_core::store::alias_table::DynamoAliasStore;
use swh_core::store::catalog_table::DynamoCatalogStore;
use swh_core::store::personal_max::DynamoMaxStore;
use swh_core::store::routine_map::RoutineMapStore;
use swh_core::sugarwod::SugarWodClient;

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!(
            "integration test requires env var {name} — see cdk/scripts/run-integration-tests.sh"
        )
    })
}

/// Cheap collision-avoidance for test rows written to shared gamma tables — not cryptographic,
/// just needs to not collide with itself across repeated local runs.
fn unique_suffix() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .to_string()
}

async fn dynamo_client() -> DynamoClient {
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    DynamoClient::new(&config)
}

async fn hevy_client() -> HevyClient {
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let secrets_client = aws_sdk_secretsmanager::Client::new(&config);
    let api_key = fetch_hevy_api_key(&secrets_client)
        .await
        .expect("fetch_hevy_api_key against the real Secrets Manager secret");
    HevyClient::new(reqwest::Client::new(), api_key)
}

#[tokio::test]
#[ignore]
async fn exercise_alias_store_roundtrip_and_distinct_ids() {
    let store = DynamoAliasStore::new(dynamo_client().await, env("ALIAS_TABLE_NAME"));
    let alias = format!("integration-test-alias-{}", unique_suffix());
    let template_id = format!("INTEGRATION-TEST-ID-{}", unique_suffix());

    store
        .put(
            &alias,
            AliasEntry {
                canonical_title: "Integration Test Exercise".to_string(),
                hevy_template_id: template_id.clone(),
            },
            "manual",
        )
        .await
        .expect("PutItem against real ExerciseAlias table");

    let fetched = store
        .get(&alias)
        .await
        .expect("GetItem against real ExerciseAlias table")
        .expect("just-written alias should be immediately readable");
    assert_eq!(fetched.hevy_template_id, template_id);
    assert_eq!(fetched.canonical_title, "Integration Test Exercise");

    let ids = store
        .distinct_template_ids()
        .await
        .expect("Scan against real ExerciseAlias table");
    assert!(
        ids.contains(&template_id),
        "distinct_template_ids should include the row just written"
    );
}

#[tokio::test]
#[ignore]
async fn catalog_store_batch_upsert_and_lookup() {
    let store = DynamoCatalogStore::new(dynamo_client().await, env("CATALOG_TABLE_NAME"));
    let id = format!("integration-test-{}", unique_suffix());
    let title = format!("Integration Test Squat {}", unique_suffix());

    let template = ExerciseTemplate {
        id: id.clone(),
        title: title.clone(),
        kind: "weight_reps".to_string(),
        primary_muscle_group: "quadriceps".to_string(),
        secondary_muscle_groups: vec![],
        equipment: "barbell".to_string(),
        is_custom: true,
    };

    store
        .batch_upsert(std::slice::from_ref(&template))
        .await
        .expect("BatchWriteItem against real HevyExerciseCatalog table");

    let by_id = store
        .find_by_id(&id)
        .await
        .expect("GetItem against real HevyExerciseCatalog table")
        .expect("just-written template should be immediately readable by id");
    assert_eq!(by_id.title, title);
    assert!(by_id.is_loaded_type, "weight_reps should be a loaded type");

    let normalized = swh_core::matching::normalize::normalize(&title);
    let by_title = store
        .find_by_normalized_title(&normalized)
        .await
        .expect("Query against the real TitleNormalizedIndex GSI")
        .expect("just-written template should be findable via the GSI");
    assert_eq!(by_title.hevy_template_id, id);
}

#[tokio::test]
#[ignore]
async fn personal_max_store_roundtrip() {
    let store = DynamoMaxStore::new(dynamo_client().await, env("PERSONAL_MAX_TABLE_NAME"));
    let id = format!("integration-test-max-{}", unique_suffix());

    store
        .upsert(&id, "Integration Test Lift", 100.0, "test-workout-id")
        .await
        .expect("PutItem against real PersonalMax table");

    let max_kg = store
        .get_max_kg(&id)
        .await
        .expect("GetItem against real PersonalMax table")
        .expect("just-written max should be immediately readable");
    assert!((max_kg - 100.0).abs() < 1e-9);
}

#[tokio::test]
#[ignore]
async fn routine_map_store_roundtrip() {
    let store = RoutineMapStore::new(dynamo_client().await, env("ROUTINE_MAP_TABLE_NAME"));
    let key = format!("integration-test-routine-{}", unique_suffix());

    store
        .upsert(&key, "test-routine-id-123", "Integration Test Routine")
        .await
        .expect("PutItem against real RoutineMap table");

    let id = store
        .get_routine_id(&key)
        .await
        .expect("GetItem against real RoutineMap table")
        .expect("just-written routine mapping should be immediately readable");
    assert_eq!(id, "test-routine-id-123");
}

#[tokio::test]
#[ignore]
async fn secrets_manager_fetch_hevy_api_key_returns_nonempty_key() {
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let client = aws_sdk_secretsmanager::Client::new(&config);
    let key = fetch_hevy_api_key(&client).await.expect(
        "fetch_hevy_api_key against the secret named by HEVY_SECRET_ID \
             (a CfnOutput of the data stack; see run-integration-tests.sh)",
    );
    assert!(!key.is_empty(), "Hevy API key should not be empty");
}

#[tokio::test]
#[ignore]
async fn ssm_fetch_discord_webhook_url_returns_a_url() {
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let client = aws_sdk_ssm::Client::new(&config);
    // Discord notifications are off by default, in which case the deployment has no webhook
    // parameter and the test role has no `ssm:GetParameter` grant. Nothing to verify, so skip
    // rather than fail — this is the default configuration, not a broken one.
    let Some(param_name) = swh_core::config::discord_webhook_param() else {
        eprintln!(
            "skipped: DISCORD_WEBHOOK_PARAM_NAME unset (notifications disabled). Deploy with \
             -c discordEnabled=true and re-run to exercise this path."
        );
        return;
    };
    let url = swh_core::discord::fetch_webhook_url(&client, &param_name)
        .await
        .expect(
            "fetch_webhook_url against the real gamma SSM parameter — has it been created yet? \
             aws ssm put-parameter --name <param> --type SecureString --value <url>",
        );
    assert!(
        url.starts_with("https://"),
        "webhook URL should be an https URL"
    );
}

#[tokio::test]
#[ignore]
async fn sugarwod_live_fetch_returns_todays_workouts() {
    let client = SugarWodClient::new(reqwest::Client::new());
    let tz = std::str::FromStr::from_str(&env("LOCAL_TIMEZONE")).expect("valid IANA timezone");
    let date = today_yyyymmdd_in(tz);
    let response = client
        .fetch_workouts(&env("SUGARWOD_AFFILIATE_ID"), &date)
        .await
        .expect("live SugarWOD fetch (public, unauthenticated, no side effects)");
    assert!(response.success);
    // SugarWOD may legitimately have zero workouts posted for a given date (e.g. a rest day
    // isn't always programmed) — only assert the call succeeded and parsed, not that `data` is
    // non-empty.
}

#[tokio::test]
#[ignore]
async fn hevy_live_get_only_calls_succeed() {
    let hevy = hevy_client().await;

    // GET /v1/exercise_templates — read-only, safe.
    let templates = hevy
        .fetch_all_exercise_templates()
        .await
        .expect("live Hevy GET /v1/exercise_templates");
    assert!(
        !templates.is_empty(),
        "Hevy's catalog should have at least one template"
    );

    // GET /v1/exercise_history/{id} — read-only, safe. Uses the first real template id from
    // the call above so this test is self-contained (no dependency on other tests' ordering).
    let first_id = &templates[0].id;
    let history = hevy
        .fetch_exercise_history(first_id)
        .await
        .expect("live Hevy GET /v1/exercise_history/{id}");
    // History may legitimately be empty if that exercise was never logged — only assert the
    // call succeeded and parsed.
    let _ = history.exercise_history.len();
}
