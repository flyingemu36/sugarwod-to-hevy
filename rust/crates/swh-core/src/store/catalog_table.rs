// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

use crate::error::{Result, SwhError};
use crate::hevy::model::{is_loaded_exercise_type, ExerciseTemplate};
use crate::matching::normalize::normalize;
use crate::matching::resolver::{CatalogEntry, CatalogStore};
use async_trait::async_trait;
use aws_sdk_dynamodb::types::{AttributeValue, PutRequest, WriteRequest};
use aws_sdk_dynamodb::Client;
use std::collections::HashMap;

const GSI_NAME: &str = "TitleNormalizedIndex";

pub struct DynamoCatalogStore {
    client: Client,
    table_name: String,
}

impl DynamoCatalogStore {
    pub fn new(client: Client, table_name: String) -> Self {
        Self { client, table_name }
    }

    /// Bulk-upserts the Hevy exercise-template catalog, 25 items per `BatchWriteItem` call
    /// (DynamoDB's per-call limit) — used by `refresh-catalog`. The catalog runs to roughly
    /// a thousand templates, so one PutItem a piece would be ~1000 calls.
    pub async fn batch_upsert(&self, templates: &[ExerciseTemplate]) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        for chunk in templates.chunks(25) {
            let requests: Vec<WriteRequest> = chunk
                .iter()
                .map(|t| {
                    let item: HashMap<String, AttributeValue> = HashMap::from([
                        ("id".to_string(), AttributeValue::S(t.id.clone())),
                        ("title".to_string(), AttributeValue::S(t.title.clone())),
                        (
                            "title_normalized".to_string(),
                            AttributeValue::S(normalize(&t.title)),
                        ),
                        ("type".to_string(), AttributeValue::S(t.kind.clone())),
                        (
                            "primary_muscle_group".to_string(),
                            AttributeValue::S(t.primary_muscle_group.clone()),
                        ),
                        (
                            "secondary_muscle_groups".to_string(),
                            AttributeValue::L(
                                t.secondary_muscle_groups
                                    .iter()
                                    .map(|s| AttributeValue::S(s.clone()))
                                    .collect(),
                            ),
                        ),
                        (
                            "equipment".to_string(),
                            AttributeValue::S(t.equipment.clone()),
                        ),
                        ("is_custom".to_string(), AttributeValue::Bool(t.is_custom)),
                        ("updated_at".to_string(), AttributeValue::S(now.clone())),
                    ]);
                    WriteRequest::builder()
                        .put_request(PutRequest::builder().set_item(Some(item)).build().unwrap())
                        .build()
                })
                .collect();

            self.client
                .batch_write_item()
                .request_items(&self.table_name, requests)
                .send()
                .await
                .map_err(SwhError::dynamo)?;
        }
        Ok(())
    }
}

#[async_trait]
impl CatalogStore for DynamoCatalogStore {
    /// Looks a template up by its normalized title.
    ///
    /// Distinct Hevy titles can normalize to the same key — the live catalog has 632 templates
    /// but only 626 distinct normalized titles, because punctuation and case are stripped
    /// ("Dumbbell Step-Up" vs "Dumbbell Step Up", "V-Up" vs "V Up", "Sled push" vs "Sled Push").
    /// An earlier version issued this query with `.limit(1)`, which let DynamoDB decide which of
    /// a colliding pair came back — so resolution was *nondeterministic across runs*. That is
    /// not cosmetic: the two "air bike" rows have types `weight_reps` and `duration`, so the
    /// coin flip decided whether a load was prescribed at all, and the two "dumbbell step up"
    /// rows carry different `PersonalMax` entries (20.4 kg vs 18.1 kg).
    ///
    /// So: fetch every match and pick deterministically, preferring the built-in template over a
    /// user-created one. Built-ins carry the more appropriate `type` in every colliding pair in
    /// the live catalog (an air bike really is a `duration` machine, a V-up really is
    /// `reps_only`), and a gym's genuinely custom movements — "DB Thruster", "Front Squat (DB)" —
    /// have no built-in namesake to lose to. `id` breaks any remaining tie so the choice is
    /// stable rather than merely arbitrary.
    async fn find_by_normalized_title(
        &self,
        title_normalized: &str,
    ) -> Result<Option<CatalogEntry>> {
        let resp = self
            .client
            .query()
            .table_name(&self.table_name)
            .index_name(GSI_NAME)
            .key_condition_expression("title_normalized = :t")
            .expression_attribute_values(":t", AttributeValue::S(title_normalized.to_string()))
            .send()
            .await
            .map_err(SwhError::dynamo)?;

        let mut items = resp.items.unwrap_or_default();
        items.sort_by(|a, b| {
            let custom = |i: &HashMap<String, AttributeValue>| {
                i.get("is_custom")
                    .and_then(|v| v.as_bool().ok())
                    .copied()
                    .unwrap_or(false)
            };
            let id = |i: &HashMap<String, AttributeValue>| {
                i.get("id")
                    .and_then(|v| v.as_s().ok())
                    .cloned()
                    .unwrap_or_default()
            };
            custom(a).cmp(&custom(b)).then_with(|| id(a).cmp(&id(b)))
        });

        let Some(item) = items.first() else {
            return Ok(None);
        };
        Ok(Some(item_to_entry(item)))
    }

    async fn find_by_id(&self, hevy_template_id: &str) -> Result<Option<CatalogEntry>> {
        let resp = self
            .client
            .get_item()
            .table_name(&self.table_name)
            .key("id", AttributeValue::S(hevy_template_id.to_string()))
            .send()
            .await
            .map_err(SwhError::dynamo)?;

        let Some(item) = resp.item else {
            return Ok(None);
        };
        Ok(Some(item_to_entry(&item)))
    }
}

fn item_to_entry(item: &HashMap<String, AttributeValue>) -> CatalogEntry {
    let id = item
        .get("id")
        .and_then(|v| v.as_s().ok())
        .cloned()
        .unwrap_or_default();
    let title = item
        .get("title")
        .and_then(|v| v.as_s().ok())
        .cloned()
        .unwrap_or_default();
    let kind = item
        .get("type")
        .and_then(|v| v.as_s().ok())
        .cloned()
        .unwrap_or_default();
    CatalogEntry {
        hevy_template_id: id,
        title,
        is_loaded_type: is_loaded_exercise_type(&kind),
        kind,
    }
}
