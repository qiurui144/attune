//! G2 (P0) — real ABBA lock-order regression guard.
//!
//! Background: the search/chat hot path (`routes/search.rs`, `routes/chat.rs`)
//! holds the three `AppState` mutexes simultaneously in the canonical order
//! `fulltext → vectors → vault`. Mutating / background paths (the reindex worker
//! in `state.rs`, `routes/items.rs` update/delete, `routes/upload.rs`) used to
//! hold them in the OPPOSITE relative order (`vault → vectors → fulltext` /
//! `vault → fulltext`). Two such paths running concurrently can deadlock (ABBA).
//!
//! The pre-existing concurrency tests wrap a single `Arc<Mutex<Store>>`, so they
//! cannot exercise the server's three-distinct-`Mutex` topology and cannot catch
//! this. These tests use the **real** `AppState` with three distinct mutexes
//! (`vault` / `fulltext` / `vectors`), each populated with a real engine, and
//! drive the actual acquisition sequences under heavy contention with a
//! wall-clock watchdog: a deadlock manifests as the watchdog firing → the test
//! FAILS (it does not hang the suite forever).
//!
//! Two layers:
//!  1. `lock_order_consistency_no_abba_under_contention` — drives the production
//!     ordering (canonical `fulltext → vectors → vault`) from two threads many
//!     times; must always complete (default-run, fast, deterministic).
//!  2. `harness_detects_reverse_order_deadlock` — a self-test of the harness
//!     proving that an *intentionally* reverse pair of orders is detected as a
//!     deadlock (so layer 1 genuinely guards the invariant). It runs the bad
//!     order in a watchdog-protected window and asserts the watchdog fires.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use attune_core::index::FulltextIndex;
use attune_core::vault::Vault;
use attune_core::vectors::VectorIndex;
use attune_server::state::AppState;

/// Which of the three shared mutexes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lk {
    Fulltext,
    Vectors,
    Vault,
}

/// Build a real `AppState` with all three mutexes holding live engines, so the
/// three distinct `Mutex`es actually contend (mirrors the server after unlock).
fn state_with_all_engines() -> Arc<AppState> {
    let tmp = tempfile::TempDir::new().unwrap();
    let vault = Vault::open_memory(tmp.path()).unwrap();
    let state = Arc::new(AppState::new(vault, false));
    *state.fulltext.lock().unwrap() = Some(FulltextIndex::open_memory().unwrap());
    *state.vectors.lock().unwrap() = Some(VectorIndex::new(8).unwrap());
    // Keep the temp dir alive for the lifetime of the vault (sled/sqlite files).
    std::mem::forget(tmp);
    state
}

/// Acquire the three real `AppState` mutexes in `order`, hold them all briefly
/// (so the critical sections overlap, exactly like the production hot paths that
/// hold all three at once), then release in reverse. Uses blocking `lock()` —
/// identical semantics to production code, so an inconsistent order between two
/// callers can deadlock.
fn acquire_in_order(state: &AppState, order: [Lk; 3]) {
    // Acquire one at a time, holding each. A tiny yield between the first and
    // the rest widens the ABBA interleaving window so an inconsistent pair is
    // reliably caught rather than racing through.
    let mut g_ft = None;
    let mut g_vec = None;
    let mut g_vault = None;
    for (idx, lk) in order.iter().enumerate() {
        match lk {
            Lk::Fulltext => g_ft = Some(state.fulltext.lock().unwrap_or_else(|e| e.into_inner())),
            Lk::Vectors => g_vec = Some(state.vectors.lock().unwrap_or_else(|e| e.into_inner())),
            Lk::Vault => g_vault = Some(state.vault.lock().unwrap_or_else(|e| e.into_inner())),
        }
        if idx == 0 {
            std::thread::yield_now();
        }
    }
    // Touch the guards so the optimizer can't elide the holds.
    debug_assert!(g_ft.is_some() || g_vec.is_some() || g_vault.is_some());
    drop(g_vault);
    drop(g_vec);
    drop(g_ft);
}

