// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi

use super::normalize::normalize;
use super::Resolution;
use crate::error::Result;
use async_trait::async_trait;

/// The `"UNDEFINED"`-successor sentinel: an alias explicitly marked as "not a real exercise" —
/// excluded from the routine and never reported as unmapped.
pub const SKIP_SENTINEL: &str = "__SKIP__";

#[derive(Debug, Clone)]
pub struct AliasEntry {
    pub canonical_title: String,
    pub hevy_template_id: String,
}

#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub hevy_template_id: String,
    pub title: String,
    pub is_loaded_type: bool,
    /// The raw Hevy `type` (e.g. `weight_reps`, `distance_duration`). Kept alongside the derived
    /// `is_loaded_type` because `load_calc` needs the full type to know which set *fields* the
    /// template can hold, not just whether it takes a weight.
    pub kind: String,
}

/// Abstraction over `ExerciseAlias` so `resolver` can be unit-tested with an in-memory double
/// instead of a real DynamoDB table.
#[async_trait]
pub trait AliasStore {
    async fn get(&self, alias: &str) -> Result<Option<AliasEntry>>;
    async fn put(&self, alias: &str, entry: AliasEntry, source: &str) -> Result<()>;
}

/// Abstraction over `HevyExerciseCatalog` — both its `title_normalized` GSI (for the fallback
/// lookup) and a plain primary-key `GetItem` by `id` (used to recover `is_loaded_type` for
/// exercises that resolved via an `ExerciseAlias` hit, since alias rows don't carry it).
#[async_trait]
pub trait CatalogStore {
    async fn find_by_normalized_title(
        &self,
        title_normalized: &str,
    ) -> Result<Option<CatalogEntry>>;
    async fn find_by_id(&self, hevy_template_id: &str) -> Result<Option<CatalogEntry>>;
}

