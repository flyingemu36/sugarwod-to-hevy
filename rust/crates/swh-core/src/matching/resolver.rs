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

    if let Some(alias) = aliases.get(&normalized_key).await? {
        if alias.canonical_title == SKIP_SENTINEL {
            return Ok(Resolution::Skip);
        }
        // Alias rows don't carry `is_loaded_type` (it's catalog metadata, not translation
        // data) — recover it with a cheap primary-key lookup now that we have the id.
        let is_loaded_type = catalog
            .find_by_id(&alias.hevy_template_id)
            .await?
            .map(|c| c.is_loaded_type)
            .unwrap_or(false);
        return Ok(Resolution::Resolved {
            hevy_template_id: alias.hevy_template_id,
            is_loaded_type,
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
        });
    }

    // Fuzzy fallback — only after an exact alias AND exact catalog miss, so it can never override
    // an exact match. Bridges the two mechanical miss classes seen in real WOD data: equipment
    // abbreviations ("DB" -> "Dumbbell", tried in both of Hevy's inconsistent word orders) and
    // singular/plural drift ("Burpees" -> "Burpee"). A hit writes back an alias for the original
    // key (source `auto-resolved-variant`, so fuzzy matches are auditable and O(1) next time).
    for candidate in resolution_candidates(&normalized_key) {
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

/// Ordered, de-duplicated fuzzy candidates for an already-normalized key (never the key itself).
/// Expands equipment abbreviations in place ("db floor press" -> "dumbbell floor press", the
/// "Dumbbell X" style) and with the equipment moved to the end ("floor press dumbbell", the
/// "X (Dumbbell)" style — Hevy uses both), and toggles the last token's singular/plural. Garbage
/// candidates are harmless: they simply miss the catalog.
fn resolution_candidates(normalized_key: &str) -> Vec<String> {
    let tokens: Vec<String> = normalized_key
        .split_whitespace()
        .map(String::from)
        .collect();
    if tokens.is_empty() {
        return Vec::new();
    }

    let mut expanded: Vec<String> = Vec::new();
    let mut expansion_words: Vec<String> = Vec::new();
    for tok in &tokens {
        match ABBREVIATIONS.iter().find(|(ab, _)| *ab == tok) {
            Some((_, full)) => {
                for w in full.split_whitespace() {
                    expanded.push(w.to_string());
                    expansion_words.push(w.to_string());
                }
            }
            None => expanded.push(tok.clone()),
        }
    }

    let mut bases: Vec<Vec<String>> = vec![tokens.clone()];
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
    }

    let mut out: Vec<String> = Vec::new();
    let mut push = |form: String| {
        if !form.is_empty() && form != normalized_key && !out.contains(&form) {
            out.push(form);
        }
    };
    for base in &bases {
        push(base.join(" "));
        push(swap_last_token_plurality(base));
    }
    out
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
            },
        );
        let res = resolve("Back Squat", &aliases, &catalog).await.unwrap();
        match res {
            Resolution::Resolved {
                hevy_template_id,
                is_loaded_type,
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
}
