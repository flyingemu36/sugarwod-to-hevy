// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

use crate::error::{Result, SwhError};
use crate::matching::resolver::{AliasEntry, AliasStore};
use async_trait::async_trait;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_dynamodb::Client;

pub struct DynamoAliasStore {
    client: Client,
    table_name: String,
}

impl DynamoAliasStore {
    pub fn new(client: Client, table_name: String) -> Self {
        Self { client, table_name }
    }

    /// Full scan for the distinct `hevy_template_id`s currently referenced by any alias —
    /// used by `refresh-maxes` to bound `exercise_history` calls to exercises actually in use,
    /// rather than the whole 1000+ exercise catalog. The table is small (dozens of rows), so a
    /// scan here is appropriate. Note this is the ONLY Scan in the codebase — catalog lookups
    /// go through the TitleNormalizedIndex GSI precisely to avoid scanning the large table.
    pub async fn distinct_template_ids(&self) -> Result<Vec<String>> {
        let mut ids = std::collections::HashSet::new();
        let mut last_key = None;
        loop {
            let mut req = self.client.scan().table_name(&self.table_name);
            if let Some(key) = last_key.clone() {
                req = req.set_exclusive_start_key(Some(key));
            }
            let resp = req
                .send()
                .await
                .map_err(|e| SwhError::Dynamo(e.to_string()))?;
            for item in resp.items.unwrap_or_default() {
                if let Some(id) = item.get("hevy_template_id").and_then(|v| v.as_s().ok()) {
                    if !id.is_empty() {
                        ids.insert(id.clone());
                    }
                }
            }
            last_key = resp.last_evaluated_key;
            if last_key.is_none() {
                break;
            }
        }
        Ok(ids.into_iter().collect())
    }
}

#[async_trait]
impl AliasStore for DynamoAliasStore {
    async fn get(&self, alias: &str) -> Result<Option<AliasEntry>> {
        let resp = self
            .client
            .get_item()
            .table_name(&self.table_name)
            .key("alias", AttributeValue::S(alias.to_string()))
            .send()
            .await
            .map_err(|e| SwhError::Dynamo(e.to_string()))?;

        let Some(item) = resp.item else {
            return Ok(None);
        };

        let canonical_title = item
            .get("canonical_title")
            .and_then(|v| v.as_s().ok())
            .cloned()
            .unwrap_or_default();
        let hevy_template_id = item
            .get("hevy_template_id")
            .and_then(|v| v.as_s().ok())
            .cloned()
            .unwrap_or_default();

        Ok(Some(AliasEntry {
            canonical_title,
            hevy_template_id,
        }))
    }

    async fn put(&self, alias: &str, entry: AliasEntry, source: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.client
            .put_item()
            .table_name(&self.table_name)
            .item("alias", AttributeValue::S(alias.to_string()))
            .item("canonical_title", AttributeValue::S(entry.canonical_title))
            .item(
                "hevy_template_id",
                AttributeValue::S(entry.hevy_template_id),
            )
            .item("source", AttributeValue::S(source.to_string()))
            .item("updated_at", AttributeValue::S(now))
            .send()
            .await
            .map_err(|e| SwhError::Dynamo(e.to_string()))?;
        Ok(())
    }
}