/// Resolves one exercise identity key to a Hevy template (or Skip/Unmapped), per the plan's
/// "Matching pipeline" step 3: alias exact hit -> catalog GSI exact hit (write back an
/// `auto-resolved` alias so curation compounds over time) -> unmapped.
pub async fn resolve(
    key: &str,
    aliases: &impl AliasStore,
    catalog: &impl CatalogStore,
) -> Result<Resolution> {
    let normalized_key = normalize(key);

    // Nothing survived normalizing (a "-" or "+" separator line), so there is no movement to
    // look up — and DynamoDB rejects an empty key attribute outright, which used to fail the
    // whole sync (2026-10-15, 2026-10-21). Skip, rather than report it as unmapped.
    if normalized_key.is_empty() {
        return Ok(Resolution::Skip);
    }

    if let Some(alias) = aliases.get(&normalized_key).await? {
        if alias.canonical_title == SKIP_SENTINEL {
            return Ok(Resolution::Skip);
        }
        // Alias rows don't carry catalog metadata (`is_loaded_type`, `kind`) — it's catalog data,
        // not translation data — so recover it with a cheap primary-key lookup by id.
        let entry = catalog.find_by_id(&alias.hevy_template_id).await?;
        let is_loaded_type = entry.as_ref().map(|c| c.is_loaded_type).unwrap_or(false);
        let kind = entry.map(|c| c.kind).unwrap_or_default();
        return Ok(Resolution::Resolved {
            hevy_template_id: alias.hevy_template_id,
            is_loaded_type,
            kind,
        });
    }

    if let Some(catalog_entry) = catalog.find_by_normalized_title(&normalized_key).await? {
        aliases
            .put(
                &normalized_key,
                AliasEntry {
                    canonical_title: catalog_entry.title.clone(),
                    hevy_template_id: catalog_entry.hevy_template_id.clone(),
                },
                "auto-resolved",
            )
            .await?;
        return Ok(Resolution::Resolved {
            hevy_template_id: catalog_entry.hevy_template_id,
            is_loaded_type: catalog_entry.is_loaded_type,
            kind: catalog_entry.kind,
        });
    }

    // Fuzzy fallback — only after an exact alias AND exact catalog miss, so it can never override
    // an exact match. Bridges the mechanical miss classes seen in real WOD data: equipment
    // abbreviations ("DB" -> "Dumbbell", tried in both of Hevy's inconsistent word orders),
    // singular/plural drift ("Burpees" -> "Burpee"), droppable qualifiers ("DB Alt Snatch") and
    // walk/carry synonyms. A hit writes back an alias for the original key (source
    // `auto-resolved-variant`, so fuzzy matches are auditable and O(1) next time).
    for candidate in resolution_candidates(&normalized_key) {
        // Each candidate is tried against the alias table before the catalog. Curated aliases
        // carry the coach vocabulary the catalog never will — "kb farmer carry" -> Farmers Walk —
        // and checking only the catalog meant "KB Farmer Walk" missed it over a one-word
        // difference. A skip-sentinel alias is not honored here: a fuzzy neighbour being noise
        // says nothing about whether this key is.
        if let Some(alias) = aliases.get(&candidate).await? {
            if alias.canonical_title != SKIP_SENTINEL && !alias.hevy_template_id.is_empty() {
                let entry = catalog.find_by_id(&alias.hevy_template_id).await?;
                aliases
                    .put(&normalized_key, alias.clone(), "auto-resolved-variant")
                    .await?;
                return Ok(Resolution::Resolved {
                    hevy_template_id: alias.hevy_template_id,
                    is_loaded_type: entry.as_ref().map(|c| c.is_loaded_type).unwrap_or(false),
                    kind: entry.map(|c| c.kind).unwrap_or_default(),
                });
            }
        }
        if let Some(catalog_entry) = catalog.find_by_normalized_title(&candidate).await? {
            aliases
                .put(
                    &normalized_key,
                    AliasEntry {
                        canonical_title: catalog_entry.title.clone(),
                        hevy_template_id: catalog_entry.hevy_template_id.clone(),
                    },
                    "auto-resolved-variant",
                )
                .await?;
            return Ok(Resolution::Resolved {
                hevy_template_id: catalog_entry.hevy_template_id,
                is_loaded_type: catalog_entry.is_loaded_type,
                kind: catalog_entry.kind,
            });
        }
    }

    Ok(Resolution::Unmapped)
}

/// Equipment/shorthand abbreviations coaches write vs. how Hevy's catalog spells them out.
const ABBREVIATIONS: &[(&str, &str)] = &[
    ("db", "dumbbell"),
    ("kb", "kettlebell"),
    ("bb", "barbell"),
    ("sa", "single arm"),
    ("sl", "single leg"),
];

/// Qualifier words that describe *how* a movement is done rather than *which* movement it is —
/// "DB Alt Snatch" is a DB snatch, "Max Power Cleans" are power cleans, "Heavy DB Box Step Overs"
/// are DB box step overs. Hevy titles never carry them, so the catalog title a coach means can
/// only be reached once they are dropped. "single" is handled separately (see
/// `strip_single_qualifier`) because "single arm"/"single leg" must survive.
const DROPPABLE_QUALIFIERS: &[&str] = &["alt", "alternating", "max", "heavy", "strict"];

/// Word swaps that name the same movement. Coaches write "KB Farmer Walk" and "KB Farmer Carry"
/// interchangeably, and the alias table holds only one spelling of each.
const SYNONYMS: &[(&str, &str)] = &[("walk", "carry"), ("carry", "walk")];

