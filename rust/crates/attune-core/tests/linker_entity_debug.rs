//! Debug helper — prints extract_entities output for every golden fixture.
//! Run with `cargo test -p attune-core --test linker_entity_debug -- --nocapture --ignored`.

use std::fs;
use std::path::PathBuf;

#[test]
#[ignore]
fn dump_all_golden_entities() {
    let mut corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    corpus.push("tests/corpora/linker_golden");
    let mut files: Vec<_> = fs::read_dir(&corpus)
        .expect("read corpus dir")
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .map(|s| s == "yaml")
                .unwrap_or(false)
        })
        .collect();
    files.sort_by_key(|e| e.path());
    for entry in files {
        let raw = fs::read_to_string(entry.path()).unwrap();
        let yaml: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
        let id = yaml["id"].as_str().unwrap_or("?");
        for side in ["doc_a", "doc_b"] {
            let content = yaml[side]["content"].as_str().unwrap_or("");
            println!("=== {id} :: {side} ===");
            let ents = attune_core::entities::extract_entities(content);
            for e in &ents {
                println!("  {:?} -> {}", e.kind, e.value);
            }
        }
    }
}
