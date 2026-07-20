use crate::error::{AppError, AppResult};
use crate::state::SharedState;
use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use std::time::Instant;

const DEFAULT_RETRIEVAL_WARMUP_METADATA_LIMIT: u32 = 4096;
const DEFAULT_RETRIEVAL_WARMUP_TOP_K: u32 = 5;

fn post_unlock_settings(state: &SharedState) -> serde_json::Value {
    state
        .vault
        .lock()
        .ok()
        .and_then(|vault| vault.store().get_meta("app_settings").ok().flatten())
        .and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

fn retrieval_warmup_queries() -> Vec<String> {
    let configured = std::env::var("ATTUNE_RETRIEVAL_WARMUP_QUERIES")
        .ok()
        .or_else(|| std::env::var("ATTUNE_SCHEDULER_RETRIEVAL_WARMUP_QUERIES").ok());
    let raw = configured.unwrap_or_else(|| {
        [
            "source manual reference",
            "citation source lookup",
            "local knowledge source",
            "来源 手册 引用",
        ]
        .join(";")
    });
    raw.split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .take(8)
        .map(ToString::to_string)
        .collect()
}

fn retrieval_warmup_metadata_limit() -> usize {
    crate::local_scheduler::env_u32_any(
        &[
            "ATTUNE_RETRIEVAL_WARMUP_METADATA_LIMIT",
            "ATTUNE_SCHEDULER_RETRIEVAL_WARMUP_METADATA_LIMIT",
        ],
        DEFAULT_RETRIEVAL_WARMUP_METADATA_LIMIT,
    ) as usize
}

fn retrieval_warmup_top_k() -> usize {
    crate::local_scheduler::env_u32_any(
        &[
            "ATTUNE_RETRIEVAL_WARMUP_TOP_K",
            "ATTUNE_SCHEDULER_RETRIEVAL_WARMUP_TOP_K",
        ],
        DEFAULT_RETRIEVAL_WARMUP_TOP_K,
    )
    .clamp(1, 20) as usize
}

fn warm_retrieval_after_unlock(state: &SharedState) {
    let settings = post_unlock_settings(state);
    let default_enabled = crate::local_scheduler::native_kb_enabled(&settings, &state.hardware);
    if !crate::local_scheduler::env_bool_any(
        &[
            "ATTUNE_RETRIEVAL_WARMUP",
            "ATTUNE_SCHEDULER_RETRIEVAL_WARMUP",
            "ATTUNE_LOCAL_RETRIEVAL_WARMUP",
        ],
        default_enabled,
    ) {
        return;
    }

    let started = Instant::now();
    let metadata_limit = retrieval_warmup_metadata_limit();
    let top_k = retrieval_warmup_top_k();

    let dek = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(dek) = vault.dek_db() else { return };
        let _ = vault.store().list_items(metadata_limit, 0);
        dek
    };

    let reranker = state.reranker.lock().ok().and_then(|g| g.clone());
    let embedding = crate::routes::privacy::governed_embedding(state, false);
    let queries = retrieval_warmup_queries();
    let mut warmed = 0usize;

    for query in &queries {
        let (mut params, _) = crate::retrieval_policy::build_search_params(
            state.hardware.form_factor,
            true,
            query,
            None,
            top_k,
            None,
            None,
            None,
        );
        params.skip_rerank = true;
        let result = {
            let ft_guard = if params.skip_vector {
                state.fulltext.try_lock().ok()
            } else {
                Some(state.fulltext.lock().unwrap_or_else(|e| e.into_inner()))
            };
            let vec_guard = if params.skip_vector {
                None
            } else {
                Some(state.vectors.lock().unwrap_or_else(|e| e.into_inner()))
            };
            let vault_guard = state.vault.lock().unwrap_or_else(|e| e.into_inner());
            let ctx = attune_core::search::SearchContext {
                fulltext: ft_guard.as_ref().and_then(|guard| guard.as_ref()),
                vectors: vec_guard.as_ref().and_then(|guard| guard.as_ref()),
                embedding: embedding.clone(),
                reranker: reranker.clone(),
                store: vault_guard.store(),
                dek: &dek,
            };
            attune_core::search::search_with_context(&ctx, query, &params)
        };
        match result {
            Ok(results) => {
                warmed += 1;
                tracing::debug!(
                    query = %query,
                    results = results.len(),
                    skip_vector = params.skip_vector,
                    "post-unlock retrieval warmup query complete"
                );
            }
            Err(e) => {
                tracing::debug!(query = %query, error = %e, "post-unlock retrieval warmup query skipped")
            }
        }
    }

    tracing::info!(
        queries = warmed,
        metadata_limit,
        elapsed_ms = started.elapsed().as_millis(),
        "post-unlock retrieval warmup complete"
    );
}

