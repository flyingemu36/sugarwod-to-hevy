// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi
//
// Full offline pipeline replay: parse -> correlate -> resolve -> load, run against read-only
// snapshots of the real Prod DynamoDB tables instead of live DynamoDB. Alias write-backs are
// recorded in memory and never sent anywhere, so replaying a day cannot mutate Prod.
//
//   aws dynamodb scan --table-name SugarwodToHevy-HevyExerciseCatalog-Prod > catalog.json
//   cargo run -p swh-core --example replay_resolve -- <dir-with-dumps> wod-*.json
//   cargo run -p swh-core --example replay_resolve -- <dir-with-dumps> --keys "db front squat" ...
//
// Catalog keys are recomputed with the current `normalize` rather than read from the dump, so a
// normalization change is previewed as it will look after the next refresh-catalog run. Pass
// `--seed <exercise-translation.json>` right after the dump dir to overlay the dictionary the way
// `cdk/scripts/seed-aliases.ts` would write it, previewing an alias change before it is seeded.

use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use swh_core::error::Result;
use swh_core::hevy::model::is_loaded_exercise_type;
use swh_core::matching::build_routine_exercises;
use swh_core::matching::load_calc::MaxStore;
use swh_core::matching::normalize::normalize;
use swh_core::matching::resolver::{
    resolve, AliasEntry, AliasStore, CatalogEntry, CatalogStore, SKIP_SENTINEL,
};
use swh_core::sugarwod::model::SugarWodResponse;

