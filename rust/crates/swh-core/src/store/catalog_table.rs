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
                .map_err(|e| SwhError::Dynamo(e.to_string()))?;
        }
        Ok(())
    }
}

#[async_trait]
impl CatalogStore for DynamoCatalogStore {
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
            .limit(1)
            .send()
            .await
            .map_err(|e| SwhError::Dynamo(e.to_string()))?;

        let Some(item) = resp.items.unwrap_or_default().into_iter().next() else {
            return Ok(None);
        };
        Ok(Some(item_to_entry(&item)))
    }

    async fn find_by_id(&self, hevy_template_id: &str) -> Result<Option<CatalogEntry>> {
        let resp = self
            .client
            .get_item()
            .table_name(&self.table_name)
            .key("id", AttributeValue::S(hevy_template_id.to_string()))
            .send()
            .await
            .map_err(|e| SwhError::Dynamo(e.to_string()))?;

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
    }
}