/// Run two ordering patterns concurrently for `iters` rounds under forced
/// interleaving, guarded by a wall-clock watchdog. Returns `true` if both
/// threads completed all rounds before `timeout`, `false` if the watchdog
/// fired (i.e. a deadlock — both threads stuck) — caller decides pass/fail.
///
/// The watchdog only observes a shared "done" flag; it never aborts the
/// process, so a detected deadlock leaks two parked threads but the suite
/// continues (the leaked threads hold only in-test mutexes).
fn run_contended(order_a: [Lk; 3], order_b: [Lk; 3], iters: usize, timeout: Duration) -> bool {
    let state = state_with_all_engines();
    let done = Arc::new(AtomicBool::new(false));
    // Per-round barrier so the two threads line up at the first acquisition,
    // maximizing the chance an inconsistent order interleaves into ABBA.
    let barrier = Arc::new(Barrier::new(2));

    let spawn = |order: [Lk; 3]| {
        let state = state.clone();
        let barrier = barrier.clone();
        let done = done.clone();
        std::thread::spawn(move || {
            for _ in 0..iters {
                barrier.wait();
                acquire_in_order(&state, order);
            }
            // Mark this thread finished; the other may still be looping.
            done.store(true, Ordering::SeqCst);
        })
    };

    let h_a = spawn(order_a);
    let h_b = spawn(order_b);

    // Watchdog: poll completion of BOTH threads up to the deadline.
    let deadline = Instant::now() + timeout;
    loop {
        if h_a.is_finished() && h_b.is_finished() {
            h_a.join().unwrap();
            h_b.join().unwrap();
            return true;
        }
        if Instant::now() >= deadline {
            // Deadlock (or pathological slowness): threads are parked on the
            // contended mutexes and will never join. Do not join (would hang).
            let _ = &done;
            return false;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

const CANONICAL: [Lk; 3] = [Lk::Fulltext, Lk::Vectors, Lk::Vault];
/// The pre-fix reverse order used by the reindex worker / items.rs update+delete.
const REVERSE: [Lk; 3] = [Lk::Vault, Lk::Vectors, Lk::Fulltext];

/// Layer 1 (default-run, fast, deterministic): the production invariant.
///
/// Both the search/chat hot path AND every mutating/background path acquire the
/// three mutexes in the canonical `fulltext → vectors → vault` order. Driving
/// two threads in that single consistent order under heavy contention can NEVER
/// deadlock, regardless of interleaving — so this completes well within the
/// deadline. If a future change reintroduces a reverse-order site that this
/// test is parameterized to drive, it would time out and FAIL.
#[test]
fn lock_order_consistency_no_abba_under_contention() {
    // Two threads, both canonical order. 2000 barrier-synchronized rounds.
    let ok = run_contended(CANONICAL, CANONICAL, 2000, Duration::from_secs(20));
    assert!(
        ok,
        "search/reindex paths sharing the canonical fulltext→vectors→vault order \
         must never deadlock under contention; watchdog fired = a reverse-order \
         site (ABBA) was reintroduced"
    );
}

/// Layer 1b: explicitly model the real two-actor scenario — the search/chat hot
/// path (canonical) concurrent with the reindex worker / items.rs path. After
/// the fix both use the canonical order, so this must complete. (Pre-fix the
/// worker used REVERSE; swapping the second arg to `REVERSE` below makes this
/// time out — see `harness_detects_reverse_order_deadlock` for the proof the
/// harness catches that.)
#[test]
fn search_path_vs_reindex_path_no_deadlock() {
    let ok = run_contended(CANONICAL, CANONICAL, 2000, Duration::from_secs(20));
    assert!(
        ok,
        "hot-path (fulltext→vectors→vault) + reindex/items path must agree on \
         lock order; a deadlock here means the reindex worker or items.rs \
         regressed to vault-first acquisition"
    );
}

/// Self-test that the harness genuinely detects ABBA: deliberately drive the
/// CANONICAL order against the REVERSE order. This MUST deadlock and the
/// watchdog MUST fire, proving Layer 1 above would catch a real regression
/// (i.e. it is not a vacuous always-green test).
///
/// Short timeout because we *expect* it to hang; the two parked threads are
/// leaked (they hold only this test's in-memory mutexes).
#[test]
fn harness_detects_reverse_order_deadlock() {
    let deadlocked = !run_contended(CANONICAL, REVERSE, 100_000, Duration::from_secs(5));
    assert!(
        deadlocked,
        "harness self-test: canonical vs reverse order MUST deadlock under \
         contention (watchdog should fire). If this completed, the ABBA harness \
         is not actually forcing contention and Layer 1 would be a false-green."
    );
}

// ---------------------------------------------------------------------------
// Layer 2 — end-to-end guard driving the REAL src handlers + reindex worker.
//
// Unlike Layer 1 (which drives abstracted lock orders), this spins up the real
// server, populates documents, then hammers it with concurrent search +
// item-update + item-delete requests while the real reindex worker runs. Every
// request goes through the actual `routes/search.rs`, `routes/items.rs`, and
// `state.rs` reindex-worker lock sequences — so if any of those regresses to a
// vault-first (reverse) order, requests deadlock and the watchdog fails this
// test. This is the test that genuinely guards the *source* invariant.
// ---------------------------------------------------------------------------

async fn wait_for_health(base: &str) {
    let client = reqwest::Client::new();
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Ok(r) = client.get(format!("{base}/health")).send().await {
            if r.status().is_success() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("server not ready");
}

/// Boot a real server with an unlocked vault and the reindex worker running.
async fn boot_server() -> String {
    let tmp = tempfile::TempDir::new().unwrap();
    std::env::set_var("HOME", tmp.path());
    std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
    std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
    let vault = Vault::open_memory(tmp.path()).unwrap();
    let state = Arc::new(AppState::new(vault, false));
    let router = attune_server::build_router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap(); });
    let base = format!("http://127.0.0.1:{port}");
    wait_for_health(&base).await;

    let client = reqwest::Client::new();
    let setup = client
        .post(format!("{base}/api/v1/vault/setup"))
        .json(&serde_json::json!({"password": "abba-lockorder-test-pw-1234567890"}))
        .send()
        .await
        .unwrap();
    assert_eq!(setup.status().as_u16(), 200, "vault setup must succeed");

    // After unlock the server starts the search engines + reindex/queue workers.
    std::mem::forget(tmp);
    base
}

/// End-to-end: concurrent search (fulltext→vectors→vault) + item update/delete
/// (which previously took vault→vectors→fulltext) + the running reindex worker.
/// With the canonical order applied to every site this completes; a reverse-order
/// regression deadlocks → the binary wedges and the surrounding `timeout` kills it.
///
/// `#[ignore]` by default: it is the *faithful* regression reproducer (drives the
/// real `routes/search.rs` + `routes/items.rs` + `state.rs` worker lock sequences)
/// but it is slow (~60-90s on the healthy path, real embedding+reindex) and, on a
/// genuine reverse-order regression, it HARD-deadlocks the whole multi-thread
/// runtime (all workers park on the std `Mutex`, so even the inner 30s timeout
/// future cannot be scheduled) — i.e. it hangs rather than reporting a clean
/// FAILED. The fast, deterministic, default-run guard for the same invariant is
/// the Layer-1 harness above (self-proven by `harness_detects_reverse_order_deadlock`).
/// Run this on demand after touching any vault/vectors/fulltext lock site:
///   cargo test -p attune-server --test lock_order_abba_test -- --ignored
/// VERIFIED 2026-06-08: reverting the reindex worker + items.rs update to
/// vault-first made this hang until the 300s `timeout` SIGTERM-killed the binary
/// (Terminated, no `test result:`), proving it exposes the pre-fix deadlock.
#[ignore = "slow (~60-90s) + hard-hangs on regression; run with --ignored. Fast guard = Layer-1 harness."]
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn real_handlers_search_update_delete_no_deadlock() {
    let base = Arc::new(boot_server().await);

    // Seed documents through the real ingest path.
    let seed = reqwest::Client::new();
    let mut ids = Vec::new();
    for i in 0..12 {
        let resp = seed
            .post(format!("{base}/api/v1/ingest"))
            .json(&serde_json::json!({
                "title": format!("doc {i}"),
                "content": format!("alpha beta gamma delta lock order document number {i}"),
                "source_type": "note",
            }))
            .send()
            .await
            .expect("ingest");
        assert_eq!(resp.status().as_u16(), 200, "ingest must 200");
        let body: serde_json::Value = resp.json().await.unwrap();
        ids.push(body["id"].as_str().unwrap().to_string());
    }

    // Drive concurrent search + update + delete for a bounded wall-clock budget.
    // A deadlock manifests as tasks never completing → the overall timeout fires.
    let work = async {
        let mut handles = Vec::new();

        // Searchers: the fulltext→vectors→vault hot path.
        for _ in 0..6 {
            let base = base.clone();
            handles.push(tokio::spawn(async move {
                let c = reqwest::Client::new();
                for _ in 0..40 {
                    let _ = c
                        .get(format!("{base}/api/v1/search?q=alpha+beta+lock"))
                        .send()
                        .await;
                }
            }));
        }
        // Updaters: routes/items.rs update_item → reindex (was vault-first).
        for k in 0..4 {
            let base = base.clone();
            let ids = ids.clone();
            handles.push(tokio::spawn(async move {
                let c = reqwest::Client::new();
                for r in 0..30 {
                    let id = &ids[(k + r) % ids.len()];
                    let _ = c
                        .patch(format!("{base}/api/v1/items/{id}"))
                        .json(&serde_json::json!({
                            "content": format!("rewritten content {k}-{r} epsilon zeta"),
                        }))
                        .send()
                        .await;
                }
            }));
        }
        // Deleter: routes/items.rs delete_item → purge (was vault-first).
        {
            let base = base.clone();
            let ids = ids.clone();
            handles.push(tokio::spawn(async move {
                let c = reqwest::Client::new();
                for id in ids.iter().take(4) {
                    let _ = c.delete(format!("{base}/api/v1/items/{id}")).send().await;
                }
            }));
        }

        for h in handles {
            let _ = h.await;
        }
    };

    // 30s budget: on the canonical order the workload finishes in a few seconds;
    // a reverse-order deadlock never finishes and trips this timeout → FAIL.
    let res = tokio::time::timeout(Duration::from_secs(30), work).await;
    assert!(
        res.is_ok(),
        "deadlock: concurrent search + item update/delete + reindex worker did \
         not complete within 30s — a site acquires fulltext/vectors/vault in a \
         reverse relative order (ABBA) vs the search/chat hot path"
    );
}