fn s(item: &Value, key: &str) -> String {
    item.get(key)
        .and_then(|v| v.get("S"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn scan_items(path: &str) -> Vec<Value> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let v: Value = serde_json::from_str(&text).expect("parse scan dump");
    v.get("Items")
        .and_then(|i| i.as_array())
        .cloned()
        .unwrap_or_default()
}

struct SnapshotAliases {
    rows: Mutex<HashMap<String, AliasEntry>>,
    writes: Mutex<Vec<(String, String, String)>>, // (alias, canonical, source)
}

#[async_trait]
impl AliasStore for SnapshotAliases {
    async fn get(&self, alias: &str) -> Result<Option<AliasEntry>> {
        Ok(self.rows.lock().unwrap().get(alias).cloned())
    }
    async fn put(&self, alias: &str, entry: AliasEntry, source: &str) -> Result<()> {
        self.writes.lock().unwrap().push((
            alias.to_string(),
            entry.canonical_title.clone(),
            source.to_string(),
        ));
        self.rows.lock().unwrap().insert(alias.to_string(), entry);
        Ok(())
    }
}

struct SnapshotCatalog {
    by_norm: HashMap<String, CatalogEntry>,
    by_id: HashMap<String, CatalogEntry>,
}

#[async_trait]
impl CatalogStore for SnapshotCatalog {
    async fn find_by_normalized_title(&self, t: &str) -> Result<Option<CatalogEntry>> {
        Ok(self.by_norm.get(t).cloned())
    }
    async fn find_by_id(&self, id: &str) -> Result<Option<CatalogEntry>> {
        Ok(self.by_id.get(id).cloned())
    }
}

struct SnapshotMaxes {
    by_id: HashMap<String, f64>,
}

#[async_trait]
impl MaxStore for SnapshotMaxes {
    async fn get_max_kg(&self, exercise_template_id: &str) -> Result<Option<f64>> {
        Ok(self.by_id.get(exercise_template_id).copied())
    }
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("dump dir");

    let aliases = SnapshotAliases {
        rows: Mutex::new(
            scan_items(&format!("{dir}/ExerciseAlias.json"))
                .iter()
                .map(|i| {
                    (
                        s(i, "alias"),
                        AliasEntry {
                            canonical_title: s(i, "canonical_title"),
                            hevy_template_id: s(i, "hevy_template_id"),
                        },
                    )
                })
                .collect(),
        ),
        writes: Mutex::new(Vec::new()),
    };

    // Sorted the same way `DynamoCatalogStore::find_by_normalized_title` now sorts a colliding
    // GSI query — built-in before custom, then by id — so the snapshot replay reproduces the
    // deployed tiebreak instead of whatever order the scan happened to return.
    let mut catalog_items = scan_items(&format!("{dir}/HevyExerciseCatalog.json"));
    catalog_items.sort_by_key(|i| {
        (
            i.get("is_custom")
                .and_then(|v| v.get("BOOL"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            s(i, "id"),
        )
    });
    let mut by_norm: HashMap<String, CatalogEntry> = HashMap::new();
    let mut by_id = HashMap::new();
    for i in &catalog_items {
        let e = CatalogEntry {
            hevy_template_id: s(i, "id"),
            title: s(i, "title"),
            is_loaded_type: is_loaded_exercise_type(&s(i, "type")),
            kind: s(i, "type"),
        };
        by_id.insert(e.hevy_template_id.clone(), e.clone());
        by_norm.entry(normalize(&s(i, "title"))).or_insert(e);
    }
    let catalog = SnapshotCatalog { by_norm, by_id };

    let mut rest: Vec<String> = args.collect();
    if rest.first().map(|a| a == "--seed").unwrap_or(false) {
        let path = rest[1].clone();
        rest.drain(..2);
        let dict: serde_json::Map<String, Value> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let mut rows = aliases.rows.lock().unwrap();
        let mut unresolved = Vec::new();
        for (raw_alias, canonical) in dict {
            let canonical = canonical.as_str().unwrap_or_default().to_string();
            let entry = if canonical == "UNDEFINED" {
                AliasEntry {
                    canonical_title: SKIP_SENTINEL.to_string(),
                    hevy_template_id: String::new(),
                }
            } else if let Some(c) = catalog.by_norm.get(&normalize(&canonical)) {
                AliasEntry {
                    canonical_title: canonical.clone(),
                    hevy_template_id: c.hevy_template_id.clone(),
                }
            } else {
                unresolved.push(format!("{raw_alias:?} -> {canonical:?}"));
                continue;
            };
            rows.insert(normalize(&raw_alias), entry);
        }
        println!("seeded from {path}; unresolved: {unresolved:?}");
    }

    let maxes = SnapshotMaxes {
        by_id: scan_items(&format!("{dir}/PersonalMax.json"))
            .iter()
            .filter_map(|i| {
                let id = s(i, "exercise_template_id");
                let kg = i
                    .get("max_weight_kg")
                    .and_then(|v| v.get("N"))
                    .and_then(|v| v.as_str())
                    .and_then(|v| v.parse::<f64>().ok())?;
                Some((id, kg))
            })
            .collect(),
    };

    if rest.first().map(|a| a == "--keys").unwrap_or(false) {
        println!(
            "catalog={} aliases={}\n",
            catalog_items.len(),
            aliases.rows.lock().unwrap().len()
        );
        for key in &rest[1..] {
            let r = resolve(key, &aliases, &catalog).await.unwrap();
            println!("{key:40} -> {r:?}");
        }
        return;
    }

    for path in &rest {
        let resp: SugarWodResponse =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        println!("\n########## {path} ##########");
        for w in &resp.data {
            let (matched, report) =
                build_routine_exercises(&w.description, &w.movements, &aliases, &catalog, &maxes)
                    .await
                    .unwrap();
            println!("\n=== {} ===", w.title);
            println!("  RESOLVED ({}):", matched.len());
            for m in &matched {
                let title = catalog
                    .by_id
                    .get(&m.hevy_template_id)
                    .map(|c| c.title.clone())
                    .unwrap_or_default();
                let kgs: Vec<String> = m
                    .sets
                    .iter()
                    .map(|s| {
                        let mut parts = Vec::new();
                        if let Some(r) = s.reps {
                            parts.push(format!("{r}r"));
                        }
                        if let Some(d) = s.distance_meters {
                            parts.push(format!("{d}m"));
                        }
                        if let Some(d) = s.duration_seconds {
                            parts.push(format!("{d}s"));
                        }
                        if let Some(w) = s.weight_kg {
                            parts.push(format!("{w:.1}kg"));
                        }
                        if parts.is_empty() {
                            "-".into()
                        } else {
                            parts.join("@")
                        }
                    })
                    .collect();
                println!(
                    "    {:?} -> {:?}  sets={} weights={:?}",
                    m.notes,
                    title,
                    m.sets.len(),
                    kgs
                );
            }
            if !report.unmapped.is_empty() {
                println!(
                    "  UNMAPPED ({}): {:?}",
                    report.unmapped.len(),
                    report.unmapped
                );
            }
            if !report.unsupported_fields.is_empty() {
                println!(
                    "  FIELD_DROPPED ({}): {:?}",
                    report.unsupported_fields.len(),
                    report.unsupported_fields
                );
            }
            if !report.load_unknown.is_empty() {
                println!(
                    "  LOAD_UNKNOWN ({}): {:?}",
                    report.load_unknown.len(),
                    report.load_unknown
                );
            }
        }
    }

    let w = aliases.writes.lock().unwrap();
    println!(
        "\n### alias write-backs that would have happened ({}):",
        w.len()
    );
    for (a, c, src) in w.iter() {
        println!("    {a:40} -> {c:35} [{src}]");
    }
    let skips = aliases
        .rows
        .lock()
        .unwrap()
        .values()
        .filter(|e| e.canonical_title == SKIP_SENTINEL)
        .count();
    println!("### ({skips} __SKIP__ aliases in the snapshot)");
    let _ = normalize("");
}
