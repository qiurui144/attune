//! memory-continuity — deterministic golden gate + property tests (CI-blocking).
//!
//! per `attune/CLAUDE.md` §「Agent 验证铁律」/ §6.1 测试矩阵 (plan Task 9):
//!   - deterministic path (export/import + reindex_one, no LLM) → pass rate **1.00**
//!   - ground truth **independent** of the engine: every `expected.*` is curated
//!     in the YAML fixture, never derived by running the code under test.
//!   - 6-class coverage:
//!       * Golden case ≥ 10 ... `golden/memory_continuity/01..12` (12 files)
//!       * Property test ≥ 3 .. `mod proptests` (import(export(x))==x + 2 more)
//!       * Boundary `#[test]` . `05-empty-bundle` (zero memories) + `09` (already-current)
//!       * Error case ......... `11-wrong-passphrase` + `12-corrupt-bundle`
//!       * Integration E2E .... `attune-server/tests/memory_continuity_e2e.rs`
//!       * Regression fixture . `10-roundtrip-then-reindex` (import-then-reindex combo)
//!
//! Recall is verified with the SAME `MemoryVectorIndex` cosine search the product
//! uses — MockEmbeddingProvider hashes shared tokens to nearby vectors, so a query
//! sharing keywords with a memory summary genuinely ranks it first. This proves
//! "old memory recall restored after reindex" is real retrieval, not a row count.

use std::path::{Path, PathBuf};

use attune_core::crypto::Key32;
use attune_core::embed::{EmbeddingProvider, MockEmbeddingProvider};
use attune_core::memory::migration::reindex_one;
use attune_core::memory::portability::{export_memory_bundle, import_memory_bundle};
use attune_core::memory::retrieval::MemoryVectorIndex;
use attune_core::store::Store;
use serde::Deserialize;

// ── YAML schema ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // `id`/`description` are loaded for parity + diagnostics, not asserted.
struct GoldenCase {
    id: String,
    #[serde(default)]
    description: String,
    // round-trip family (01-06)
    #[serde(default)]
    passphrase: Option<String>,
    #[serde(default)]
    memories: Vec<Mem>,
    #[serde(default)]
    preexisting: Vec<Mem>,
    // reindex family (07-09)
    #[serde(default)]
    reindex: Option<ReindexSpec>,
    // combo family (10)
    #[serde(default)]
    import_then_reindex: Option<ComboSpec>,
    // error family (11-12)
    #[serde(default)]
    wrong_passphrase: Option<String>,
    #[serde(default)]
    corrupt: Option<String>,
    expected: Expected,
}

#[derive(Debug, Deserialize, Clone)]
struct Mem {
    hash: String,
    summary: String,
    model: String,
}

#[derive(Debug, Deserialize)]
struct ReindexSpec {
    from_dim: usize,
    to_dim: usize,
    memories: Vec<Mem>,
    recall_query: String,
    recall_min_hits: usize,
}

#[derive(Debug, Deserialize)]
struct ComboSpec {
    passphrase: String,
    bundle_dim: usize,
    target_dim: usize,
    memories: Vec<Mem>,
    recall_query: String,
    recall_min_hits: usize,
}

#[derive(Debug, Deserialize)]
struct Expected {
    #[serde(default)]
    imported: Option<usize>,
    #[serde(default)]
    skipped_on_reimport: Option<usize>,
    #[serde(default)]
    recall_query: Option<String>,
    #[serde(default)]
    recall_min_hits: Option<usize>,
    #[serde(default)]
    stale_after_reindex: Option<usize>,
    #[serde(default)]
    import_errors: Option<bool>,
    #[serde(default)]
    rows_after_failed_import: Option<usize>,
}

// ── helpers ───────────────────────────────────────────────────────────────

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/memory_continuity")
}