/// Ordered, de-duplicated fuzzy candidates for an already-normalized key (never the key itself).
///
/// Built from a short list of *variants* of the key, most faithful first: the key as written, the
/// key with droppable qualifiers removed, the key with a single-arm "sa" also removed, and a
/// walk/carry synonym swap of each. Every variant contributes itself plus its spelling forms
/// (see `spelling_forms`). Garbage candidates are harmless: they simply miss.
fn resolution_candidates(normalized_key: &str) -> Vec<String> {
    let tokens: Vec<String> = normalized_key
        .split_whitespace()
        .map(String::from)
        .collect();
    if tokens.is_empty() {
        return Vec::new();
    }

    let mut variants: Vec<Vec<String>> = vec![tokens.clone()];
    let stripped = strip_qualifiers(&tokens);
    if stripped != tokens && !stripped.is_empty() {
        variants.push(stripped.clone());
    }
    // "SA" last of all: a single-arm DB thruster is still the DB Thruster template, but ABBREVIATIONS
    // first gives "single arm" its chance at a real "Single Arm X" title.
    let without_sa: Vec<String> = stripped.iter().filter(|t| *t != "sa").cloned().collect();
    if without_sa != stripped && !without_sa.is_empty() {
        variants.push(without_sa);
    }
    for i in 0..variants.len() {
        let swapped: Vec<String> = variants[i]
            .iter()
            .map(|t| match SYNONYMS.iter().find(|(from, _)| from == t) {
                Some((_, to)) => to.to_string(),
                None => t.clone(),
            })
            .collect();
        if swapped != variants[i] {
            variants.push(swapped);
        }
    }

    let mut out: Vec<String> = Vec::new();
    for variant in &variants {
        for form in std::iter::once(variant.join(" ")).chain(spelling_forms(variant)) {
            if !form.is_empty() && form != normalized_key && !out.contains(&form) {
                out.push(form);
            }
        }
    }
    out
}

/// Spelling forms of one key variant. Expands equipment abbreviations in place ("db floor press"
/// -> "dumbbell floor press", the "Dumbbell X" style) and with the equipment moved to the end
/// ("floor press dumbbell", the "X (Dumbbell)" style — Hevy uses both), and toggles the last
/// token's singular/plural.
fn spelling_forms(tokens: &[String]) -> Vec<String> {
    let mut expanded: Vec<String> = Vec::new();
    let mut expansion_words: Vec<String> = Vec::new();
    // The abbreviations exactly as the coach wrote them, kept unexpanded for the "(DB)" style
    // below, plus everything that wasn't an abbreviation.
    let mut abbrev_tokens: Vec<String> = Vec::new();
    let mut plain_tokens: Vec<String> = Vec::new();
    for tok in tokens {
        match ABBREVIATIONS.iter().find(|(ab, _)| *ab == tok) {
            Some((_, full)) => {
                for w in full.split_whitespace() {
                    expanded.push(w.to_string());
                    expansion_words.push(w.to_string());
                }
                abbrev_tokens.push(tok.clone());
            }
            None => {
                expanded.push(tok.clone());
                plain_tokens.push(tok.clone());
            }
        }
    }

    let mut bases: Vec<Vec<String>> = vec![tokens.to_vec()];
    if !expansion_words.is_empty() {
        bases.push(expanded.clone());
        // Equipment moved to the end: non-expansion words first, then the expansion words.
        let mut suffix: Vec<String> = expanded
            .iter()
            .filter(|w| !expansion_words.contains(w))
            .cloned()
            .collect();
        suffix.extend(expansion_words.iter().cloned());
        bases.push(suffix);

        // Abbreviation kept *literal* and moved to the end — the "Front Squat (DB)" /
        // "Push Press (KB)" style, which normalizes to "front squat db" / "push press kb".
        // Hevy custom templates created in the app use this spelling heavily (7 in the live
        // catalog), and none of the expanded forms above can reach them: expanding "db" to
        // "dumbbell" is exactly the wrong move when the catalog title says "DB". So a coach
        // writing "DB Front Squat" missed a template that was sitting right there.
        let mut literal_suffix = plain_tokens.clone();
        literal_suffix.extend(abbrev_tokens.iter().cloned());
        bases.push(literal_suffix);
    }

    let mut out = Vec::new();
    for base in &bases {
        out.push(base.join(" "));
        out.push(swap_last_token_plurality(base));
    }
    out
}

