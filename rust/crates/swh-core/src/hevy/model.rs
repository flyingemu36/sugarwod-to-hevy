// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

use serde::{Deserialize, Serialize};

// ---- Exercise template catalog (GET /v1/exercise_templates) ----
// Hevy's *published* OpenAPI spec (https://api.hevyapp.com/docs/swagger-ui-init.js) claims this
// field is `equipment_category` (a fixed enum) — but a live gamma integration test run against
// the real endpoint proved the actual wire response uses `equipment` (a plain string, e.g.
// "barbell"/"dumbbell"/"none"/"other"). The docs and the live behavior have drifted apart;
// this code trusts the live response. Full writeup in
// docs/bug-reports/hevy-openapi-equipment-field-mismatch.md — if you are ever tempted to
// "fix" this back to match the published spec, don't: the spec is what's wrong.

#[derive(Debug, Clone, Deserialize)]
pub struct ExerciseTemplatesPage {
    pub page: i32,
    pub page_count: i32,
    pub exercise_templates: Vec<ExerciseTemplate>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExerciseTemplate {
    pub id: String,
    pub title: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub primary_muscle_group: String,
    #[serde(default)]
    pub secondary_muscle_groups: Vec<String>,
    pub equipment: String,
    pub is_custom: bool,
}

impl ExerciseTemplate {
    /// Movements of this Hevy exercise `type` are normally externally loaded (barbell/dumbbell/
    /// etc.) — used by `load_calc` to decide whether an unlabeled movement should default to
    /// 50% of the lifter's known max, versus a bodyweight/cardio type that never gets a weight.
    pub fn is_loaded_type(&self) -> bool {
        is_loaded_exercise_type(&self.kind)
    }
}

/// Shared with `store::catalog_table`, which reconstructs this from a raw DynamoDB item rather
/// than an `ExerciseTemplate` struct.
pub fn is_loaded_exercise_type(kind: &str) -> bool {
    kind == "weight_reps" || kind == "weight_distance" || kind == "weight_duration"
}

// ---- Exercise history (GET /v1/exercise_history/{exerciseTemplateId}) ----
// Used for personal-max derivation: this is a per-exercise, already-flattened set history —
// a far better fit than paginating the user's entire workout history (`GET /v1/workouts`) and
// reducing over every set on every exercise, since we only need history for exercises actually
// referenced by ExerciseAlias.

#[derive(Debug, Clone, Deserialize)]
pub struct ExerciseHistoryResponse {
    pub exercise_history: Vec<ExerciseHistoryEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExerciseHistoryEntry {
    pub exercise_template_id: String,
    pub weight_kg: Option<f64>,
    pub workout_id: String,
}

// ---- Routine create/update (POST|PUT /v1/routines) ----
// Verified response shapes against the live spec: unlike a single `GET /v1/routines/{id}`
// (which wraps the routine in `{"routine": {...}}`), POST and PUT both return the `Routine`
// object *directly* — no wrapper. Confirmed against the live API, not assumed from the docs.
//
// PUT's request body also does NOT accept `folder_id` (POST does) — verified from
// `PutRoutinesRequestBody` vs `PostRoutinesRequestBody` in the spec — hence two distinct
// wrapper/body types below rather than one shared struct with an ignored field.

#[derive(Debug, Clone, Serialize)]
pub struct CreateRoutinePayload {
    pub routine: CreateRoutineBody,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateRoutineBody {
    pub title: String,
    pub folder_id: Option<i64>,
    pub notes: String,
    pub exercises: Vec<RoutineExercise>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateRoutinePayload {
    pub routine: UpdateRoutineBody,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateRoutineBody {
    pub title: String,
    pub notes: String,
    pub exercises: Vec<RoutineExercise>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoutineExercise {
    pub exercise_template_id: String,
    pub superset_id: Option<i64>,
    pub rest_seconds: Option<i64>,
    pub notes: String,
    pub sets: Vec<RoutineSet>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoutineSet {
    #[serde(rename = "type")]
    pub kind: String,
    pub weight_kg: Option<f64>,
    pub reps: Option<i64>,
    pub distance_meters: Option<i64>,
    pub duration_seconds: Option<i64>,
    pub custom_metric: Option<f64>,
}

impl RoutineSet {
    pub fn normal() -> Self {
        Self {
            kind: "normal".to_string(),
            weight_kg: None,
            reps: None,
            distance_meters: None,
            duration_seconds: None,
            custom_metric: None,
        }
    }
}

/// Response body for both `POST /v1/routines` (201) and `PUT /v1/routines/{id}` (200) — the
/// `Routine` object directly, unwrapped. Only `id` is modeled; the rest of the payload
/// (exercises, timestamps, etc.) isn't needed by this system and is ignored by serde.
#[derive(Debug, Clone, Deserialize)]
pub struct RoutineResponse {
    pub id: String,
}
