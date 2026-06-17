//! #2 #5: 解锁不再同步阻塞在底座模型(~330MB)下载上 + 四类模型都被后台调度。
//!
//! 这两条是本次 fix 的核心 invariant：
//! - `init_search_engines()`（解锁路径同步调用）**不**触发任何模型下载——跑完后
//!   `model_bootstrap` 仍全 Pending（download 已挪到后台）。
//! - `spawn_model_bootstrap()` 立即返回（不阻塞调用方），并把 embedding/reranker/ocr/asr
//!   四类全部调度（离开 Pending）。
//!
//! 用 `HF_HUB_OFFLINE=1` 让真实下载 fail-fast（逃生开关），测试无网络依赖、确定性收敛。

use std::sync::Arc;
use std::time::{Duration, Instant};

use attune_core::infer::bootstrap_status::{ModelPhase, MODEL_CLASSES};

fn isolate_home(tmp: &tempfile::TempDir) {
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
        // 逃生开关：禁止任何联网下载，让四类 ensure_* fail-fast（不挂网络超时）。
        std::env::set_var("HF_HUB_OFFLINE", "1");
        // 走 ONNX 分支（默认），这样 embedding ONNX 在 offline 下失败 → 触发 Ollama 兜底，
        // 正好验证"失败不致 embedding 不可用"。
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

/// `spawn_model_bootstrap` 立即返回（不阻塞解锁），并把四类全部调度到终态。
/// 即便底层下载源很慢/失败（这里用 offline 模拟），调用方也立刻拿回控制权。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_model_bootstrap_is_non_blocking_and_schedules_all_four() {
    let tmp = tempfile::TempDir::new().unwrap();
    isolate_home(&tmp);
    let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
    let state = Arc::new(attune_server::state::AppState::new(vault, false));

    // 立即返回：spawn 后调用方不被任何下载阻塞（offline 下底层每类会 fail-fast，
    // 但即便慢源真在下，spawn 本身也只是起线程 → 必须秒回）。
    let started = Instant::now();
    attune_server::state::AppState::spawn_model_bootstrap(state.clone());
    let spawn_elapsed = started.elapsed();
    assert!(
        spawn_elapsed < Duration::from_secs(1),
        "spawn_model_bootstrap must return immediately (background), took {spawn_elapsed:?}"
    );

    // 后台线程应把四类都调度到终态（Ready 或 Failed），不留 Pending。
    // offline 下 embedding 会兜底到 Ollama → Ready；reranker/ocr/asr fail-fast → Failed
    //（弱机 asr tier 不支持时也会被标 Ready 跳过）。
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let all_scheduled = MODEL_CLASSES.iter().all(|c| {
            !matches!(state.model_bootstrap.phase(c), Some(ModelPhase::Pending) | None)
        });
        if all_scheduled {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "all four classes should leave Pending within 20s; snapshot={}",
            state.model_bootstrap.snapshot()
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    // 四类都被调度（离开 Pending）= bug #5 的"四类都触发"已满足。
    for class in MODEL_CLASSES {
        let phase = state.model_bootstrap.phase(class).expect("phase present");
        assert!(
            !matches!(phase, ModelPhase::Pending),
            "{class} was never scheduled (still Pending) — all base models must be triggered"
        );
    }

    // embedding 必须可用（offline ONNX 失败 → Ollama 兜底 Ready），不因下载失败而缺位。
    assert_eq!(
        state.model_bootstrap.phase("embedding"),
        Some(ModelPhase::Ready),
        "embedding must end Ready (Ollama fallback when ONNX download unavailable)"
    );
    assert!(
        state.embedding().is_some(),
        "embedding provider must be installed even when ONNX download fails"
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
    assert!(state.model_bootstrap.all_ready(), "idempotent: stays all ready");
}