/// The key with every droppable qualifier removed — `DROPPABLE_QUALIFIERS` anywhere, plus a
/// leading per-limb "single".
///
/// A leading "Single" is a per-limb rep qualifier — "Single DB Step-Overs" is a DB step-over done
/// one leg at a time, the same movement as the two-limb spelling — so the catalog title it wants
/// never contains the word. Dropping it *before* the spelling forms are generated is what makes it
/// useful: "single db step ups" can then reach "Dumbbell Step-Ups" and not just "db step ups".
fn strip_qualifiers(tokens: &[String]) -> Vec<String> {
    let tokens = strip_single_qualifier(tokens).unwrap_or(tokens);
    tokens
        .iter()
        .filter(|t| !DROPPABLE_QUALIFIERS.contains(&t.as_str()))
        .cloned()
        .collect()
}

/// The tokens after a leading, droppable "single", or `None` when there is nothing to drop.
///
/// "single arm"/"single leg" are held back deliberately: those are whole-compound qualifiers Hevy
/// spells out in its own titles ("Single Leg Romanian Deadlift"), and `ABBREVIATIONS` expands the
/// coach's "SA"/"SL" *into* them. Dropping the word there would undo that expansion and match the
/// two-limb movement instead — a wrong answer, which is worse than no answer.
fn strip_single_qualifier(tokens: &[String]) -> Option<&[String]> {
    let (first, rest) = tokens.split_first()?;
    if first != "single" || rest.is_empty() {
        return None;
    }
    match rest.first().map(String::as_str) {
        Some("arm" | "leg") => None,
        _ => Some(rest),
    }
}

/// Crude singular<->plural: toggle a trailing "s" on the last token, rejoined. "renegade rows"
/// -> "renegade row"; "burpee" -> "burpees".
fn swap_last_token_plurality(tokens: &[String]) -> String {
    let mut t = tokens.to_vec();
    if let Some(last) = t.last_mut() {
        match last.strip_suffix('s') {
            Some(stripped) => *last = stripped.to_string(),
            None => last.push('s'),
        }
    }
    t.join(" ")
}

#[cfg(test)]
pub mod test_doubles {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct InMemoryAliasStore {
        pub rows: Mutex<HashMap<String, AliasEntry>>,
        pub writes: Mutex<Vec<(String, String)>>, // (alias, source)
    }

    #[async_trait]
    impl AliasStore for InMemoryAliasStore {
        async fn get(&self, alias: &str) -> Result<Option<AliasEntry>> {
            Ok(self.rows.lock().unwrap().get(alias).cloned())
        }

        async fn put(&self, alias: &str, entry: AliasEntry, source: &str) -> Result<()> {
            self.rows.lock().unwrap().insert(alias.to_string(), entry);
            self.writes
                .lock()
                .unwrap()
                .push((alias.to_string(), source.to_string()));
            Ok(())
        }
    }

    #[derive(Default)]
    pub struct InMemoryCatalogStore {
        pub rows: Mutex<HashMap<String, CatalogEntry>>,
    }

    #[async_trait]
    impl CatalogStore for InMemoryCatalogStore {
        async fn find_by_normalized_title(
            &self,
            title_normalized: &str,
        ) -> Result<Option<CatalogEntry>> {
            Ok(self.rows.lock().unwrap().get(title_normalized).cloned())
        }

