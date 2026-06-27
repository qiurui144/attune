//! Memory scope isolation — property tests (spec §9, chat-centric IA 2026-06-26).
//!
//! The privacy invariant the whole feature rests on, asserted over hundreds of
//! randomized memory→scope assignments:
//!
//! 1. **Project never leaks another project** — a `Project(P)` retrieval returns
//!    only rows whose scope is `global` or exactly `project:P`; no `project:Q≠P`
//!    row ever appears.
//! 2. **Loose conversation sees global only** — a `Conversation(_)` retrieval
//!    returns only `global` rows (its own recall is lazily derived, not stored).
//! 3. **Global sees everything** — the legacy/Global scope returns every live row
//!    regardless of scope (back-compat with pre-IA list_live_memories).

use attune_core::crypto::Key32;
use attune_core::memory::MemoryScope;
use attune_core::store::Store;
use proptest::prelude::*;

/// Map a 0..4 selector to a scope: 0=global, 1=project A, 2=B, 3=C.
fn scope_for(sel: u8) -> MemoryScope {
    match sel {
        0 => MemoryScope::Global,
        1 => MemoryScope::Project("A".into()),
        2 => MemoryScope::Project("B".into()),
        _ => MemoryScope::Project("C".into()),
    }
}

proptest! {
    #[test]
    fn project_retrieval_never_leaks_other_project(
        assignments in prop::collection::vec((0u8..4u8, "[a-z]{3}"), 1..30)
    ) {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        for (i, (sel, h)) in assignments.iter().enumerate() {
            // Unique hash per row so uq_memories_source never collapses inserts.
            let hash = format!("{h}-{i}");
            store
                .insert_memory_scoped(
                    &dek, "episodic", 0, 1, &[hash], "s", "m", 0, &scope_for(*sel),
                )
                .unwrap();
        }
        for p in ["A", "B", "C"] {
            let got = store
                .list_live_memories_scoped(&dek, "episodic", false, &MemoryScope::Project(p.into()))
                .unwrap();
            for m in &got {
                prop_assert!(
                    m.scope_kind == "global"
                        || (m.scope_kind == "project" && m.scope_id.as_deref() == Some(p)),
                    "leak: project {p} saw scope {:?}/{:?}",
                    m.scope_kind,
                    m.scope_id
                );
            }
        }
    }

    #[test]
    fn conversation_retrieval_sees_global_only(
        assignments in prop::collection::vec((0u8..4u8, "[a-z]{3}"), 1..30)
    ) {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        for (i, (sel, h)) in assignments.iter().enumerate() {
            let hash = format!("{h}-{i}");
            store
                .insert_memory_scoped(
                    &dek, "episodic", 0, 1, &[hash], "s", "m", 0, &scope_for(*sel),
                )
                .unwrap();
        }
        let got = store
            .list_live_memories_scoped(&dek, "episodic", false, &MemoryScope::Conversation("c1".into()))
            .unwrap();
        for m in &got {
            prop_assert_eq!(
                m.scope_kind.as_str(),
                "global",
                "loose conversation must see global only, saw {:?}",
                m.scope_id
            );
        }
    }

    #[test]
    fn global_retrieval_sees_every_row(
        assignments in prop::collection::vec((0u8..4u8, "[a-z]{3}"), 1..30)
    ) {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let n = assignments.len();
        for (i, (sel, h)) in assignments.iter().enumerate() {
            let hash = format!("{h}-{i}");
            store
                .insert_memory_scoped(
                    &dek, "episodic", 0, 1, &[hash], "s", "m", 0, &scope_for(*sel),
                )
                .unwrap();
        }
        let got = store
            .list_live_memories_scoped(&dek, "episodic", false, &MemoryScope::Global)
            .unwrap();
        prop_assert_eq!(got.len(), n, "Global scope must see all {} rows", n);
    }
}