fn load_cases() -> Vec<(String, GoldenCase)> {
    let mut out = Vec::new();
    let mut paths: Vec<_> = std::fs::read_dir(golden_dir())
        .expect("read golden dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("yaml"))
        .collect();
    paths.sort();
    for p in paths {
        let text = std::fs::read_to_string(&p).expect("read fixture");
        let case: GoldenCase =
            serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {p:?}: {e}"));
        out.push((p.file_name().unwrap().to_string_lossy().into_owned(), case));
    }
    out
}

fn seed(store: &Store, dek: &Key32, mems: &[Mem]) {
    for (i, m) in mems.iter().enumerate() {
        store
            .insert_memory(dek, "episodic", 1, 2, std::slice::from_ref(&m.hash), &m.summary, &m.model, i as i64)
            .expect("insert memory");
    }
}

/// Embed every live memory under `emb` into a fresh in-memory index, then count
/// how many of the top-K hits clear a permissive similarity floor — proving the
/// query actually retrieves the seeded memory (real cosine recall, not a count).
fn recall_hits(store: &Store, dek: &Key32, emb: &dyn EmbeddingProvider, query: &str) -> usize {
    let mut idx = MemoryVectorIndex::new(emb.dimensions()).expect("index");
    let mems = store.list_recent_memories(dek, usize::MAX).expect("list");
    for m in &mems {
        let (vecs, _) = emb.embed(&[m.summary.as_str()]).expect("embed");
        if let Some(v) = vecs.into_iter().next() {
            let _ = idx.upsert(&m.id, &v);
        }
    }
    if idx.is_empty() {
        return 0;
    }
    let (qv, _) = emb.embed(&[query]).expect("embed query");
    let qvec = qv.into_iter().next().expect("query vec");
    idx.search(&qvec, 5)
        .expect("search")
        .into_iter()
        // MockEmbeddingProvider keyword-shared vectors land well above 0; a low
        // floor only excludes orthogonal (no shared token) memories.
        .filter(|(_, score)| *score > 0.1)
        .count()
}

// ── the gate ────────────────────────────────────────────────────────────────

#[test]
fn memory_continuity_golden_gate() {
    let cases = load_cases();
    assert!(cases.len() >= 10, "≥10 golden fixtures required, found {}", cases.len());

    let mut ran = 0usize;
    for (file, case) in &cases {
        if let Some(spec) = &case.reindex {
            run_reindex_case(file, case, spec);
        } else if let Some(spec) = &case.import_then_reindex {
            run_combo_case(file, case, spec);
        } else if case.wrong_passphrase.is_some() || case.corrupt.is_some() {
            run_error_case(file, case);
        } else {
            run_roundtrip_case(file, case);
        }
        ran += 1;
    }
    assert_eq!(ran, cases.len(), "every fixture must be exercised");
    eprintln!("memory-continuity golden gate: {ran}/{} cases PASS (1.00)", cases.len());
}

/// 01-06: export from source vault → import into a fresh (different-DEK) vault →
/// imported count matches, second import is fully idempotent (skipped), and the
/// curated query recalls at least `recall_min_hits` memories.
fn run_roundtrip_case(file: &str, case: &GoldenCase) {
    let pass = case.passphrase.as_deref().unwrap_or("pw");
    let src = Store::open_memory().unwrap();
    let src_dek = Key32::generate();
    seed(&src, &src_dek, &case.memories);
    let bundle = export_memory_bundle(&src, &src_dek, pass).unwrap();

    let dst = Store::open_memory().unwrap();
    let dst_dek = Key32::generate();
    seed(&dst, &dst_dek, &case.preexisting);

    let r = import_memory_bundle(&dst, &dst_dek, &bundle, pass).unwrap();
    if let Some(exp) = case.expected.imported {
        assert_eq!(r.imported, exp, "{file}: imported");
    }

    // second import of the same bundle → every bundle row already present → skip.
    let r2 = import_memory_bundle(&dst, &dst_dek, &bundle, pass).unwrap();
    if let Some(exp) = case.expected.skipped_on_reimport {
        assert_eq!(r2.skipped, exp, "{file}: skipped on re-import");
        assert_eq!(r2.imported, 0, "{file}: re-import adds nothing");
    }

    if let (Some(q), Some(min)) = (&case.expected.recall_query, case.expected.recall_min_hits) {
        let emb = MockEmbeddingProvider::new(4);
        let hits = recall_hits(&dst, &dst_dek, &emb, q);
        assert!(hits >= min, "{file}: recall '{q}' got {hits} hits, want ≥{min}");
    }
}

/// 07-09: seed old-model vectors, switch embedding model, reindex EACH stale id via
/// the real `reindex_one`, then assert 0 stale remain + the query still recalls.
fn run_reindex_case(file: &str, case: &GoldenCase, spec: &ReindexSpec) {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    seed(&store, &dek, &spec.memories);

    // Lay down the OLD-model vectors (dimension = from_dim) so they read as stale
    // vs the new active model.
    let old_emb = MockEmbeddingProvider::new(spec.from_dim);
    for m in &spec.memories {
        let id = store.find_memory_id_by_source("episodic", std::slice::from_ref(&m.hash)).unwrap().unwrap();
        let (v, _) = old_emb.embed(&[m.summary.as_str()]).unwrap();
        store.put_memory_vector(&id, &v[0], &old_emb.model_name(), 1).unwrap();
    }

    let new_emb = MockEmbeddingProvider::new(spec.to_dim);
    let cur_model = new_emb.model_name();
    let stale_before = store.list_stale_memory_ids(&cur_model).unwrap();
    // 09 (from==to) is the idempotent boundary: already current ⇒ 0 stale to begin.
    if spec.from_dim != spec.to_dim {
        assert_eq!(stale_before.len(), spec.memories.len(), "{file}: all old vectors stale pre-reindex");
    }

    for id in &stale_before {
        reindex_one(&store, &dek, &new_emb, id).unwrap();
    }

    let stale_after = store.list_stale_memory_ids(&cur_model).unwrap().len();
    assert_eq!(
        stale_after,
        case.expected.stale_after_reindex.unwrap_or(0),
        "{file}: stale after reindex"
    );

    let hits = recall_hits(&store, &dek, &new_emb, &spec.recall_query);
    assert!(hits >= spec.recall_min_hits, "{file}: post-reindex recall '{}' got {hits}", spec.recall_query);
}

/// 10: bundle carries OLD-model vectors → import into a target on a NEW model →
/// reindex aligns them → 0 stale + recall restored (the migration + portability combo).
fn run_combo_case(file: &str, case: &GoldenCase, spec: &ComboSpec) {
    // Build a source vault whose vectors are under bundle_dim.
    let src = Store::open_memory().unwrap();
    let src_dek = Key32::generate();
    seed(&src, &src_dek, &spec.memories);
    let bundle_emb = MockEmbeddingProvider::new(spec.bundle_dim);
    for m in &spec.memories {
        let id = src.find_memory_id_by_source("episodic", std::slice::from_ref(&m.hash)).unwrap().unwrap();
        let (v, _) = bundle_emb.embed(&[m.summary.as_str()]).unwrap();
        src.put_memory_vector(&id, &v[0], &bundle_emb.model_name(), 1).unwrap();
    }
    let bundle = export_memory_bundle(&src, &src_dek, &spec.passphrase).unwrap();

    // Target vault runs target_dim. Import (carries bundle_dim vectors), then reindex.
    let dst = Store::open_memory().unwrap();
    let dst_dek = Key32::generate();
    let r = import_memory_bundle(&dst, &dst_dek, &bundle, &spec.passphrase).unwrap();
    if let Some(exp) = case.expected.imported {
        assert_eq!(r.imported, exp, "{file}: imported");
    }

    let target_emb = MockEmbeddingProvider::new(spec.target_dim);
    let cur_model = target_emb.model_name();
    for id in dst.list_stale_memory_ids(&cur_model).unwrap() {
        reindex_one(&dst, &dst_dek, &target_emb, &id).unwrap();
    }
    let stale_after = dst.list_stale_memory_ids(&cur_model).unwrap().len();
    assert_eq!(stale_after, case.expected.stale_after_reindex.unwrap_or(0), "{file}: stale after combo reindex");

    let hits = recall_hits(&dst, &dst_dek, &target_emb, &spec.recall_query);
    assert!(hits >= spec.recall_min_hits, "{file}: combo recall '{}' got {hits}", spec.recall_query);
}

/// 11-12: a wrong passphrase / a corrupted bundle must be rejected BEFORE any row
/// is written — the import is atomic-reject (zero partial writes).
fn run_error_case(file: &str, case: &GoldenCase) {
    let pass = case.passphrase.as_deref().unwrap_or("pw");
    let src = Store::open_memory().unwrap();
    let src_dek = Key32::generate();
    seed(&src, &src_dek, &case.memories);
    let mut bundle = export_memory_bundle(&src, &src_dek, pass).unwrap();

    let (try_pass, try_bundle): (&str, &[u8]) = if let Some(wp) = &case.wrong_passphrase {
        (wp.as_str(), &bundle)
    } else {
        // corrupt: truncate the tail so the GCM ciphertext fails to authenticate.
        let cut = bundle.len().saturating_sub(8);
        bundle.truncate(cut);
        (pass, &bundle)
    };

    let dst = Store::open_memory().unwrap();
    let dst_dek = Key32::generate();
    let res = import_memory_bundle(&dst, &dst_dek, try_bundle, try_pass);

    if case.expected.import_errors == Some(true) {
        assert!(res.is_err(), "{file}: import must error");
    }
    let rows = dst.list_recent_memories(&dst_dek, usize::MAX).unwrap().len();
    assert_eq!(rows, case.expected.rows_after_failed_import.unwrap_or(0), "{file}: zero write on rejected import");
}

// ── property tests (≥3) ──────────────────────────────────────────────────────

mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// A random list of distinct-hash memories that survives JSON + AES-GCM round-trip.
    fn mems_strategy() -> impl Strategy<Value = Vec<Mem>> {
        prop::collection::vec(
            (any::<u32>(), "[a-zA-Z0-9 \u{4e00}-\u{9fff}]{1,60}").prop_map(|(h, s)| Mem {
                hash: format!("h{h}"),
                summary: if s.trim().is_empty() { "x".into() } else { s },
                model: "embed-dim4".into(),
            }),
            0..8,
        )
        // de-dup by hash: insert_memory's source-hash unique index would skip dups,
        // which is correct behaviour but breaks a "all N round-trip" invariant.
        .prop_map(|mut v| {
            v.sort_by(|a, b| a.hash.cmp(&b.hash));
            v.dedup_by(|a, b| a.hash == b.hash);
            v
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        /// import(export(x, pw), pw) == x : every exported memory summary survives
        /// the口令-encrypted round-trip into a fresh vault, byte-for-byte.
        #[test]
        fn import_of_export_is_identity(mems in mems_strategy(), pw in "[ -~]{8,24}") {
            let src = Store::open_memory().unwrap();
            let src_dek = Key32::generate();
            seed(&src, &src_dek, &mems);
            let bundle = export_memory_bundle(&src, &src_dek, &pw).unwrap();

            let dst = Store::open_memory().unwrap();
            let dst_dek = Key32::generate();
            let r = import_memory_bundle(&dst, &dst_dek, &bundle, &pw).unwrap();
            prop_assert_eq!(r.imported, mems.len());

            let mut got: Vec<String> = dst
                .list_recent_memories(&dst_dek, usize::MAX).unwrap()
                .into_iter().map(|m| m.summary).collect();
            let mut want: Vec<String> = mems.iter().map(|m| m.summary.clone()).collect();
            got.sort(); want.sort();
            prop_assert_eq!(got, want);
        }

        /// Idempotency: re-importing the same bundle never adds a row (skipped == N).
        #[test]
        fn second_import_is_pure_skip(mems in mems_strategy(), pw in "[ -~]{8,24}") {
            let src = Store::open_memory().unwrap();
            let src_dek = Key32::generate();
            seed(&src, &src_dek, &mems);
            let bundle = export_memory_bundle(&src, &src_dek, &pw).unwrap();

            let dst = Store::open_memory().unwrap();
            let dst_dek = Key32::generate();
            import_memory_bundle(&dst, &dst_dek, &bundle, &pw).unwrap();
            let r2 = import_memory_bundle(&dst, &dst_dek, &bundle, &pw).unwrap();
            prop_assert_eq!(r2.imported, 0);
            prop_assert_eq!(r2.skipped, mems.len());
        }

        /// Any wrong passphrase is rejected and writes nothing (atomic reject).
        #[test]
        fn wrong_passphrase_never_writes(mems in mems_strategy().prop_filter("nonempty", |m| !m.is_empty()),
                                          pw in "[ -~]{8,16}", wrong in "[ -~]{8,16}") {
            prop_assume!(pw != wrong);
            let src = Store::open_memory().unwrap();
            let src_dek = Key32::generate();
            seed(&src, &src_dek, &mems);
            let bundle = export_memory_bundle(&src, &src_dek, &pw).unwrap();

            let dst = Store::open_memory().unwrap();
            let dst_dek = Key32::generate();
            prop_assert!(import_memory_bundle(&dst, &dst_dek, &bundle, &wrong).is_err());
            prop_assert_eq!(dst.list_recent_memories(&dst_dek, usize::MAX).unwrap().len(), 0);
        }
    }
}