        async fn find_by_id(&self, hevy_template_id: &str) -> Result<Option<CatalogEntry>> {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .values()
                .find(|c| c.hevy_template_id == hevy_template_id)
                .cloned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_doubles::*;
    use super::*;

    fn seeded_aliases() -> InMemoryAliasStore {
        let store = InMemoryAliasStore::default();
        {
            let mut rows = store.rows.lock().unwrap();
            rows.insert(
                "back squat".to_string(),
                AliasEntry {
                    canonical_title: "Squat (Barbell)".to_string(),
                    hevy_template_id: "SQUAT-ID".to_string(),
                },
            );
            rows.insert(
                "x 7 rounds rotating".to_string(),
                AliasEntry {
                    canonical_title: SKIP_SENTINEL.to_string(),
                    hevy_template_id: String::new(),
                },
            );
        }
        store
    }

    #[tokio::test]
    async fn alias_exact_hit_resolves_and_recovers_is_loaded_type_from_catalog() {
        let aliases = seeded_aliases();
        let catalog = InMemoryCatalogStore::default();
        catalog.rows.lock().unwrap().insert(
            "squat barbell".to_string(),
            CatalogEntry {
                hevy_template_id: "SQUAT-ID".to_string(),
                title: "Squat (Barbell)".to_string(),
                is_loaded_type: true,
                kind: "weight_reps".to_string(),
            },
        );
        let res = resolve("Back Squat", &aliases, &catalog).await.unwrap();
        match res {
            Resolution::Resolved {
                hevy_template_id,
                is_loaded_type,
                ..
            } => {
                assert_eq!(hevy_template_id, "SQUAT-ID");
                assert!(is_loaded_type);
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn skip_sentinel_excludes_without_being_unmapped() {
        let aliases = seeded_aliases();
        let catalog = InMemoryCatalogStore::default();
        let res = resolve("X 7 rounds rotating", &aliases, &catalog)
            .await
            .unwrap();
        assert!(matches!(res, Resolution::Skip));
    }

    /// Mirrors DynamoDB, which rejects an empty string as a key attribute value with a
    /// `ValidationException`. The in-memory doubles above accept it, which is how the crash below
    /// got past the unit tests.
    struct RejectsEmptyKeys;

    #[async_trait]
    impl AliasStore for RejectsEmptyKeys {
        async fn get(&self, alias: &str) -> Result<Option<AliasEntry>> {
            assert!(!alias.is_empty(), "GetItem with an empty alias key");
            Ok(None)
        }
        async fn put(&self, alias: &str, _: AliasEntry, _: &str) -> Result<()> {
            assert!(!alias.is_empty(), "PutItem with an empty alias key");
            Ok(())
        }
    }

    #[async_trait]
    impl CatalogStore for RejectsEmptyKeys {
        async fn find_by_normalized_title(&self, title: &str) -> Result<Option<CatalogEntry>> {
            assert!(
                !title.is_empty(),
                "Query with an empty title_normalized key"
            );
            Ok(None)
        }
        async fn find_by_id(&self, id: &str) -> Result<Option<CatalogEntry>> {
            assert!(!id.is_empty(), "GetItem with an empty id key");
            Ok(None)
        }
    }

    #[tokio::test]
    async fn a_key_with_nothing_left_after_normalizing_is_skipped_without_a_lookup() {
        // 2026-10-15 and 2026-10-21: a WOD line of just "-" or "+" normalized to "", the alias
        // GetItem failed with a ValidationException, and the error aborted the whole day's sync
        // (all three tracks, three times over with Lambda's retries).
        for key in ["-", "+", "", "  ", "—", "*"] {
            let res = resolve(key, &RejectsEmptyKeys, &RejectsEmptyKeys)
                .await
                .unwrap();
            assert!(matches!(res, Resolution::Skip), "{key:?} -> {res:?}");
        }
    }

    #[test]
    fn a_leading_single_is_dropped_and_still_gets_the_full_expansion() {
        // "Single" marks per-limb reps, not a different movement, so the catalog title never has
        // it. Recursing on the stripped key is what reaches Hevy's spelled-out title: the useful
        // candidate is "dumbbell step ups", not merely "db step ups".
        let got = resolution_candidates("single db step ups");
        assert!(got.contains(&"db step ups".to_string()), "{got:?}");
        assert!(got.contains(&"dumbbell step ups".to_string()), "{got:?}");
        assert!(got.contains(&"step ups dumbbell".to_string()), "{got:?}");
    }

    #[test]
    fn single_arm_and_single_leg_compounds_are_never_broken_up() {
        // These are whole qualifiers Hevy spells out itself, and ABBREVIATIONS expands "sa"/"sl"
        // into them. Dropping the word would undo that and match the two-limb movement — a wrong
        // answer rather than no answer, so it must not be offered as a candidate at all.
        for key in ["single arm press", "single leg romanian deadlift"] {
            let got = resolution_candidates(key);
            let stripped = key.strip_prefix("single ").unwrap();
            assert!(
                !got.contains(&stripped.to_string()),
                "{key:?} must not offer {stripped:?}: {got:?}"
            );
        }
    }

    #[test]
    fn a_bare_single_word_is_left_alone() {
        // "Singles" is its own movement (jump-rope singles) and a lone "single" has nothing to
        // qualify — neither should be stripped into an empty or unrelated key.
        assert!(!resolution_candidates("singles").contains(&String::new()));
        assert!(!resolution_candidates("single").contains(&String::new()));
    }

    #[tokio::test]
    async fn plural_drift_now_auto_resolves_via_variant() {
        // Previously "Push-Ups" (plural) missed catalog "Push Up" and needed a hand-seeded alias;
        // the smarter resolver now bridges singular/plural drift automatically and writes the hit
        // back as an `auto-resolved-variant` alias.
        let aliases = InMemoryAliasStore::default();
        let catalog = InMemoryCatalogStore::default();
        catalog.rows.lock().unwrap().insert(
            "push up".to_string(),
            CatalogEntry {
                hevy_template_id: "PUSHUP-ID".to_string(),
                title: "Push Up".to_string(),
                is_loaded_type: false,
                kind: "reps_only".to_string(),
            },
        );
        let res = resolve("Push-Ups", &aliases, &catalog).await.unwrap();
        assert!(matches!(res, Resolution::Resolved { .. }));
        let writes = aliases.writes.lock().unwrap();
        assert_eq!(
            writes[0],
            ("push ups".to_string(), "auto-resolved-variant".to_string())
        );
    }

    #[tokio::test]
    async fn abbreviation_expands_to_hevy_prefix_word_order() {
        // "DB Floor Press" -> catalog "Dumbbell Floor Press" (equipment-first spelling).
        let aliases = InMemoryAliasStore::default();
        let catalog = InMemoryCatalogStore::default();
        catalog.rows.lock().unwrap().insert(
            "dumbbell floor press".to_string(),
            CatalogEntry {
                hevy_template_id: "DFP".to_string(),
                title: "Dumbbell Floor Press".to_string(),
                is_loaded_type: true,
                kind: "weight_reps".to_string(),
            },
        );
        let res = resolve("DB Floor Press", &aliases, &catalog).await.unwrap();
        assert!(matches!(res, Resolution::Resolved { .. }));
        assert!(aliases.rows.lock().unwrap().contains_key("db floor press"));
    }

    #[tokio::test]
    async fn abbreviation_expands_to_hevy_suffix_word_order() {
        // "DB Sumo Squat" -> catalog "Sumo Squat (Dumbbell)" (normalized "sumo squat dumbbell").
        let aliases = InMemoryAliasStore::default();
        let catalog = InMemoryCatalogStore::default();
        catalog.rows.lock().unwrap().insert(
            "sumo squat dumbbell".to_string(),
            CatalogEntry {
                hevy_template_id: "SSD".to_string(),
                title: "Sumo Squat (Dumbbell)".to_string(),
                is_loaded_type: true,
                kind: "weight_reps".to_string(),
            },
        );
        let res = resolve("DB Sumo Squat", &aliases, &catalog).await.unwrap();
        assert!(matches!(res, Resolution::Resolved { .. }));
    }

    #[tokio::test]
    async fn fuzzy_fallback_still_returns_unmapped_on_a_genuine_miss() {
        let aliases = InMemoryAliasStore::default();
        let catalog = InMemoryCatalogStore::default();
        catalog.rows.lock().unwrap().insert(
            "wall ball".to_string(),
            CatalogEntry {
                hevy_template_id: "WB".to_string(),
                title: "Wall Ball".to_string(),
                is_loaded_type: true,
                kind: "weight_reps".to_string(),
            },
        );
        let res = resolve("Renegade Rows", &aliases, &catalog).await.unwrap();
        assert!(matches!(res, Resolution::Unmapped));
    }

    #[tokio::test]
    async fn catalog_hit_writes_back_auto_resolved_alias() {
        let aliases = InMemoryAliasStore::default();
        let catalog = InMemoryCatalogStore::default();
        catalog.rows.lock().unwrap().insert(
            "wall ball".to_string(),
            CatalogEntry {
                hevy_template_id: "WALLBALL-ID".to_string(),
                title: "Wall Ball".to_string(),
                is_loaded_type: true,
                kind: "weight_reps".to_string(),
            },
        );
        let res = resolve("Wall Ball", &aliases, &catalog).await.unwrap();
        assert!(matches!(res, Resolution::Resolved { .. }));
        let writes = aliases.writes.lock().unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(
            writes[0],
            ("wall ball".to_string(), "auto-resolved".to_string())
        );
    }

    #[tokio::test]
    async fn true_miss_is_unmapped() {
        let aliases = InMemoryAliasStore::default();
        let catalog = InMemoryCatalogStore::default();
        let res = resolve("Some Totally Unknown Movement", &aliases, &catalog)
            .await
            .unwrap();
        assert!(matches!(res, Resolution::Unmapped));
    }

    #[tokio::test]
    async fn abbreviation_kept_literal_reaches_the_db_suffix_custom_template_style() {
        // The live catalog has custom templates titled "Front Squat (DB)" and "Push Press (DB)"
        // (normalized "front squat db"). Expanding "db" -> "dumbbell" can never reach them, so
        // "DB Front Squat" on 2026-09-10 was reported unmapped with the right template sitting in
        // the catalog. Keeping the abbreviation literal and moving it to the end finds it.
        let aliases = InMemoryAliasStore::default();
        let catalog = InMemoryCatalogStore::default();
        catalog.rows.lock().unwrap().insert(
            "front squat db".to_string(),
            CatalogEntry {
                hevy_template_id: "FSDB".to_string(),
                title: "Front Squat (DB)".to_string(),
                is_loaded_type: true,
                kind: "weight_reps".to_string(),
            },
        );
        let res = resolve("DB Front Squat", &aliases, &catalog).await.unwrap();
        match res {
            Resolution::Resolved {
                hevy_template_id, ..
            } => assert_eq!(hevy_template_id, "FSDB"),
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn expanded_forms_are_still_tried_before_the_literal_suffix() {
        // Adding the literal-suffix form must not reorder the existing candidates: an exact
        // "Dumbbell X" / "X Dumbbell" catalog title should still win if both exist.
        let c = resolution_candidates("db push press");
        let pos = |s: &str| {
            c.iter()
                .position(|x| x == s)
                .unwrap_or_else(|| panic!("{s} missing"))
        };
        assert!(pos("dumbbell push press") < pos("push press db"));
        assert!(pos("push press dumbbell") < pos("push press db"));
    }

    #[tokio::test]
    async fn alias_hit_recovers_kind_from_the_catalog() {
        // Alias rows don't store the Hevy type; it has to come back from the catalog row so
        // load_calc can check the prescription against it.
        let aliases = InMemoryAliasStore::default();
        aliases.rows.lock().unwrap().insert(
            "ski".to_string(),
            AliasEntry {
                canonical_title: "Skiing".to_string(),
                hevy_template_id: "SKI-ID".to_string(),
            },
        );
        let catalog = InMemoryCatalogStore::default();
        catalog.rows.lock().unwrap().insert(
            "skiing".to_string(),
            CatalogEntry {
                hevy_template_id: "SKI-ID".to_string(),
                title: "Skiing".to_string(),
                is_loaded_type: false,
                kind: "duration".to_string(),
            },
        );
        match resolve("SKI", &aliases, &catalog).await.unwrap() {
            Resolution::Resolved { kind, .. } => assert_eq!(kind, "duration"),
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    fn alias(rows: &[(&str, &str, &str)]) -> InMemoryAliasStore {
        let store = InMemoryAliasStore::default();
        for (key, title, id) in rows {
            store.rows.lock().unwrap().insert(
                key.to_string(),
                AliasEntry {
                    canonical_title: title.to_string(),
                    hevy_template_id: id.to_string(),
                },
            );
        }
        store
    }

    #[tokio::test]
    async fn a_fuzzy_candidate_can_hit_a_curated_alias_not_just_the_catalog() {
        // 2026-10-13/19/22/23: "KB Farmer Walk" was unmapped four times while the alias table held
        // "kb farmer carry" -> Farmers Walk. No catalog title spells it either way, so only an
        // alias lookup on the walk/carry synonym could reach it.
        let aliases = alias(&[("kb farmer carry", "Farmers Walk", "FW")]);
        let catalog = InMemoryCatalogStore::default();
        match resolve("KB Farmer Walk", &aliases, &catalog).await.unwrap() {
            Resolution::Resolved {
                hevy_template_id, ..
            } => assert_eq!(hevy_template_id, "FW"),
            other => panic!("expected Resolved, got {other:?}"),
        }
        assert!(aliases.writes.lock().unwrap().contains(&(
            "kb farmer walk".to_string(),
            "auto-resolved-variant".to_string()
        )));
    }

    #[tokio::test]
    async fn droppable_qualifiers_reach_the_plain_movement() {
        // "DB Alt Snatch" (2026-10-14), "Max Power Cleans" (2026-10-14), "Heavy DB Box Step Overs"
        // (2026-10-21), "DB SA thruster" (2026-10-10): each names a movement already mapped.
        let aliases = alias(&[
            ("db snatch", "Dumbbell Snatch", "DBS"),
            ("db thruster", "DB Thruster", "DBT"),
            ("db box step overs", "Dumbbell Step Up", "DSU"),
        ]);
        let catalog = InMemoryCatalogStore::default();
        catalog.rows.lock().unwrap().insert(
            "power clean".to_string(),
            CatalogEntry {
                hevy_template_id: "PC".to_string(),
                title: "Power Clean".to_string(),
                is_loaded_type: true,
                kind: "weight_reps".to_string(),
            },
        );
        for (key, want) in [
            ("DB Alt Snatch", "DBS"),
            ("Max Power Cleans", "PC"),
            ("Heavy DB Box Step Overs", "DSU"),
            ("DB SA thruster", "DBT"),
        ] {
            match resolve(key, &aliases, &catalog).await.unwrap() {
                Resolution::Resolved {
                    hevy_template_id, ..
                } => assert_eq!(hevy_template_id, want, "{key}"),
                other => panic!("{key}: expected Resolved, got {other:?}"),
            }
        }
    }

    #[test]
    fn single_arm_spellings_are_tried_before_sa_is_dropped() {
        let c = resolution_candidates("sa lateral raise");
        let pos = |s: &str| {
            c.iter()
                .position(|x| x == s)
                .unwrap_or_else(|| panic!("{s} missing from {c:?}"))
        };
        assert!(pos("single arm lateral raise") < pos("lateral raise"));
    }

    #[tokio::test]
    async fn a_skip_sentinel_on_a_fuzzy_neighbour_does_not_skip_the_key() {
        let aliases = alias(&[("rounds", SKIP_SENTINEL, "")]);
        let catalog = InMemoryCatalogStore::default();
        let res = resolve("Max Rounds", &aliases, &catalog).await.unwrap();
        assert!(matches!(res, Resolution::Unmapped), "{res:?}");
    }
}
