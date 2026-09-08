// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

use crate::error::{Result, SwhError};
use serde::Serialize;

#[derive(Serialize)]
struct DiscordMessage<'a> {
    content: &'a str,
}

pub async fn fetch_webhook_url(client: &aws_sdk_ssm::Client, param_name: &str) -> Result<String> {
    let resp = client
        .get_parameter()
        .name(param_name)
        .with_decryption(true)
        .send()
        .await
        .map_err(|e| SwhError::Ssm(e.to_string()))?;

    resp.parameter
        .and_then(|p| p.value)
        .ok_or_else(|| SwhError::Ssm("discord webhook parameter has no value".to_string()))
}

pub async fn post_message(http: &reqwest::Client, webhook_url: &str, content: &str) -> Result<()> {
    let resp = http
        .post(webhook_url)
        .json(&DiscordMessage { content })
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(SwhError::Discord(format!("status {status}: {body}")));
    }
    Ok(())
}

/// One workout's contribution to the daily summary, before all workouts are joined into a
/// single Discord message (decision #3: one summary covering everything synced that morning).
pub struct WorkoutOutcome {
    pub title: String,
    pub exercise_count: usize,
    pub unmapped: Vec<String>,
    pub load_unknown: Vec<String>,
}

pub fn build_summary_message(outcomes: &[WorkoutOutcome]) -> String {
    let mut lines = Vec::new();
    for outcome in outcomes {
        if outcome.exercise_count == 0 {
            // Skipped rather than synced — no exercise resolved, so no Hevy routine was written
            // (an empty routine would be rejected by Hevy). The unmapped list below shows what
            // to add to ExerciseAlias to fix it.
            lines.push(format!(
                "⏭️ **{}** — nothing synced (no exercises resolved)",
                outcome.title
            ));
        } else {
            lines.push(format!(
                "✅ **{}** synced ({} exercises)",
                outcome.title, outcome.exercise_count
            ));
        }
        if !outcome.unmapped.is_empty() {
            lines.push(format!(
                "   ⚠️ {} couldn't be mapped: {}",
                outcome.unmapped.len(),
                outcome.unmapped.join(", ")
            ));
        }
        if !outcome.load_unknown.is_empty() {
            lines.push(format!(
                "   ⚠️ {} loaded without a known max: {}",
                outcome.load_unknown.len(),
                outcome.load_unknown.join(", ")
            ));
        }
    }
    lines.join("\n")
}

pub fn build_failure_message(context: &str, error: &str) -> String {
    format!("❌ WOD sync failed ({context}): {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_includes_unmapped_and_load_unknown() {
        let outcomes = vec![WorkoutOutcome {
            title: "HYROX".to_string(),
            exercise_count: 9,
            unmapped: vec!["Weird Movement".to_string()],
            load_unknown: vec!["Alt DB Hang Snatch".to_string()],
        }];
        let msg = build_summary_message(&outcomes);
        assert!(msg.contains("HYROX"));
        assert!(msg.contains("Weird Movement"));
        assert!(msg.contains("Alt DB Hang Snatch"));
    }

    #[test]
    fn zero_exercise_outcome_reads_as_skipped_not_synced() {
        // A workout where nothing resolved must be reported as skipped, not
        //  "✅ synced (0 exercises)", and still list what couldn't be mapped.
        let outcomes = vec![WorkoutOutcome {
            title: "Fitness".to_string(),
            exercise_count: 0,
            unmapped: vec!["DB Reverse Lunges".to_string()],
            load_unknown: vec![],
        }];
        let msg = build_summary_message(&outcomes);
        assert!(!msg.contains('✅'));
        assert!(msg.contains("nothing synced"));
        assert!(msg.contains("DB Reverse Lunges"));
    }

    #[test]
    fn clean_sync_has_no_warning_lines() {
        let outcomes = vec![WorkoutOutcome {
            title: "Performance".to_string(),
            exercise_count: 5,
            unmapped: vec![],
            load_unknown: vec![],
        }];
        let msg = build_summary_message(&outcomes);
        assert!(!msg.contains("⚠️"));
    }
}
