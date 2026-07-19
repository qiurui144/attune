//! #2 #5: 解锁不再同步阻塞在底座模型(~330MB)下载上 + 四类模型交给 scheduler 托管。
//!
//! 这两条是本次 fix 的核心 invariant：
//! - `init_search_engines()`（解锁路径同步调用）**不**触发任何模型下载——跑完后
//!   `model_bootstrap` 仍全 Pending（download 已挪到后台）。
//! - `spawn_model_bootstrap()` 立即返回（不阻塞调用方），安装 scheduler-backed
//!   embedding/reranker handles，并把 embedding/reranker/ocr/asr 四类标 Ready。

use std::sync::Arc;
use std::time::{Duration, Instant};

use attune_core::infer::bootstrap_status::{ModelPhase, MODEL_CLASSES};

fn isolate_home(tmp: &tempfile::TempDir) {
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
        std::env::remove_var("ATTUNE_EMBEDDING_BACKEND");
    }
}

/// 解锁路径里同步调用的 `init_search_engines()` 本身不下载任何模型：
/// 跑完后 model_bootstrap 仍全 Pending（证明 ~330MB 下载已移出同步解锁路径）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn init_search_engines_does_not_download_models() {
    let tmp = tempfile::TempDir::new().unwrap();
    isolate_home(&tmp);
    let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
    let state = Arc::new(attune_server::state::AppState::new(vault, false));

    // init_search_engines 是解锁路径里**同步**那一步。注入一个"慢源"也只会卡在这里——
    // 但因为它不再构造 embedding/reranker provider，所以零下载、秒回。
    let started = Instant::now();
    state.init_search_engines();
    let elapsed = started.elapsed();

    // 不下载 → 应远快于一次模型下载（offline 也不会去尝试，因为根本没调 ensure_*）。
    assert!(
        elapsed < Duration::from_secs(5),
        "init_search_engines should be fast (no model download), took {elapsed:?}"
    );

    // 关键：四类底座仍全 Pending —— 解锁同步路径没有触发任何模型获取。
    for class in MODEL_CLASSES {
        assert_eq!(
            state.model_bootstrap.phase(class),
            Some(ModelPhase::Pending),
            "{class} must stay Pending after init_search_engines (download is background-only)"
        );
    }
    assert_eq!(state.model_bootstrap.snapshot()["all_ready"], false);
}

/// `spawn_model_bootstrap` 立即返回（不阻塞解锁），并把四类全部交给 scheduler 托管。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_model_bootstrap_is_non_blocking_and_marks_scheduler_managed_ready() {
    let tmp = tempfile::TempDir::new().unwrap();
    isolate_home(&tmp);
    let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
    vault.setup("P@ss-bootstrap-models").expect("setup");
    let state = Arc::new(attune_server::state::AppState::new(vault, false));

    // 立即返回：spawn 后调用方不被任何下载阻塞。attune-server 只安装
    // scheduler-backed runtime handles，模型拉取/加载由 scheduler 负责。
    let started = Instant::now();
    attune_server::state::AppState::spawn_model_bootstrap(state.clone());
    let spawn_elapsed = started.elapsed();
    assert!(
        spawn_elapsed < Duration::from_secs(1),
        "spawn_model_bootstrap must return immediately (background), took {spawn_elapsed:?}"
    );

    let deadline = Instant::now() + Duration::from_secs(2);
    while !(state.model_bootstrap.all_ready()
        && state.embedding().is_some()
        && state
            .reranker
            .lock()
            .ok()
            .map(|g| g.is_some())
            .unwrap_or(false))
    {
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    for class in MODEL_CLASSES {
        let phase = state.model_bootstrap.phase(class).expect("phase present");
        assert_eq!(phase, ModelPhase::Ready, "{class} should be scheduler-managed Ready");
    }
    assert!(state.embedding().is_some(), "embedding provider handle must be installed");
    assert!(
        state
            .reranker
            .lock()
            .ok()
            .map(|g| g.is_some())
            .unwrap_or(false),
        "reranker provider handle must be installed"
    );
}

/// 防重入：四类都 ready 后再次 spawn 直接跳过（不重复拉取）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_model_bootstrap_is_idempotent_when_all_ready() {
    let tmp = tempfile::TempDir::new().unwrap();
    isolate_home(&tmp);
    let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
    let state = Arc::new(attune_server::state::AppState::new(vault, false));

    // 直接把四类标 ready 模拟"已完成首次 bootstrap"。
    for c in MODEL_CLASSES {
        state.model_bootstrap.mark_ready(c);
    }
    assert!(state.model_bootstrap.all_ready());

    // 再 spawn 应立即 no-op 返回（不起新下载线程改变状态）。
    attune_server::state::AppState::spawn_model_bootstrap(state.clone());
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        state.model_bootstrap.all_ready(),
        "idempotent: stays all ready"
    );
}