fn spawn_post_unlock_services(state: SharedState) {
    // Keep unlock/setup responsive: crypto unlock returns the bearer token immediately,
    // while local indexes, model bootstrap, EP stack probing, and workers warm up in the
    // background. Membership-owned LLM/PluginHub credentials and entitlement
    // rows deliberately remain inactive until the cloud-session restore below
    // has re-verified the account, paid license, and device binding.
    state.reload_llm();
    state.reload_plugin_hub_from_settings();
    state.entitlement_cache.hydrate_from_rows(Vec::new());
    let member_restore_state = state.clone();
    tokio::spawn(async move {
        let _ =
            crate::routes::member::restore_member_state_from_cloud_session(&member_restore_state)
                .await;
    });

    tokio::task::spawn_blocking(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.init_search_engines();
            warm_retrieval_after_unlock(&state);
            // #2 #5: 底座模型(embedding/reranker/ocr/asr)后台拉取,解锁不阻塞在 ~330MB 下载上。
            crate::state::AppState::spawn_model_bootstrap(state.clone());
            // EP 运行时软件栈(cuda/openvino/rocm/directml/vitisai userspace)按需安装,
            // 缺则像底座模型一样后台拉取(内核驱动除外,走 #6 consent)。
            crate::state::AppState::spawn_stack_bootstrap(state.clone());
            crate::state::AppState::start_classify_worker(state.clone());
            crate::state::AppState::start_rescan_worker(state.clone());
            crate::state::AppState::start_reindex_worker(state.clone());
            crate::state::AppState::start_webdav_sync_worker(state.clone());
            crate::state::AppState::start_email_sync_worker(state.clone());
            crate::state::AppState::start_rss_sync_worker(state.clone());
            crate::state::AppState::start_monitoring_worker(state.clone());
            crate::state::AppState::start_queue_worker(state.clone());
            crate::state::AppState::start_skill_evolver(state.clone());
            crate::state::AppState::start_entitlement_worker(state.clone());
            // G3①: drain any locked-mode staged ingests now that the DEK is available.
            crate::state::AppState::start_staging_drain_worker(state);
        }));
        if result.is_err() {
            tracing::error!("vault post-unlock service bootstrap panicked");
        }
    });
}

#[derive(Deserialize)]
pub struct SetupRequest {
    pub password: String,
}

#[derive(Deserialize)]
pub struct UnlockRequest {
    pub password: String,
}

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    pub old_password: String,
    pub new_password: String,
}

#[derive(Deserialize)]
pub struct ForgotPasswordResetRequest {
    pub confirmation: String,
}

#[derive(Deserialize)]
pub struct ResetWithRecoveryKeyRequest {
    pub recovery_key: String,
    pub new_password: String,
}

pub async fn vault_status(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let vault_state = vault.state();
    let item_count = if matches!(vault_state, attune_core::vault::VaultState::Unlocked) {
        vault.store().item_count().unwrap_or(0)
    } else {
        0
    };

    Json(serde_json::json!({
        "state": vault_state,
        "items": item_count,
    }))
}

