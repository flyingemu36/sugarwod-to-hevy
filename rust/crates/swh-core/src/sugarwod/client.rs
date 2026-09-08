// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

use super::jsonp::unwrap_jsonp;
use super::model::SugarWodResponse;
use crate::error::{Result, SwhError};
use crate::retry::retry_with_backoff;

//Yes, there's a version 2 of the API, but the gym in question uses v1.
// The v2 API is also a private API, so it requires a user token to access.
// The v1 API is public and doesn't require a token.
const BASE_URL: &str = "https://app.sugarwod.com/public/api/v1";

pub struct SugarWodClient {
    http: reqwest::Client,
}

impl SugarWodClient {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// Fetches all workouts tagged `workout-of-the-day` for `affiliate_id` on `date`
    /// (SugarWOD's `YYYYMMDD` format). May return more than one workout (e.g. separate
    /// "Performance" and "HYROX" variants on the same day).
    pub async fn fetch_workouts(&self, affiliate_id: &str, date: &str) -> Result<SugarWodResponse> {
        let url = format!(
            "{BASE_URL}/affiliates/{affiliate_id}/workouts/{date}?jsonp=SugarWOD.workoutsFetched&tracks=%5B%22workout-of-the-day%22%5D"
        );

        let text = retry_with_backoff(|| {
            let http = self.http.clone();
            let url = url.clone();
            async move {
                let resp = http.get(&url).send().await?;
                if !resp.status().is_success() {
                    return Err(SwhError::SugarWodFetch(format!("status {}", resp.status())));
                }
                Ok(resp.text().await?)
            }
        })
        .await?;

        unwrap_jsonp(&text)
    }
}
