// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Eamun Rahimi
//
// Offline replay harness: runs the real `parse_description` + `correlator` over saved SugarWOD
// API responses and prints, per workout, every parsed line with its set prescription and the
// resolution key that `resolver` would look up. Resolution itself needs DynamoDB, so this
// stops at the key — which is exactly the boundary where parse bugs live.
//
//   cargo run -p swh-core --example replay_parse -- wod-20260912.json ...

use swh_core::matching::correlator::resolution_keys;
use swh_core::matching::description_parser::parse_description;
use swh_core::sugarwod::model::SugarWodResponse;

fn main() {
    for path in std::env::args().skip(1) {
        let text = std::fs::read_to_string(&path).expect("read fixture");
        let resp: SugarWodResponse = serde_json::from_str(&text).expect("parse fixture");
        println!("\n########## {path} ##########");
        for w in &resp.data {
            println!("\n=== {} / {} ===", w.track, w.title);
            for l in w.description.replace("\\n", "\n").lines() {
                println!("  raw| {l}");
            }
            let lines = parse_description(&w.description);
            let keys = resolution_keys(&lines, &w.movements);
            println!("  --> {} parsed line(s)", lines.len());
            for (l, k) in lines.iter().zip(keys.iter()) {
                let sets: Vec<String> = l
                    .sets
                    .iter()
                    .map(|s| {
                        let mut parts = Vec::new();
                        if let Some(r) = s.reps {
                            parts.push(format!("{r}reps"));
                        }
                        if let Some(d) = s.distance_meters {
                            parts.push(format!("{d}m"));
                        }
                        if let Some(d) = s.duration_seconds {
                            parts.push(format!("{d}s"));
                        }
                        if let Some(p) = s.percent_1rm {
                            parts.push(format!("{:.0}%1rm", p * 100.0));
                        }
                        if let Some(wt) = s.explicit_weight_lb {
                            parts.push(format!("{wt}lb"));
                        }
                        if parts.is_empty() {
                            "-".to_string()
                        } else {
                            parts.join("+")
                        }
                    })
                    .collect();
                println!(
                    "      name={:?} key={:?} sets[{}]={:?}{}",
                    l.name_text,
                    k,
                    sets.len(),
                    sets,
                    l.rpe_note
                        .as_ref()
                        .map(|n| format!(" rpe={n:?}"))
                        .unwrap_or_default(),
                );
            }
        }
    }
}
