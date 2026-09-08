// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

use crate::config::hevy_secret_id;
use crate::error::{Result, SwhError};
use serde::Deserialize;

#[derive(Deserialize)]
struct HevySecret {
    api_key: String,
}

/// Fetches the Hevy API key from Secrets Manager. Standardizes on a single canonical JSON
/// shape: the secret's value must be a JSON object with an `api_key` key. The secret's id comes
/// from the `HEVY_SECRET_ID` env var, injected by the CDK `ComputeStack` from deployment config.
pub async fn fetch_hevy_api_key(client: &aws_sdk_secretsmanager::Client) -> Result<String> {
    let secret_id = hevy_secret_id()?;
    let resp = client
        .get_secret_value()
        .secret_id(&secret_id)
        .send()
        .await
        .map_err(|e| SwhError::Secrets(e.to_string()))?;

    let raw = resp
        .secret_string
        .ok_or_else(|| SwhError::Secrets("secret has no SecretString".to_string()))?;

    let parsed: HevySecret = serde_json::from_str(&raw)
        .map_err(|e| SwhError::Secrets(format!("secret JSON shape mismatch: {e}")))?;

    Ok(parsed.api_key)
}