pub async fn vault_setup(
    State(state): State<SharedState>,
    Json(body): Json<SetupRequest>,
) -> AppResult<Json<serde_json::Value>> {
    // setup 成功后内部走一次 lock+unlock，复用 unlock 的 token 颁发路径，
    // 让首次安装直接拿到可用 token（避免客户端必须 restart server 再 unlock）。
    let (token, recovery_key) = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let recovery_key = vault
            .setup_with_recovery_key(&body.password)
            .map_err(|e| AppError::BadRequest(e.to_string()))?;
        // setup 自动 Unlocked；先 lock 再 unlock，复用 unlock token 颁发路径。
        // 首次安装一次性操作，多一次 Argon2id 派生可接受。
        vault
            .lock()
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let token = vault
            .unlock(&body.password)
            .map_err(|e| AppError::Internal(e.to_string()))?;
        (token, recovery_key)
    };
    state.clear_locked_privacy_authorization();
    // Bug-C: vault unlock 后立即触发 reload_llm,确保 settings 中已有的 llm config
    // 在 server restart 后第一次 chat 即可工作(不再依赖 member-login gateway_should_apply
    // 走 reload_llm 分支)。
    spawn_post_unlock_services(state.clone());
    Ok(Json(serde_json::json!({
        "status": "ok",
        "state": "unlocked",
        "token": token,
        "recovery_key": recovery_key,
    })))
}

pub async fn vault_unlock(
    State(state): State<SharedState>,
    Json(body): Json<UnlockRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let token = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        vault
            .unlock(&body.password)
            .map_err(|e| AppError::Unauthorized(e.to_string()))?
    };
    state.clear_locked_privacy_authorization();
    // Bug-C: per setup 同步注释,unlock 后强制 reload_llm,杜绝
    // "server restart → unlock → chat 503" 的 P3。
    spawn_post_unlock_services(state.clone());
    Ok(Json(serde_json::json!({"status": "ok", "token": token})))
}

/// Flush and remove all vault-derived runtime handles before dropping the DEKs.
/// Both public lock surfaces must use this helper so their security semantics
/// cannot drift.
pub(crate) async fn lock_and_clear_runtime(state: &SharedState) -> attune_core::error::Result<()> {
    let blocking_state = state.clone();
    tokio::task::spawn_blocking(move || {
        // The guard belongs to the blocking transaction, not to its async
        // waiter. If a disconnected client cancels the handler, Tokio detaches
        // this closure; keeping the guard here still serializes member
        // login/logout/restore until the actual vault clear+lock completes.
        let _transition = blocking_state.member_transition.blocking_lock();
        blocking_state.lock_vault_and_clear_runtime()
    })
    .await
    .map_err(|_| {
        attune_core::error::VaultError::Io(std::io::Error::other(
            "vault runtime clear worker failed",
        ))
    })?
}

pub async fn vault_lock(State(state): State<SharedState>) -> AppResult<Json<serde_json::Value>> {
    lock_and_clear_runtime(&state)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(serde_json::json!({"status": "ok", "state": "locked"})))
}

pub async fn export_device_secret(
    State(state): State<SharedState>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let secret = vault
        .export_device_secret()
        .map_err(|e| AppError::Forbidden(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "device_secret": secret,
        "warning": "Store this value securely. It's required to unlock the vault on other devices."
    })))
}

#[derive(Deserialize)]
pub struct ImportDeviceSecretRequest {
    pub device_secret: String,
}

pub async fn import_device_secret(
    State(state): State<SharedState>,
    Json(body): Json<ImportDeviceSecretRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    vault
        .import_device_secret(&body.device_secret)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "status": "ok",
        "message": "device secret imported. Use /vault/unlock with your master password."
    })))
}

pub async fn vault_change_password(
    State(state): State<SharedState>,
    Json(body): Json<ChangePasswordRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    vault
        .change_password(&body.old_password, &body.new_password)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    Ok(Json(serde_json::json!({"status": "ok"})))
}

