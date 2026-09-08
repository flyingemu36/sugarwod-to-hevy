// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

pub mod correlator;
pub mod description_parser;
pub mod load_calc;
pub mod normalize;
pub mod resolver;

/// One Hevy "set" worth of prescription extracted from a description line, before identity
/// resolution or load calculation has happened.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SetSpec {
    pub reps: Option<i64>,
    pub distance_meters: Option<i64>,
    pub duration_seconds: Option<i64>,
    /// e.g. `Some(0.65)` for "@ 65%" — resolved against `PersonalMax` in `load_calc`.
    pub percent_1rm: Option<f64>,
    /// From a parenthetical annotation like "(95/65)" — already in lbs, the men's/first value.
    pub explicit_weight_lb: Option<f64>,
}

/// One exercise line parsed out of a SugarWOD `description`, with the raw text identity plus
/// its structured set prescription. `resolution_key` is filled in by `correlator` once
/// `movements[]` corroboration (if any) has been considered.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedExerciseLine {
    pub name_text: String,
    pub sets: Vec<SetSpec>,
    pub rpe_note: Option<String>,
    /// Rest time attached to *this* exercise from a following `"-Rest M:00-"` marker line.
    pub rest_seconds_after: Option<i64>,
}

/// The outcome of resolving a `ParsedExerciseLine` to (or failing to find) a Hevy exercise.
#[derive(Debug, Clone)]
pub enum Resolution {
    /// Explicitly marked as "not a real exercise" (the old `"UNDEFINED"`/new `__SKIP__`
    /// sentinel) — excluded from the routine and from the unmapped report.
    Skip,
    Resolved {
        hevy_template_id: String,
        /// Hevy's exercise `type` (e.g. `weight_reps`) — used by `load_calc`.
        is_loaded_type: bool,
    },
    /// Neither an alias nor a direct catalog title match was found — reported, not dropped.
    Unmapped,
}

/// Exercises that couldn't be identified, or were identified but needed a load with no known
/// `PersonalMax` entry — both surfaced in the Discord summary so gaps in `ExerciseAlias`/
/// `PersonalMax` get curated as a side effect of daily use instead of requiring log-diving.
#[derive(Debug, Clone, Default)]
pub struct MatchReport {
    pub unmapped: Vec<String>,
    pub load_unknown: Vec<String>,
}

/// The full "Matching pipeline" from the plan, steps 1-5 end to end: parse `description`,
/// correlate against `movements[]` (works correctly when empty, the common case in real data),
/// resolve each line's identity, compute its loaded sets, and collect anything that needs
/// human attention into a `MatchReport` instead of silently dropping it.
pub async fn build_routine_exercises(
    description: &str,
    movements: &[crate::sugarwod::model::SugarWodMovement],
    aliases: &impl resolver::AliasStore,
    catalog: &impl resolver::CatalogStore,
    maxes: &impl load_calc::MaxStore,
) -> crate::error::Result<(
    Vec<crate::hevy::routine_builder::MatchedExercise>,
    MatchReport,
)> {
    let lines = description_parser::parse_description(description);
    let keys = correlator::resolution_keys(&lines, movements);

    let mut matched = Vec::new();
    let mut report = MatchReport::default();

    for (line, key) in lines.iter().zip(keys.iter()) {
        let resolution = resolver::resolve(key, aliases, catalog).await?;

        match &resolution {
            Resolution::Skip => continue,
            Resolution::Unmapped => {
                report.unmapped.push(line.name_text.clone());
                continue;
            }
            Resolution::Resolved {
                hevy_template_id, ..
            } => {
                let load_result = load_calc::compute_sets(&line.sets, &resolution, maxes).await?;
                if load_result.load_unknown {
                    report.load_unknown.push(line.name_text.clone());
                }
                let notes = line
                    .rpe_note
                    .clone()
                    .unwrap_or_else(|| line.name_text.clone());
                matched.push(crate::hevy::routine_builder::MatchedExercise {
                    hevy_template_id: hevy_template_id.clone(),
                    notes,
                    rest_seconds_after: line.rest_seconds_after,
                    sets: load_result.sets,
                });
            }
        }
    }

    Ok((matched, report))
}
