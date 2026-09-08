// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

use super::model::{
    CreateRoutineBody, CreateRoutinePayload, ExerciseHistoryResponse, ExerciseTemplate,
    ExerciseTemplatesPage, RoutineExercise, RoutineResponse, UpdateRoutineBody,
    UpdateRoutinePayload,
};
use crate::error::{Result, SwhError};
use crate::retry::retry_with_backoff;

const BASE_URL: &str = "https://api.hevyapp.com";

pub struct HevyClient {
    http: reqwest::Client,
    api_key: String,
}

impl HevyClient {
    pub fn new(http: reqwest::Client, api_key: String) -> Self {
        Self { http, api_key }
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{BASE_URL}{path}");
        retry_with_backoff(|| {
            let http = self.http.clone();
            let url = url.clone();
            let api_key = self.api_key.clone();
            async move {
                let resp = http.get(&url).header("api-key", api_key).send().await?;
                let status = resp.status();
                if !status.is_success() {
                    let body = resp.text().await.unwrap_or_default();
                    return Err(SwhError::HevyApi {
                        status: status.as_u16(),
                        body,
                    });
                }
                resp.json::<T>()
                    .await
                    .map_err(|e| SwhError::SugarWodParse(format!("hevy response parse: {e}")))
            }
        })
        .await
    }

    /// Paginates the full Hevy exercise-template catalog. `pageSize` is capped at 100 by Hevy.
    pub async fn fetch_all_exercise_templates(&self) -> Result<Vec<ExerciseTemplate>> {
        let mut all = Vec::new();
        let mut page = 1;
        loop {
            let resp: ExerciseTemplatesPage = self
                .get_json(&format!("/v1/exercise_templates?page={page}&pageSize=100"))
                .await?;
            let got = resp.exercise_templates.len();
            all.extend(resp.exercise_templates);
            if got == 0 || page >= resp.page_count {
                break;
            }
            page += 1;
        }
        Ok(all)
    }

    /// Full logged history for one exercise template — used to derive a personal max.
    pub async fn fetch_exercise_history(
        &self,
        exercise_template_id: &str,
    ) -> Result<ExerciseHistoryResponse> {
        self.get_json(&format!("/v1/exercise_history/{exercise_template_id}"))
            .await
    }

    /// `POST /v1/routines`. Deliberately **not** wrapped in `retry_with_backoff`: a real incident
    /// against the live API showed why — the response failed to parse (see
    /// `parse_routine_response` below for the actual cause), and because the retry wrapper
    /// covered the whole send-and-parse operation, each retry fired a brand new, non-idempotent
    /// `POST`, creating three duplicate routines in the real Hevy account before the final
    /// attempt's error propagated. A single POST attempt only: if the send itself fails
    /// (network/transport error), that's surfaced directly with no retry, since retrying a
    /// non-idempotent write risks the same duplication if the first attempt actually succeeded
    /// server-side but the client never saw the response.
    pub async fn create_routine(
        &self,
        title: &str,
        notes: &str,
        exercises: Vec<RoutineExercise>,
    ) -> Result<RoutineResponse> {
        let payload = CreateRoutinePayload {
            routine: CreateRoutineBody {
                title: title.to_string(),
                folder_id: None,
                notes: notes.to_string(),
                exercises,
            },
        };
        let url = format!("{BASE_URL}/v1/routines");
        let resp = self
            .http
            .post(&url)
            .header("api-key", &self.api_key)
            .json(&payload)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SwhError::HevyApi {
                status: status.as_u16(),
                body,
            });
        }
        let body = resp.text().await?;
        parse_routine_response(&body)
    }

    /// `PUT /v1/routines/{id}`. Returns `Ok(None)` on a 404 (routine no longer exists — e.g.
    /// deleted manually in the Hevy app) so callers can fall back to `create_routine`, rather
    /// than treating that as a hard error.
    pub async fn update_routine(
        &self,
        routine_id: &str,
        title: &str,
        notes: &str,
        exercises: Vec<RoutineExercise>,
    ) -> Result<Option<RoutineResponse>> {
        let payload = UpdateRoutinePayload {
            routine: UpdateRoutineBody {
                title: title.to_string(),
                notes: notes.to_string(),
                exercises,
            },
        };
        let url = format!("{BASE_URL}/v1/routines/{routine_id}");
        let resp = self
            .http
            .put(&url)
            .header("api-key", &self.api_key)
            .json(&payload)
            .send()
            .await?;
        let status = resp.status();
        if status.as_u16() == 404 {
            return Ok(None);
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SwhError::HevyApi {
                status: status.as_u16(),
                body,
            });
        }
        let body = resp.text().await?;
        Ok(Some(parse_routine_response(&body)?))
    }
}

/// Parses a `POST`/`PUT /v1/routines` response body into a `RoutineResponse`, tolerating every
/// shape seen in practice so far — Hevy's API has proven inconsistent enough here that guessing
/// a single "correct" shape from the docs isn't reliable (same docs-vs-live drift already found
/// on `equipment`/`equipment_category`, see `hevy/model.rs`):
///   - the published OpenAPI spec claims the `Routine` object directly, unwrapped
///   - a real `POST` against the live API returned it wrapped in `{"routine": {...}}`
///   - a real `PUT` in the same session returned it wrapped in `{"routine": [{...}]}`
///     — an array containing one object, not a bare object
///
/// Handles all three: unwrap a top-level `"routine"` key if present, then unwrap one level of
/// array if what's left is an array, before deserializing whatever object remains.
fn parse_routine_response(body: &str) -> Result<RoutineResponse> {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|e| {
        SwhError::SugarWodParse(format!(
            "hevy response was not valid JSON: {e} (body: {body})"
        ))
    })?;

    let unwrapped = value.get("routine").unwrap_or(&value);
    let target = match unwrapped.as_array().and_then(|arr| arr.first()) {
        Some(first) => first,
        None => unwrapped,
    };

    serde_json::from_value(target.clone()).map_err(|e| {
        SwhError::SugarWodParse(format!(
            "hevy routine response missing expected fields: {e} (body: {body})"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unwrapped_shape_per_the_published_docs() {
        let body = r#"{"id":"00000000-0000-4000-8000-000000000001","title":"Test Routine"}"#;
        let parsed = parse_routine_response(body).unwrap();
        assert_eq!(parsed.id, "00000000-0000-4000-8000-000000000001");
    }

    #[test]
    fn parses_wrapped_object_shape_seen_in_real_live_post_response() {
        let body = r#"{"routine":{"id":"00000000-0000-4000-8000-000000000001","title":"Test Routine","exercises":[]}}"#;
        let parsed = parse_routine_response(body).unwrap();
        assert_eq!(parsed.id, "00000000-0000-4000-8000-000000000001");
    }

    #[test]
    fn parses_wrapped_array_shape_seen_in_real_live_put_response() {
        let body = r#"{"routine":[{"id":"00000000-0000-4000-8000-000000000001","title":"Test Routine","exercises":[]}]}"#;
        let parsed = parse_routine_response(body).unwrap();
        assert_eq!(parsed.id, "00000000-0000-4000-8000-000000000001");
    }

    #[test]
    fn error_message_includes_raw_body_for_diagnosability() {
        let body = r#"{"unexpected":"shape"}"#;
        let err = parse_routine_response(body).unwrap_err();
        assert!(err.to_string().contains("unexpected"));
    }
}