pub async fn vault_forgot_password_reset(
    State(state): State<SharedState>,
    Json(body): Json<ForgotPasswordResetRequest>,
) -> AppResult<Json<serde_json::Value>> {
    // Clearing scheduler-native providers can release a blocking HTTP client's
    // private Tokio runtime. Keep the complete clear/reset transaction off the
    // async handler worker so the response cannot be reset midway through.
    let confirmation = body.confirmation;
    let blocking_state = state.clone();
    tokio::task::spawn_blocking(move || {
        let _transition = blocking_state.member_transition.blocking_lock();
        blocking_state.clear_search_engines();
        let vault = blocking_state
            .vault
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        vault.forgot_password_reset(&confirmation)
    })
    .await
    .map_err(|_| AppError::Internal("vault reset worker failed".into()))?
    .map_err(|e| AppError::BadRequest(e.to_string()))?;
    state.clear_locked_privacy_authorization();

    Ok(Json(serde_json::json!({
        "status": "ok",
        "state": "sealed",
        "message": "vault reset complete, run setup again"
    })))
}

pub async fn vault_reset_with_recovery_key(
    State(state): State<SharedState>,
    Json(body): Json<ResetWithRecoveryKeyRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let token = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        vault
            .reset_password_with_recovery_key(&body.recovery_key, &body.new_password)
            .map_err(|e| AppError::BadRequest(e.to_string()))?;
        vault
            .unlock(&body.new_password)
            .map_err(|e| AppError::Unauthorized(e.to_string()))?
    };
    state.clear_locked_privacy_authorization();

    // Bug-C: reset 后也走 unlock 同样路径,显式 reload_llm。
    spawn_post_unlock_services(state.clone());

    Ok(Json(serde_json::json!({
        "status": "ok",
        "state": "unlocked",
        "token": token,
    })))
}

/// G3① observability: pending locked-mode staged ingest count. Readable in ANY vault
/// state (no DEK needed) so a UI / monitor can show "N files queued, waiting for unlock".
pub async fn vault_staging_status(State(_state): State<SharedState>) -> Json<serde_json::Value> {
    let pending = attune_core::staging::IngestStaging::open_default().count();
    Json(serde_json::json!({ "pending": pending }))
}

/// G3② auto-unlock threat-model copy. The real key-sealing mechanism (TPM / OS keyring /
/// secure enclave) is PENDING a dedicated security review; enabling auto-unlock changes
/// the threat model (a host-key-sealed vault is readable by anyone with physical access to
/// the device). This text is surfaced in the UI next to the toggle.
const AUTO_UNLOCK_THREAT_MODEL: &str = "Enabling auto-unlock seals the vault key on this \
device so it unlocks without your password after a reboot. This changes the threat model: \
anyone with physical access to the device can then read your vault. The key-sealing \
mechanism is pending a security review and is NOT yet implemented — turning this on only \
records your intent and shows this warning; no key is written to disk.";

#[derive(Deserialize)]
pub struct AutoUnlockRequest {
    pub enabled: bool,
}

/// Path to the auto-unlock intent flag. Stored as a tiny non-secret file under config_dir
/// (a single byte `0`/`1`) so it is readable/settable regardless of vault lock state.
/// Deliberately holds NO key material — the real sealing is PENDING-security-review.
fn auto_unlock_flag_path() -> std::path::PathBuf {
    attune_core::platform::config_dir().join("auto_unlock.flag")
}

fn read_auto_unlock_flag() -> bool {
    std::fs::read(auto_unlock_flag_path())
        .ok()
        .and_then(|b| b.first().copied())
        .map(|b| b == b'1')
        .unwrap_or(false)
}

/// GET auto-unlock state. `implemented:false` signals the real sealing is not yet built.
pub async fn vault_get_auto_unlock(State(_state): State<SharedState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "enabled": read_auto_unlock_flag(),
        "implemented": false,
        "threat_model": AUTO_UNLOCK_THREAT_MODEL,
    }))
}

