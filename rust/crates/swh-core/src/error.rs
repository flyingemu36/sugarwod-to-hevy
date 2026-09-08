// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

#[derive(Debug, thiserror::Error)]
pub enum SwhError {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("SugarWOD fetch failed: {0}")]
    SugarWodFetch(String),

    #[error("SugarWOD response parse failed: {0}")]
    SugarWodParse(String),

    #[error("Hevy API error (status {status}): {body}")]
    HevyApi { status: u16, body: String },

    #[error("DynamoDB error: {0}")]
    Dynamo(String),

    #[error("SSM error: {0}")]
    Ssm(String),

    #[error("Secrets Manager error: {0}")]
    Secrets(String),

    #[error("Discord webhook post failed: {0}")]
    Discord(String),

    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
}

pub type Result<T> = std::result::Result<T, SwhError>;
