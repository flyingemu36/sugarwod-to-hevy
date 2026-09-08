// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct SugarWodResponse {
    pub success: bool,
    pub data: Vec<SugarWodWorkout>,
}

// SugarWOD's API model (https://app.sugarwod.com/developers-api-docs#get-started)
// is in a beta format. There's no guarantee that your gym will use the same fields
// the way they're intended to. The Workouts are a giant text (well, JSON) blob that
// are very unstructured. Hence why we have a large amount of code to work on parsing
// and matching.
#[derive(Debug, Clone, Deserialize)]
pub struct SugarWodWorkout {
    pub id: String,
    pub track: String,
    pub title: String,
    pub description: String,
    #[serde(rename = "scheduledDateInteger")]
    pub scheduled_date_integer: i64,
    #[serde(rename = "scheduledDateDisplay")]
    pub scheduled_date_display: String,
    #[serde(default)]
    pub movements: Vec<SugarWodMovement>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SugarWodMovement {
    pub id: String,
    pub name: String,
}