/// PUT auto-unlock intent. Records the flag and returns the threat-model warning. Because
/// the real key-sealing is PENDING-security-review, enabling does NOT actually seal a key
/// or auto-unlock anything — it only persists the intent + warns. This avoids shipping a
/// half-built, insecure key store.
pub async fn vault_set_auto_unlock(
    State(_state): State<SharedState>,
    Json(body): Json<AutoUnlockRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let path = auto_unlock_flag_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&path, if body.enabled { b"1" } else { b"0" })
        .map_err(|e| AppError::Internal(format!("persist auto-unlock flag: {e}")))?;
    Ok(Json(serde_json::json!({
        "enabled": body.enabled,
        "implemented": false,
        "warning": if body.enabled { AUTO_UNLOCK_THREAT_MODEL } else { "" },
        "code": "auto-unlock-pending-security-review",
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WARMUP_ENV_KEYS: &[&str] = &[
        "ATTUNE_RETRIEVAL_WARMUP_QUERIES",
        "ATTUNE_SCHEDULER_RETRIEVAL_WARMUP_QUERIES",
        "ATTUNE_RETRIEVAL_WARMUP_METADATA_LIMIT",
        "ATTUNE_SCHEDULER_RETRIEVAL_WARMUP_METADATA_LIMIT",
        "ATTUNE_RETRIEVAL_WARMUP_TOP_K",
        "ATTUNE_SCHEDULER_RETRIEVAL_WARMUP_TOP_K",
    ];

    struct EnvSnapshot {
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl EnvSnapshot {
        fn clean(keys: &'static [&'static str]) -> Self {
            let saved = keys
                .iter()
                .map(|key| (*key, std::env::var(key).ok()))
                .collect();
            for key in keys {
                std::env::remove_var(key);
            }
            Self { saved }
        }
    }

    impl Drop for EnvSnapshot {
        fn drop(&mut self) {
            for (key, value) in self.saved.drain(..) {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    #[test]
    fn retrieval_warmup_queries_default_to_source_lookups() {
        let _guard = crate::test_support::lock_test_env();
        let _env = EnvSnapshot::clean(WARMUP_ENV_KEYS);

        let queries = retrieval_warmup_queries();

        assert!(queries.iter().any(|q| q.contains("source")));
        assert!(queries.iter().any(|q| q.contains("manual")));
        assert!(queries.iter().any(|q| q.contains("来源")));
    }

    #[test]
    fn retrieval_warmup_queries_use_generic_env_and_cap_count() {
        let _guard = crate::test_support::lock_test_env();
        let _env = EnvSnapshot::clean(WARMUP_ENV_KEYS);
        std::env::set_var(
            "ATTUNE_RETRIEVAL_WARMUP_QUERIES",
            " q1 ; ; q2 ; q3 ; q4 ; q5 ; q6 ; q7 ; q8 ; q9 ",
        );
        std::env::set_var(
            "ATTUNE_SCHEDULER_RETRIEVAL_WARMUP_QUERIES",
            "scheduler-only",
        );

        let queries = retrieval_warmup_queries();

        assert_eq!(queries.len(), 8);
        assert_eq!(queries[0], "q1");
        assert_eq!(queries[7], "q8");
        assert!(!queries.iter().any(|q| q == "scheduler-only"));
    }

    #[test]
    fn retrieval_warmup_limits_use_scheduler_fallbacks_and_top_k_clamp() {
        let _guard = crate::test_support::lock_test_env();
        let _env = EnvSnapshot::clean(WARMUP_ENV_KEYS);
        std::env::set_var("ATTUNE_SCHEDULER_RETRIEVAL_WARMUP_METADATA_LIMIT", "123");
        std::env::set_var("ATTUNE_SCHEDULER_RETRIEVAL_WARMUP_TOP_K", "99");

        assert_eq!(retrieval_warmup_metadata_limit(), 123);
        assert_eq!(retrieval_warmup_top_k(), 20);
    }

    fn install_blocking_scheduler_reranker(state: &SharedState) {
        let install_state = state.clone();
        std::thread::spawn(move || {
            install_state.set_reranker(Some(std::sync::Arc::new(
                attune_core::infer::reranker::LocalSchedulerRerankProvider::new(
                    "http://127.0.0.1:8090",
                    "kb.query.rerank",
                    60_000,
                ),
            )));
        })
        .join()
        .expect("install scheduler reranker");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn vault_lock_drops_blocking_scheduler_client_off_tokio_worker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("open vault");
        vault.setup("vault-lock-runtime-test").expect("setup vault");
        let state = std::sync::Arc::new(crate::state::AppState::new(vault, false));
        install_blocking_scheduler_reranker(&state);

        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            lock_and_clear_runtime(&state),
        )
        .await
        .expect("vault lock must not reset the async connection")
        .expect("vault lock must succeed");

        assert!(matches!(
            state
                .vault
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .state(),
            attune_core::vault::VaultState::Locked
        ));
        assert!(state.reranker().is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn canceled_vault_lock_keeps_member_transition_until_transaction_finishes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("open vault");
        vault
            .setup("vault-lock-cancellation-test")
            .expect("setup vault");
        let state = std::sync::Arc::new(crate::state::AppState::new(vault, false));
        install_blocking_scheduler_reranker(&state);

        // Hold the vault mutex so the detached blocking transaction cannot
        // finish before we cancel its async waiter.
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let holder_state = state.clone();
        let holder = std::thread::spawn(move || {
            let _vault = holder_state.vault.lock().unwrap_or_else(|e| e.into_inner());
            held_tx.send(()).expect("signal held vault");
            release_rx.recv().expect("release held vault");
        });
        held_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("vault mutex holder started");

        let task_state = state.clone();
        let lock_task = tokio::spawn(async move { lock_and_clear_runtime(&task_state).await });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if state.member_transition.try_lock().is_err() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("blocking transaction acquired member transition");

        lock_task.abort();
        let _ = lock_task.await;
        assert!(
            state.member_transition.try_lock().is_err(),
            "canceling the waiter must not release the transaction guard"
        );

        release_tx.send(()).expect("release held vault");
        tokio::task::spawn_blocking(move || holder.join().expect("vault holder thread"))
            .await
            .expect("join vault holder");

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Ok(guard) = state.member_transition.try_lock() {
                    drop(guard);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("detached blocking transaction completed");
        assert!(matches!(
            state
                .vault
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .state(),
            attune_core::vault::VaultState::Locked
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn forgot_password_reset_clear_drops_blocking_scheduler_client_off_tokio_worker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("open vault");
        vault
            .setup("vault-reset-runtime-test")
            .expect("setup vault");
        let state = std::sync::Arc::new(crate::state::AppState::new(vault, false));
        install_blocking_scheduler_reranker(&state);

        let lock_state = state.clone();
        std::thread::spawn(move || {
            lock_state
                .vault
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .lock()
                .expect("lock vault without clearing runtime");
        })
        .join()
        .expect("lock vault thread");

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            vault_forgot_password_reset(
                State(state.clone()),
                Json(ForgotPasswordResetRequest {
                    // An invalid confirmation still exercises the route's
                    // clear-before-reset lifecycle without deleting any real
                    // platform data directory from this parallel unit test.
                    confirmation: "WRONG".into(),
                }),
            ),
        )
        .await
        .expect("vault reset clear must not reset the async connection");

        assert!(
            result.is_err(),
            "invalid reset confirmation must be rejected"
        );
        assert!(matches!(
            state
                .vault
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .state(),
            attune_core::vault::VaultState::Locked
        ));
        assert!(state.reranker().is_none());
    }
}
