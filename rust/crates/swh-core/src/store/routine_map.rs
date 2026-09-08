// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

use crate::error::{Result, SwhError};
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_dynamodb::Client;

pub struct RoutineMapStore {
    client: Client,
    table_name: String,
}

impl RoutineMapStore {
    pub fn new(client: Client, table_name: String) -> Self {
        Self { client, table_name }
    }

    pub async fn get_routine_id(&self, routine_key: &str) -> Result<Option<String>> {
        let resp = self
            .client
            .get_item()
            .table_name(&self.table_name)
            .key("routine_key", AttributeValue::S(routine_key.to_string()))
            .send()
            .await
            .map_err(|e| SwhError::Dynamo(e.to_string()))?;

        Ok(resp.item.and_then(|item| {
            item.get("hevy_routine_id")
                .and_then(|v| v.as_s().ok())
                .cloned()
        }))
    }

    pub async fn upsert(
        &self,
        routine_key: &str,
        hevy_routine_id: &str,
        last_synced_title: &str,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.client
            .put_item()
            .table_name(&self.table_name)
            .item("routine_key", AttributeValue::S(routine_key.to_string()))
            .item(
                "hevy_routine_id",
                AttributeValue::S(hevy_routine_id.to_string()),
            )
            .item(
                "last_synced_title",
                AttributeValue::S(last_synced_title.to_string()),
            )
            .item("updated_at", AttributeValue::S(now))
            .send()
            .await
            .map_err(|e| SwhError::Dynamo(e.to_string()))?;
        Ok(())
    }
}

/// Word-order-independent normalized routine key: lowercase, split into words on non-alphanumerics,
/// drop the filler word "and", sort tokens alphabetically, join with '-'. Both
/// "Performance + Fitness" and "Fitness + Performance" collapse to the same key, since SugarWOD
/// doesn't guarantee a consistent merge order for combined-track workout titles.
pub fn normalize_routine_key(title: &str) -> String {
    let lower = title.to_lowercase();
    let mut words: Vec<String> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && *w != "and")
        .map(|w| w.to_string())
        .collect();
    words.sort();
    words.join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_order_independent_merged_title() {
        assert_eq!(
            normalize_routine_key("Performance + Fitness"),
            normalize_routine_key("Fitness + Performance")
        );
        assert_eq!(
            normalize_routine_key("Performance + Fitness"),
            "fitness-performance"
        );
    }

    #[test]
    fn single_word_title() {
        assert_eq!(normalize_routine_key("HYROX"), "hyrox");
    }
}
