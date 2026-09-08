// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

use crate::error::{Result, SwhError};
use crate::matching::load_calc::MaxStore;
use async_trait::async_trait;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_dynamodb::Client;

pub struct DynamoMaxStore {
    client: Client,
    table_name: String,
}

// Hevy doesn't have an API to fetch your 1RM, so we store it in a
// DynamoDB table. This is a simple key-value store with the
// exercise template ID as the key and the 1RM weight in kg as the value.
impl DynamoMaxStore {
    pub fn new(client: Client, table_name: String) -> Self {
        Self { client, table_name }
    }

    /// Upserts a derived max for one exercise — used by `refresh-maxes`.
    pub async fn upsert(
        &self,
        hevy_template_id: &str,
        title: &str,
        max_weight_kg: f64,
        source_workout_id: &str,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.client
            .put_item()
            .table_name(&self.table_name)
            .item(
                "exercise_template_id",
                AttributeValue::S(hevy_template_id.to_string()),
            )
            .item("title", AttributeValue::S(title.to_string()))
            .item(
                "max_weight_kg",
                AttributeValue::N(max_weight_kg.to_string()),
            )
            .item(
                "source_workout_id",
                AttributeValue::S(source_workout_id.to_string()),
            )
            .item("updated_at", AttributeValue::S(now))
            .send()
            .await
            .map_err(|e| SwhError::Dynamo(e.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl MaxStore for DynamoMaxStore {
    async fn get_max_kg(&self, hevy_template_id: &str) -> Result<Option<f64>> {
        let resp = self
            .client
            .get_item()
            .table_name(&self.table_name)
            .key(
                "exercise_template_id",
                AttributeValue::S(hevy_template_id.to_string()),
            )
            .send()
            .await
            .map_err(|e| SwhError::Dynamo(e.to_string()))?;

        let Some(item) = resp.item else {
            return Ok(None);
        };
        Ok(item
            .get("max_weight_kg")
            .and_then(|v| v.as_n().ok())
            .and_then(|s| s.parse::<f64>().ok()))
    }
}
