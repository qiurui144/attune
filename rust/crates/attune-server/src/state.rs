use attune_core::classifier::Classifier;
use attune_core::clusterer::ClusterSnapshot;
use attune_core::embed::{
    EmbeddingProvider, LocalSchedulerEmbeddingProvider, OpenAiEmbeddingProvider,
};
use attune_core::index::FulltextIndex;
use attune_core::llm::{LlmProvider, LocalSchedulerInferLlmProvider, OpenAiLlmProvider};
use attune_core::outbound_gate::{OutboundGate, OutboundKind, OutboundPolicy};
use attune_core::pii::Redactor;
use attune_core::resource_governor::{global_registry, TaskKind};
use attune_core::tag_index::TagIndex;
use attune_core::taxonomy::Taxonomy;
use attune_core::vault::Vault;
use attune_core::vectors::VectorIndex;
use attune_core::vlm::{LlmVlmProvider, VlmProvider};
use attune_core::web_search::WebSearchProvider;
use lru::LruCache;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const SEARCH_CACHE_CAPACITY: usize = 256;
const SEARCH_CACHE_TTL_SECS: u64 = 30;
const DEFAULT_EMBED_QUEUE_BATCH_SIZE: u32 = 512;
const MAX_EMBED_QUEUE_BATCH_SIZE: u32 = 2048;

fn embed_queue_batch_size() -> usize {
    crate::local_scheduler::env_u32_any(
        &[
            "ATTUNE_EMBED_QUEUE_BATCH_SIZE",
            "ATTUNE_EMBED_BATCH_SIZE",
            "ATTUNE_INDEX_EMBED_BATCH_SIZE",
        ],
        DEFAULT_EMBED_QUEUE_BATCH_SIZE,
    )
    .clamp(1, MAX_EMBED_QUEUE_BATCH_SIZE) as usize
}

/// Apply the single cloud-embedding boundary and return the only payload that
/// may be sent to the provider. Queue and memory callers must embed this return
/// value, never the original input.
fn enforce_cloud_embedding_payload(
    enabled: bool,
    vault_unlocked: bool,
    contains_l0: bool,
    redactor: &Redactor,
    payload: &str,
) -> Result<String, attune_core::outbound_gate::OutboundError> {
    let policy = OutboundPolicy {
        kind: OutboundKind::Embedding,
        enabled,
        vault_unlocked,
        redactor: Some(redactor),
        local_destination: false,
        contains_l0,
    };
    OutboundGate::enforce(&policy, payload)
}

/// Prepare one classifier input at the local/cloud boundary. Local classifiers
/// receive the original text. Cloud classifiers require consent, an unlocked
/// vault, non-L0 content, and receive only the gate's redacted strings.
pub(crate) fn govern_classification_input(
    local: bool,
    enabled: bool,
    vault_unlocked: bool,
    contains_l0: bool,
    redactor: &Redactor,
    title: &str,
    content: &str,
) -> Result<(String, String), attune_core::outbound_gate::OutboundError> {
    if local {
        return Ok((title.to_string(), content.to_string()));
    }
    let policy = OutboundPolicy {
        kind: OutboundKind::Llm,
        enabled,
        vault_unlocked,
        redactor: Some(redactor),
        local_destination: false,
        contains_l0,
    };
    let title = OutboundGate::enforce(&policy, title)?;
    let content = OutboundGate::enforce(&policy, content)?;
    Ok((title, content))
}

fn llm_privacy_enabled(settings: &Option<serde_json::Value>) -> bool {
    settings
        .as_ref()
        .and_then(|value| value.pointer("/privacy/llm"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn env_bool_override(keys: &[&str]) -> Option<bool> {
    keys.iter().find_map(|key| {
        std::env::var(key).ok().map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
    })
}

fn classify_worker_auto_enabled(settings: &Option<serde_json::Value>) -> bool {
    classify_worker_auto_enabled_with_override(
        settings,
        env_bool_override(&[
            "ATTUNE_CLASSIFY_WORKER_ENABLED",
            "ATTUNE_AUTO_CLASSIFY_WORKER_ENABLED",
        ]),
    )
}

fn classify_worker_auto_enabled_with_override(
    settings: &Option<serde_json::Value>,
    env_override: Option<bool>,
) -> bool {
    if let Some(enabled) = env_override {
        return enabled;
    }
    if let Some(enabled) = settings
        .as_ref()
        .and_then(|value| value.pointer("/classification/auto_worker_enabled"))
        .and_then(serde_json::Value::as_bool)
    {
        return enabled;
    }
    let Some(settings) = settings.as_ref() else {
        return true;
    };
    !crate::local_scheduler::native_kb_ask_enabled(settings)
}

pub struct CachedSearch {
    pub query: String,
    pub results: Vec<attune_core::search::SearchResult>,
    pub created_at: Instant,
}

impl CachedSearch {
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed().as_secs() >= SEARCH_CACHE_TTL_SECS
    }
}

pub type SharedState = Arc<AppState>;

#[derive(Clone, Debug, serde::Serialize)]
pub struct BackgroundScanTaskStatus {
    pub task_id: String,
    pub dir_id: String,
    pub path: String,
    pub status: String,
    pub progress: f32,
    pub message: String,
    pub total: Option<usize>,
    pub new: Option<usize>,
    pub updated: Option<usize>,
    pub skipped: Option<usize>,
    pub deleted: Option<usize>,
    pub degraded: Option<usize>,
    pub errors: Option<usize>,
    pub elapsed_ms: Option<u128>,
}

/// A bearer token that was successfully verified immediately before the vault
/// was locked. Vault locking deliberately invalidates normal sessions, but the
/// privacy dashboard must still let that same authenticated caller inspect the
/// locked state and wipe its cloud session. Only a digest is retained, and the
/// original session expiry remains authoritative.
struct LockedPrivacyAuthorization {
    token_digest: [u8; 32],
    expires_at: i64,
}

/// Fully-built embedding runtime waiting for a generation-checked publish.
///
/// Keeping construction outside `runtime_install_guard` avoids blocking vault
/// lock while indexes are rebuilt. The whole candidate is then either
/// published for the generation it was derived from or dropped without
/// exposing any of its credential-bearing providers.
struct EmbeddingRuntimeCandidate {
    provider: Arc<dyn EmbeddingProvider>,
    is_local: bool,
    reranker: Arc<dyn attune_core::infer::RerankProvider>,
    vectors: Option<VectorIndex>,
    memory_index: Option<attune_core::memory::MemoryVectorIndex>,
}

pub struct AppState {
    pub vault: Mutex<Vault>,
    pub fulltext: Mutex<Option<FulltextIndex>>,
    pub vectors: Mutex<Option<VectorIndex>>,
    /// Multi-layer memory (2026-05-18): dedicated vector index over L2/L3 memory
    /// summaries so the tier-aware assembler can rank them. Built at unlock from
    /// `memory_vectors`; `None` until the embedding dimension is known.
    pub memory_index: Mutex<Option<attune_core::memory::MemoryVectorIndex>>,
    pub embedding: Mutex<Option<Arc<dyn EmbeddingProvider>>>,
    pub reranker: Mutex<Option<Arc<dyn attune_core::infer::RerankProvider>>>,
    /// #2 #5: 底座模型后台下载进度快照（embedding / reranker / ocr / asr）。
    /// 解锁立即返回，模型在后台线程拉取；本字段让 /ai_stack 暴露进度 + 失败原因，
    /// 不静默。clone 廉价（内部 Arc）。
    pub model_bootstrap: attune_core::infer::bootstrap_status::ModelBootstrapStatus,
    /// EP 运行时软件栈按需安装进度快照（cuda / openvino / rocm / directml / vitisai）。
    /// 与 `model_bootstrap` 平行：栈像底座模型一样首次运行按需拉取 userspace runtime
    /// （内核驱动除外，走 #6 consent）。/ai_stack 暴露状态。clone 廉价（内部 Arc）。
    pub stack_install: attune_core::infer::stack_installer::StackInstallStatus,
    pub llm: Mutex<Option<Arc<dyn LlmProvider>>>,
    pub summary_llm: Mutex<Option<Arc<dyn LlmProvider>>>,
    pub web_search: Mutex<Option<Arc<dyn WebSearchProvider>>>,
    /// VLM provider — 图片 caption / VQA。与主 LLM 由 `reload_llm`
    /// 同一事务安装；无 vision-capable LLM 时为 None。
    pub vlm: Mutex<Option<Arc<dyn VlmProvider>>>,
    pub tag_index: Mutex<Option<TagIndex>>,
    pub cluster_snapshot: Mutex<Option<ClusterSnapshot>>,
    pub taxonomy: Mutex<Option<Arc<Taxonomy>>>,
    pub classifier: Mutex<Option<Arc<Classifier>>>,
    pub require_auth: bool,
    locked_privacy_authorization: Mutex<Option<LockedPrivacyAuthorization>>,
    /// Invalidates post-unlock bootstrap work when the vault locks. Runtime
    /// installs take `runtime_install_guard` for the final epoch/state check so
    /// a background task cannot resurrect a provider after lock cleanup.
    runtime_generation: AtomicU64,
    /// Invalidates providers derived from an older account/settings snapshot
    /// while the vault stays unlocked. This closes logout/account-switch races
    /// that a vault-only generation cannot observe.
    credential_generation: AtomicU64,
    /// Per-provider last-start-wins epochs. They prevent a slower reload that
    /// read an intermediate account/settings snapshot from overwriting a newer
    /// candidate within the same credential generation.
    llm_reload_generation: AtomicU64,
    plugin_hub_reload_generation: AtomicU64,
    embedding_reload_generation: AtomicU64,
    runtime_install_guard: Mutex<()>,
    /// 启动时检测一次的硬件画像；之后 settings/diagnostics 都读这份缓存，
    /// 避免每次请求都同步读 /proc、调 sysctl/wmic 阻塞 async worker。
    /// 见 platform.rs HardwareProfile::detect()。
    pub hardware: attune_core::platform::HardwareProfile,
    /// 防止重复启动 QueueWorker 后台线程
    pub queue_worker_running: AtomicBool,
    /// 防止重复启动 ClassifyWorker 后台线程
    pub classify_worker_running: AtomicBool,
    /// 防止重复启动 RescanWorker 后台线程
    pub rescan_worker_running: AtomicBool,
    /// 防止并发 unlock 重复初始化搜索引擎（重建索引会清空内存向量）
    pub engines_initialized: AtomicBool,
    /// 防止重复启动 SkillEvolver 后台线程
    pub evolve_worker_running: AtomicBool,
    /// 防止重复启动 MemoryConsolidator 后台线程（A1，2026-04-27）
    pub memory_consolidator_running: AtomicBool,
    /// v0.7 记忆护城河：防止重复启动 ReindexWorker 后台线程（消费 reindex_queue
    /// 让 scanner / scanner_webdav 等无法持锁的 worker 间接清向量+FTS）。
    pub reindex_worker_running: AtomicBool,
    /// 记忆延续（2026-06-15）：换 embedding 模型后老记忆向量的批量 reindex 暂停开关。
    /// 不同于 reindex_worker_running（那是 item 语料的 reindex_queue 消费）—— 这是
    /// memory_vectors 的维度键迁移，并入消费者后台 loop（run_memory_reindex_batch），
    /// POST /memory/reindex {pause:true} 可暂停（reindex 是 tier-2 本地算力，可控）。
    pub memory_reindex_paused: AtomicBool,
    /// WebDAV 周期同步 worker 是否在运行（防重复启动）。
    pub webdav_sync_worker_running: AtomicBool,
    /// Email 周期同步 worker 运行标志（防重入）。
    pub email_sync_worker_running: AtomicBool,
    /// RSS 周期同步 worker 运行标志（防重入）。
    pub rss_sync_worker_running: AtomicBool,
    /// 信息监控 digest worker 运行标志（防重入；spec 2026-06-19）。
    pub monitoring_worker_running: AtomicBool,
    /// G3① locked-mode staging drain worker 运行标志（防重入）。解锁后启动,
    /// 把 LOCKED 期间暂存的 inbound 文档补跑进 ingest pipeline,跑完即退出。
    pub staging_drain_worker_running: AtomicBool,
    pub search_cache: Mutex<LruCache<u64, CachedSearch>>,
    /// Latest in-process folder bind scans, exposed through /api/v1/index/status
    /// so long-running background imports are observable without a WebSocket.
    pub background_scan_tasks: Mutex<HashMap<String, BackgroundScanTaskStatus>>,
    /// G5 (2026-06-11): durable job queue store handle. Replaces the in-memory
    /// `office_jobs: JobRegistry` — jobs now persist in the `job_queue` table and
    /// survive restart (Running→Queued requeue for idempotent kinds). Like the
    /// usage aggregator, this is its **own** `Arc<Mutex<Store>>` opened on
    /// `db_path` (job_queue is an unencrypted table; SQLite WAL makes the extra
    /// connection safe). `None` until `install_job_store` succeeds at boot.
    /// See docs/superpowers/specs/2026-06-22-durable-job-queue.md.
    pub job_store: Mutex<Option<std::sync::Arc<std::sync::Mutex<attune_core::store::Store>>>>,
    /// 防止重复启动 G5 durable job worker 后台 task
    pub job_worker_running: AtomicBool,
    /// #82 P0 privacy fix: true when the active embedding provider is scheduler-local.
    /// Set by build_embedding_from_settings.
    /// The queue worker reads this to know whether to enforce the OutboundGate
    /// L0 + disabled check before each embedding HTTP call. Local providers are
    /// always permitted; cloud providers (OpenAI-compat endpoint) are gated.
    // Guarded by `embedding` for provider/locality snapshots. The atomic is
    // retained for cheap storage, but callers must use
    // `embedding_with_locality()` so a hot swap cannot pair an old cloud
    // provider with a new `local=true` bit.
    embedding_is_local: AtomicBool,
    /// Sprint 1 Phase B: project recommendation broadcast channel.
    /// upload.rs / chat.rs 收到信号后 send；ws.rs subscribe 推送给前端。
    pub recommendation_tx: tokio::sync::broadcast::Sender<serde_json::Value>,
    /// Sprint 2: 启动时加载的 plugins（attune-pro / 用户 / 社区）
    pub plugin_registry: std::sync::Arc<attune_core::plugin_registry::PluginRegistry>,
    /// 会员登录状态 — 控制 SettingsLocks 灰显 / 锁定 PATCH /settings 字段.
    /// 默认 LoggedOut (本地 self-host). login 后由 cloud_client.me() 推导.
    pub member_state: Mutex<attune_core::member_session::MemberState>,
    /// Serialize every membership/account transition across its remote proof,
    /// persisted-session fence, credential cleanup, and runtime publication.
    /// A plain `member_state` snapshot lock is insufficient here: two requests
    /// could otherwise interleave the shared cloud-session marker and publish
    /// different accounts to memory and disk. Tokio's mutex may be held across
    /// the blocking-task awaits used by the cloud client without blocking an
    /// async worker thread.
    pub(crate) member_transition: tokio::sync::Mutex<()>,
    /// Path-bound persisted cloud session store. Capture once at construction;
    /// every verifier/route then participates in the same account transaction
    /// even if the host process environment changes later.
    pub(crate) cloud_session_store: attune_core::cloud_session::CloudSessionStore,
    /// Session-file epoch paired with the currently published logged-in member
    /// runtime. `None` means no runtime is bound. API middleware compares this
    /// with disk to detect sequential CLI/server account switches.
    pub(crate) member_session_epoch: Mutex<Option<String>>,
    /// Last authoritative cloud proof for the currently published Paid state.
    /// Request middleware refreshes it on a bounded cadence and tears down Paid
    /// after an authoritative deny or a bounded network grace interval.
    pub(crate) member_verified_at: Mutex<Option<Instant>>,
    /// C1 paywall-bypass fix: server-side verifier for a "paid" claim. `login_token` MUST run
    /// this before granting `MemberState::Paid` so a forged `{tier:paid, license_id:..}` cannot
    /// reach a billable tier-3 op. Default = `CloudMemberVerifier` (verifies against the cloud
    /// session, fail-closed). Tests inject a verifier that performs a real (offline) match.
    pub member_verifier: Mutex<std::sync::Arc<dyn attune_core::member_verifier::MemberVerifier>>,
    /// E2/E4 (2026-05-01): PluginHub 客户端 (Mutex 让 PATCH /settings 能热更新)
    /// 默认 Mock；settings.pluginhub.url + license_key 配齐后切到 HttpPluginHubProvider
    pub plugin_hub: Mutex<std::sync::Arc<dyn attune_core::plugin_hub::PluginHubProvider>>,
    /// Plan A1 (2026-05-28): in-process cost-aware token usage ring buffer + flusher.
    ///
    /// Lifecycle:
    /// - `new()` initializes to `None` — the aggregator needs an `Arc<Mutex<Store>>`
    ///   handle which is only realizable after the vault layer exposes a sharable
    ///   store accessor (deferred to a follow-up; current `Vault` owns `Store` by
    ///   value).
    /// - `set_usage` is the install point; once an aggregator is constructed
    ///   downstream it is parked here and `usage()` returns `Some` until shutdown.
    /// - Plan A2's `CapabilityRouter` will call `state.usage()?.recent(N)` for
    ///   routing-feedback decisions.
    pub usage_aggregator: Mutex<Option<std::sync::Arc<attune_core::usage::UsageAggregator>>>,
    /// Plan A1 (2026-05-28): cost-aware response cache backend (L1 in-memory by
    /// default; SqliteEncryptedCache can be installed via `set_cache_backend`
    /// once the vault is unlocked).
    pub cache_backend: Mutex<Option<std::sync::Arc<dyn attune_core::cache::CacheBackend>>>,
    /// ACP-5 (2026-05-29): the workspace agent registry + declarative flow DAGs
    /// (`agents.registry.toml` + `agent_flows.toml`), loaded + typed-handoff
    /// validated once at startup. `None` when the files are absent (an OSS install
    /// shipping no agents) or fail to load — the chat path then never runs a flow
    /// and falls back to free-form RAG (spec §7 / §11 R8, never hard-fail chat).
    /// `Arc` so the chat handler can clone a cheap handle out of `&AppState`.
    pub agent_flows: Option<
        std::sync::Arc<(
            attune_core::agents::flow::FlowSet,
            attune_core::agents::registry::AgentRegistry,
        )>,
    >,
    /// Trust-chain T5/T8: in-memory entitlement cache (Arc<RwLock> inside).
    /// Hydrated from `plugin_entitlements` at unlock; the re-verify worker +
    /// `POST /member/entitlements/refresh` update it. Independent lock — never
    /// nested with fulltext/vectors/vault (spec §3.3 / lock-ordering 铁律).
    pub entitlement_cache: attune_core::entitlement::EntitlementCache,
    /// 防止重复启动 entitlement re-verify worker 后台线程 (T8)。
    pub entitlement_worker_running: AtomicBool,
    /// 文件夹一键整理 (organize/analyze) 的领域策略注册表。默认仅含领域无关的
    /// `GenericStrategy`(主题命名 / 无角色 / kind=collection);attune-pro 在启动时
    /// 经 `register()` 注入 `LawCaseStrategy` 等行业策略。`Arc` 让 analyze handler
    /// 廉价 clone 出 `Arc<dyn OrganizationStrategy>` 而不持 AppState 锁。
    pub strategy_registry: std::sync::Arc<attune_core::organizer::strategy::StrategyRegistry>,
    /// Capability Registry (P0 ②, spec 2026-06-26): 「重能力」元数据的单一真相源。
    /// 在 `new` 里 seed 10 个内置 OSS 重能力(embedding/reranker/ocr/asr/tts/llm/vlm/
    /// web-search/pluginhub/marketplace);health/enabled 由 `refresh_capability_health`
    /// 从既有 model_bootstrap/provider presence/member_state 投影。独立锁(内部
    /// Arc<RwLock>),从不嵌套在 vault/vectors/fulltext guard 内。
    pub capabilities: attune_core::capability::CapabilityRegistry,
}

fn session_token_expiry(token: &str) -> Option<i64> {
    let payload = token.rsplit_once('.')?.0;
    let mut parts = payload.splitn(3, ':');
    let _session_id = parts.next()?;
    parts.next()?.parse().ok()
}

impl AppState {
    /// Retain a narrowly scoped proof that `token` passed the normal vault
    /// verifier before a lock request. The raw bearer is never retained.
    pub(crate) fn arm_locked_privacy_authorization(&self, token: &str) -> bool {
        let Some(expires_at) = session_token_expiry(token) else {
            return false;
        };
        if chrono::Utc::now().timestamp() > expires_at {
            return false;
        }
        let token_digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        *self
            .locked_privacy_authorization
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(LockedPrivacyAuthorization {
            token_digest,
            expires_at,
        });
        true
    }

    /// Verify the lock-surviving privacy capability. Callers must additionally
    /// restrict this to the exact status/wipe routes and to `VaultState::Locked`.
    pub(crate) fn verify_locked_privacy_authorization(&self, token: &str) -> bool {
        let now = chrono::Utc::now().timestamp();
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let mut cached = self
            .locked_privacy_authorization
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(authorization) = cached.as_ref() else {
            return false;
        };
        if now > authorization.expires_at {
            *cached = None;
            return false;
        }
        digest == authorization.token_digest
    }

    /// A successful unlock establishes a new nonce/session generation, so a
    /// capability retained for the previous locked generation must be dropped.
    pub(crate) fn clear_locked_privacy_authorization(&self) {
        *self
            .locked_privacy_authorization
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn runtime_generation(&self) -> u64 {
        self.runtime_generation.load(Ordering::SeqCst)
    }

    fn credential_generation(&self) -> u64 {
        self.credential_generation.load(Ordering::SeqCst)
    }

    /// Invalidate every in-flight provider candidate derived from an older
    /// account or settings snapshot. The same install guard used by publisher
    /// CAS makes the bump atomic with respect to final handle publication.
    pub(crate) fn invalidate_credential_generation(&self) {
        let _install_guard = self
            .runtime_install_guard
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        self.credential_generation.fetch_add(1, Ordering::SeqCst);
    }

    fn begin_credential_reload(&self, reload_counter: &AtomicU64) -> (u64, u64, u64) {
        let _install_guard = self
            .runtime_install_guard
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let reload_generation = reload_counter.fetch_add(1, Ordering::SeqCst) + 1;
        (
            self.runtime_generation(),
            self.credential_generation(),
            reload_generation,
        )
    }

    fn begin_runtime_reload(&self, reload_counter: &AtomicU64) -> (u64, u64) {
        let _install_guard = self
            .runtime_install_guard
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let reload_generation = reload_counter.fetch_add(1, Ordering::SeqCst) + 1;
        (self.runtime_generation(), reload_generation)
    }

    /// Run a short runtime-handle install only if it still belongs to the
    /// current unlocked generation. The guard closes the check/install race
    /// with `lock_vault_and_clear_runtime`.
    fn install_runtime_if_current(&self, generation: u64, install: impl FnOnce()) -> bool {
        let _install_guard = self
            .runtime_install_guard
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if self.runtime_generation() != generation {
            return false;
        }
        let unlocked = {
            let vault = self.vault.lock().unwrap_or_else(|e| e.into_inner());
            matches!(vault.state(), attune_core::vault::VaultState::Unlocked)
        };
        if !unlocked {
            return false;
        }
        install();
        true
    }

    fn install_credential_runtime_if_current(
        &self,
        runtime_generation: u64,
        credential_generation: u64,
        reload_counter: &AtomicU64,
        reload_generation: u64,
        install: impl FnOnce(),
    ) -> bool {
        let _install_guard = self
            .runtime_install_guard
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if self.runtime_generation() != runtime_generation
            || self.credential_generation() != credential_generation
            || reload_counter.load(Ordering::SeqCst) != reload_generation
        {
            return false;
        }
        let unlocked = {
            let vault = self.vault.lock().unwrap_or_else(|e| e.into_inner());
            matches!(vault.state(), attune_core::vault::VaultState::Unlocked)
        };
        if !unlocked {
            return false;
        }
        install();
        true
    }

    fn install_reload_runtime_if_current(
        &self,
        runtime_generation: u64,
        reload_counter: &AtomicU64,
        reload_generation: u64,
        install: impl FnOnce(),
    ) -> bool {
        let _install_guard = self
            .runtime_install_guard
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if self.runtime_generation() != runtime_generation
            || reload_counter.load(Ordering::SeqCst) != reload_generation
        {
            return false;
        }
        let unlocked = {
            let vault = self.vault.lock().unwrap_or_else(|e| e.into_inner());
            matches!(vault.state(), attune_core::vault::VaultState::Unlocked)
        };
        if !unlocked {
            return false;
        }
        install();
        true
    }

    pub fn new(vault: Vault, require_auth: bool) -> Self {
        let (recommendation_tx, _rx) = tokio::sync::broadcast::channel::<serde_json::Value>(64);
        let cloud_session_store = attune_core::cloud_session::CloudSessionStore::default();
        // 2026-05-20: 启动时 LicenseCache::load 的 paid-plugin 解密 key fallback 是死路径.
        // 历史 cloud_client.list_licenses() 下发的 license_key 是 Bearer token, 不是
        // SignedLicense code — attune-cli 已经跳过写 LicenseCache (see main.rs:784-786);
        // 这里读出来也永远是 None. 直接走明文 scan; encrypted plugin 走 plugin_sync 路径
        // (它从 cloud_client.EntitledPlugin.decrypt_key 直接拿 key, 不经此 cache).
        let cached_license_key: Option<Vec<u8>> = None;
        let plugin_registry =
            match attune_core::plugin_registry::PluginRegistry::default_plugins_dir() {
                Ok(dir) => match attune_core::plugin_registry::PluginRegistry::scan_with_key(
                    &dir,
                    cached_license_key.as_deref(),
                ) {
                    Ok((reg, errs)) => {
                        tracing::info!(
                            "loaded {} plugins, {} workflows from {}",
                            reg.plugins().count(),
                            reg.workflows().len(),
                            dir.display()
                        );
                        for e in &errs {
                            tracing::warn!("plugin load error: {}", e);
                        }
                        std::sync::Arc::new(reg)
                    }
                    Err(e) => {
                        tracing::warn!("plugin scan failed: {}", e);
                        std::sync::Arc::new(attune_core::plugin_registry::PluginRegistry::new())
                    }
                },
                Err(e) => {
                    tracing::warn!("cannot resolve plugin dir: {}", e);
                    std::sync::Arc::new(attune_core::plugin_registry::PluginRegistry::new())
                }
            };
        let state = Self {
            vault: Mutex::new(vault),
            fulltext: Mutex::new(None),
            vectors: Mutex::new(None),
            memory_index: Mutex::new(None),
            embedding: Mutex::new(None),
            reranker: Mutex::new(None),
            model_bootstrap: attune_core::infer::bootstrap_status::ModelBootstrapStatus::new(),
            stack_install: attune_core::infer::stack_installer::StackInstallStatus::new(),
            llm: Mutex::new(None),
            summary_llm: Mutex::new(None),
            web_search: Mutex::new(None),
            vlm: Mutex::new(None),
            tag_index: Mutex::new(None),
            cluster_snapshot: Mutex::new(None),
            taxonomy: Mutex::new(None),
            classifier: Mutex::new(None),
            require_auth,
            locked_privacy_authorization: Mutex::new(None),
            runtime_generation: AtomicU64::new(0),
            credential_generation: AtomicU64::new(0),
            llm_reload_generation: AtomicU64::new(0),
            plugin_hub_reload_generation: AtomicU64::new(0),
            embedding_reload_generation: AtomicU64::new(0),
            runtime_install_guard: Mutex::new(()),
            queue_worker_running: AtomicBool::new(false),
            classify_worker_running: AtomicBool::new(false),
            rescan_worker_running: AtomicBool::new(false),
            evolve_worker_running: AtomicBool::new(false),
            memory_consolidator_running: AtomicBool::new(false),
            reindex_worker_running: AtomicBool::new(false),
            memory_reindex_paused: AtomicBool::new(false),
            webdav_sync_worker_running: AtomicBool::new(false),
            email_sync_worker_running: AtomicBool::new(false),
            rss_sync_worker_running: AtomicBool::new(false),
            monitoring_worker_running: AtomicBool::new(false),
            staging_drain_worker_running: AtomicBool::new(false),
            engines_initialized: AtomicBool::new(false),
            search_cache: Mutex::new(LruCache::new(
                NonZeroUsize::new(SEARCH_CACHE_CAPACITY)
                    .expect("SEARCH_CACHE_CAPACITY is non-zero const"),
            )),
            background_scan_tasks: Mutex::new(HashMap::new()),
            job_store: Mutex::new(None),
            job_worker_running: AtomicBool::new(false),
            embedding_is_local: AtomicBool::new(false),
            // 启动时检测一次硬件，后续复用（避免每次 GET/PATCH 都同步读 /proc 等）
            hardware: attune_core::platform::HardwareProfile::detect(),
            recommendation_tx,
            plugin_registry,
            // E2/E4 + G2: 默认 Mock；settings.pluginhub.url + license_key 配齐后
            // 由 reload_plugin_hub() 切到 HttpPluginHubProvider
            plugin_hub: Mutex::new(std::sync::Arc::new(
                attune_core::plugin_hub::MockPluginHubProvider::default(),
            )),
            // 默认未登录 — 本地 self-host 模式. login 后通过 /member/login endpoint 更新.
            member_state: Mutex::new(attune_core::member_session::MemberState::LoggedOut),
            member_transition: tokio::sync::Mutex::new(()),
            cloud_session_store: cloud_session_store.clone(),
            member_session_epoch: Mutex::new(None),
            member_verified_at: Mutex::new(None),
            // C1: default verifier proves paid claims against the cloud session (fail-closed).
            member_verifier: Mutex::new(std::sync::Arc::new(
                attune_core::member_verifier::CloudMemberVerifier::new(cloud_session_store),
            )),
            // Plan A1 — UsageAggregator stays None until a vault-bound Store handle
            // exists (see field docs); cache_backend defaults to in-memory L1.
            usage_aggregator: Mutex::new(None),
            cache_backend: Mutex::new(Some(std::sync::Arc::new(
                attune_core::cache::memory::MemoryLruCache::new(512),
            ))),
            // ACP-5: load + validate the workspace flow DAGs once at startup.
            // Absent files / parse / validation failure → None (chat degrades to
            // free-form RAG; never panic — spec §11 R8).
            agent_flows: match attune_core::agents::load_workspace_flows(
                "agents.registry.toml",
                "agent_flows.toml",
            ) {
                Ok((flows, reg)) => {
                    tracing::info!(
                        "ACP-5: loaded {} agent flows, {} agents from workspace",
                        flows.len(),
                        reg.len()
                    );
                    Some(std::sync::Arc::new((flows, reg)))
                }
                Err(e) => {
                    tracing::info!(
                        "ACP-5: no agent flows loaded ({e}); chat uses free-form RAG only"
                    );
                    None
                }
            },
            // Trust-chain T5/T8: empty until hydrated from vault at unlock.
            entitlement_cache: attune_core::entitlement::EntitlementCache::new(),
            entitlement_worker_running: AtomicBool::new(false),
            // Default registry = GenericStrategy only (OSS boundary: no industry
            // semantics in attune-core). attune-pro injects LawCaseStrategy at boot.
            strategy_registry: std::sync::Arc::new(
                attune_core::organizer::strategy::StrategyRegistry::new(),
            ),
            // Capability Registry starts empty; seeded below before returning.
            capabilities: attune_core::capability::CapabilityRegistry::new(),
        };
        // P0 ②: seed the 10 builtin OSS heavy-capability descriptors. health/enabled
        // are placeholders here (Ok / default); real runtime health is projected at
        // request time by `refresh_capability_health` (the diagnostics handler).
        state.register_builtin_capabilities();
        state
    }

    /// P0 ② (spec 2026-06-26): seed the static builtin OSS capability descriptors.
    /// Called once at the end of `AppState::new`. Pure metadata — registers nothing
    /// Pro/Enterprise (OSS boundary: attune-core/attune-server self-register only OSS;
    /// attune-pro extends the registry separately). Idempotent on id.
    fn register_builtin_capabilities(&self) {
        use attune_core::capability::{Capability, CapabilityKind};
        let r = &self.capabilities;
        // Local-first models (require a local model, no outbound by default).
        r.register(
            Capability::builtin("embedding", "Embedding", CapabilityKind::Model)
                .requires_local_model(true),
        );
        r.register(
            Capability::builtin("reranker", "Reranker", CapabilityKind::Model)
                .requires_local_model(true),
        );
        r.register(
            Capability::builtin("ocr", "OCR", CapabilityKind::Feature).requires_local_model(true),
        );
        r.register(
            Capability::builtin("asr", "ASR", CapabilityKind::Feature).requires_local_model(true),
        );
        r.register(
            Capability::builtin("tts", "TTS", CapabilityKind::Feature)
                .requires_local_model(true)
                .health(attune_core::capability::CapabilityHealth::Unavailable)
                .enabled(false),
        );
        // Cloud-default models (outbound by default; LLM not bundled per M2).
        r.register(Capability::builtin("llm", "LLM", CapabilityKind::Model).allows_outbound(true));
        r.register(Capability::builtin("vlm", "VLM", CapabilityKind::Model).allows_outbound(true));
        // Outbound source.
        r.register(
            Capability::builtin("web-search", "Web Search", CapabilityKind::Source)
                .allows_outbound(true),
        );
        // Member-gated feature that reaches the hub over the network.
        r.register(
            Capability::builtin("pluginhub", "Plugin Hub", CapabilityKind::Feature)
                .requires_member(true)
                .allows_outbound(true),
        );
        // OSS plugin marketplace surface (tier=Oss — NOT a Pro vertical).
        r.register(Capability::builtin(
            "marketplace",
            "Marketplace",
            CapabilityKind::Feature,
        ));
    }

    /// P0 ② (spec 2026-06-26): project current runtime signals onto the registry's
    /// `health` + `enabled` fields. Cheap, no I/O, no item/vault-content read.
    /// Called by the diagnostics handler before snapshotting.
    ///
    /// Data sources (all already in `AppState`): `model_bootstrap` phases for the
    /// four bootstrap models; last scheduler observation for TTS; provider
    /// presence (`llm()`/`vlm()`/`web_search()`); and
    /// `member_state` for the member-gated `pluginhub`. Lock discipline: the only
    /// foreign lock taken is `member_state` (independent of vault/vectors/fulltext);
    /// the registry write lock is independent too — never nested with the index
    /// hot-path locks, so this cannot ABBA-deadlock (spec §11 锁序).
    pub fn refresh_capability_health(&self) {
        use attune_core::capability::CapabilityHealth as H;
        use attune_core::infer::bootstrap_status::ModelPhase;

        // Base models: map ModelPhase → health, enabled iff Ready.
        for (id, class) in [
            ("embedding", "embedding"),
            ("reranker", "reranker"),
            ("ocr", "ocr"),
            ("asr", "asr"),
        ] {
            let h = match self.model_bootstrap.phase(class) {
                Some(ModelPhase::Ready) => H::Ok,
                Some(ModelPhase::Downloading) | Some(ModelPhase::Pending) => H::Installing,
                Some(ModelPhase::Failed { .. }) | None => H::Unavailable,
            };
            self.capabilities.set_health(id, h);
            self.capabilities.set_enabled(id, h == H::Ok);
        }

        // llm / vlm / web-search: provider presence.
        for (id, present) in [
            ("llm", self.llm().is_some()),
            ("vlm", self.vlm().is_some()),
            ("web-search", self.web_search().is_some()),
        ] {
            self.capabilities
                .set_health(id, if present { H::Ok } else { H::Unavailable });
            self.capabilities.set_enabled(id, present);
        }

        // pluginhub: paid unlocks the feature, but health is OK only when the
        // runtime provider is a real HttpPluginHubProvider. A paid user with the
        // offline mock hub still needs account activation/login to wire a license.
        let paid = self
            .member_state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_paid();
        self.capabilities.set_enabled("pluginhub", paid);
        let hub_ready = self
            .plugin_hub
            .lock()
            .map(|h| h.name() != "mock")
            .unwrap_or(false);
        self.capabilities.set_health(
            "pluginhub",
            if paid && hub_ready {
                H::Ok
            } else {
                H::Degraded
            },
        );

        // marketplace: OSS browse surface is always reachable.
        self.capabilities.set_health("marketplace", H::Ok);
    }

    /// Project the last scheduler contract/task observation onto the TTS
    /// capability. The async AI-stack probe and real synthesis route call this;
    /// diagnostics stays read-only and reports the most recent honest state.
    pub(crate) fn set_tts_capability_ready(&self, ready: bool) {
        use attune_core::capability::CapabilityHealth;
        self.capabilities.set_health(
            "tts",
            if ready {
                CapabilityHealth::Ok
            } else {
                CapabilityHealth::Unavailable
            },
        );
        self.capabilities.set_enabled("tts", ready);
    }

    /// 整理策略注册表的廉价句柄(Arc clone)。analyze handler 用它 resolve
    /// corpus_domain → strategy,无需持任何 AppState 锁。
    pub fn strategy_registry(
        &self,
    ) -> std::sync::Arc<attune_core::organizer::strategy::StrategyRegistry> {
        self.strategy_registry.clone()
    }

    /// G2 (2026-05-01) — 按 settings 切换 PluginHub provider
    /// 由 PATCH /api/v1/settings 在 pluginhub 字段变化时调
    pub fn reload_plugin_hub(&self, url: Option<&str>, license_key: Option<&str>) {
        let (runtime_generation, credential_generation, reload_generation) =
            self.begin_credential_reload(&self.plugin_hub_reload_generation);
        self.reload_plugin_hub_for_generation(
            runtime_generation,
            credential_generation,
            reload_generation,
            url,
            license_key,
        );
    }

    /// Build a PluginHub candidate and publish it only for the settings/account
    /// generation that selected its URL and license key.
    fn reload_plugin_hub_for_generation(
        &self,
        runtime_generation: u64,
        credential_generation: u64,
        reload_generation: u64,
        url: Option<&str>,
        license_key: Option<&str>,
    ) {
        let configured_url = url.filter(|value| !value.is_empty()).map(str::to_string);
        let configured_key = license_key
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let new_provider = match (configured_url.as_deref(), configured_key.as_deref()) {
            (Some(url), Some(key)) => build_plugin_hub_provider(Some(url), Some(key)),
            _ => build_plugin_hub_provider(None, None),
        };
        let installed_http_provider = new_provider.name() != "mock";

        if !self.publish_plugin_hub_if_current(
            runtime_generation,
            credential_generation,
            reload_generation,
            new_provider,
        ) {
            tracing::debug!(
                "plugin_hub: discarded stale hot-reload candidate after vault generation changed"
            );
            return;
        }

        if installed_http_provider {
            let url = configured_url.expect("HTTP PluginHub candidate has configured URL");
            tracing::info!("plugin_hub: switched to HttpPluginHubProvider @ {url}");
        } else {
            tracing::info!("plugin_hub: using MockPluginHubProvider (no url/license configured)");
        }
    }

    /// Publish a pre-built PluginHub provider while holding the runtime install
    /// guard. The displaced/cancelled blocking client is dropped on a plain OS
    /// thread after the guard is released, so Tokio workers never perform the
    /// last drop of reqwest's private runtime.
    fn publish_plugin_hub_if_current(
        &self,
        runtime_generation: u64,
        credential_generation: u64,
        reload_generation: u64,
        new_provider: Arc<dyn attune_core::plugin_hub::PluginHubProvider>,
    ) -> bool {
        let mut new_provider = Some(new_provider);
        let mut old_provider = None;
        let installed = self.install_credential_runtime_if_current(
            runtime_generation,
            credential_generation,
            &self.plugin_hub_reload_generation,
            reload_generation,
            || {
                old_provider =
                    Some(self.replace_plugin_hub_inner(
                        new_provider.take().expect("new provider present"),
                    ));
            },
        );
        if let Some(unused) = new_provider {
            drop_plugin_hub_provider(unused);
        }
        if let Some(old) = old_provider {
            drop_plugin_hub_provider(old);
        }
        installed
    }

    /// Replace PluginHub without taking `runtime_install_guard`.
    ///
    /// The caller must already hold that guard. This split is also what lets
    /// vault lock clear the provider without recursively locking a non-reentrant
    /// mutex.
    fn replace_plugin_hub_inner(
        &self,
        new_provider: Arc<dyn attune_core::plugin_hub::PluginHubProvider>,
    ) -> Arc<dyn attune_core::plugin_hub::PluginHubProvider> {
        let mut guard = self.plugin_hub.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::replace(&mut *guard, new_provider)
    }

    /// Rebuild PluginHub from the decrypted settings snapshot after unlock.
    /// Keeping this next to the LLM reload path ensures encrypted credentials
    /// are restored into runtime providers without ever returning to plaintext
    /// `app_settings` storage.
    pub fn reload_plugin_hub_from_settings(&self) {
        // Snapshot before reading decrypted settings. If lock happens after the
        // read, the candidate still carries the old generation and is rejected.
        let (runtime_generation, credential_generation, reload_generation) =
            self.begin_credential_reload(&self.plugin_hub_reload_generation);
        let settings = self.read_app_settings_json();
        let member_is_paid = self
            .member_state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_paid();
        let membership_managed = settings
            .as_ref()
            .and_then(|value| value.pointer("/pluginhub/managed_by"))
            .and_then(serde_json::Value::as_str)
            == Some(attune_core::llm_settings::MEMBER_GATEWAY_OWNER);
        // A persisted membership credential is only a cache of an
        // authoritative cloud session.  Never reactivate it on restart before
        // that session and its paid entitlement have been verified.
        if membership_managed && !member_is_paid {
            self.reload_plugin_hub_for_generation(
                runtime_generation,
                credential_generation,
                reload_generation,
                None,
                None,
            );
            return;
        }
        let url = settings
            .as_ref()
            .and_then(|value| value.pointer("/pluginhub/url"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let license_key = settings
            .as_ref()
            .and_then(|value| value.pointer("/pluginhub/license_key"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        self.reload_plugin_hub_for_generation(
            runtime_generation,
            credential_generation,
            reload_generation,
            url.as_deref(),
            license_key.as_deref(),
        );
    }

    /// 读取 vault 中持久化的 app_settings。调用方不能持有 vault lock。
    fn read_app_settings_json(&self) -> Option<serde_json::Value> {
        let vault_guard = self.vault.lock().unwrap_or_else(|e| e.into_inner());
        crate::settings_store::load_settings(&vault_guard)
            .map_err(|e| tracing::warn!("failed to load encrypted application settings: {e}"))
            .ok()
            .flatten()
    }

    /// 仅重建 state.llm + classifier，按当前 settings 重新选 provider。
    /// 用于 wizard / Settings PATCH 修改 llm.* 字段后热切，避免要求重启。
    /// 由 settings.rs 在 body.get("llm").is_some() 时调用。
    pub fn reload_llm(&self) {
        // Snapshot before decrypting settings so a candidate derived from an
        // old unlocked vault can never be published into a later generation.
        let (runtime_generation, credential_generation, reload_generation) =
            self.begin_credential_reload(&self.llm_reload_generation);
        let mut settings_json = self.read_app_settings_json();
        let member_is_paid = self
            .member_state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_paid();
        if !member_is_paid
            && settings_json
                .as_ref()
                .is_some_and(attune_core::llm_settings::membership_gateway_is_managed)
        {
            settings_json = settings_json.map(|settings| {
                attune_core::llm_settings::remove_membership_gateway_from_settings(settings).0
            });
        }
        let llm_result = build_llm_from_settings(&settings_json, &self.hardware);
        let has_provider = llm_result.is_some();
        if !self.publish_llm_if_current(
            runtime_generation,
            credential_generation,
            reload_generation,
            llm_result,
        ) {
            tracing::debug!(
                "LLM hot-reload: discarded stale provider after vault generation changed"
            );
        } else if has_provider {
            tracing::info!("LLM hot-reload: provider rebuilt from settings");
        } else {
            tracing::warn!(
                "LLM hot-reload: settings yielded no LLM provider — chat will be disabled"
            );
        }
    }

    fn publish_llm_if_current(
        &self,
        runtime_generation: u64,
        credential_generation: u64,
        reload_generation: u64,
        llm_result: Option<Arc<dyn LlmProvider>>,
    ) -> bool {
        self.install_credential_runtime_if_current(
            runtime_generation,
            credential_generation,
            &self.llm_reload_generation,
            reload_generation,
            move || match llm_result {
                Some(llm_arc) => {
                    let local_summary_llm = llm_arc.is_local().then(|| llm_arc.clone());
                    let classifier = self
                        .taxonomy
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone()
                        .map(|taxonomy| Arc::new(Classifier::new(taxonomy, llm_arc.clone())));

                    // Publish the main provider and every handle that retains a
                    // clone of it in the same guarded install transaction.
                    *self.classifier.lock().unwrap_or_else(|e| e.into_inner()) = classifier;
                    *self.vlm.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(
                        LlmVlmProvider::new(llm_arc.clone()),
                    )
                        as Arc<dyn VlmProvider>);
                    *self.summary_llm.lock().unwrap_or_else(|e| e.into_inner()) = local_summary_llm;
                    *self.llm.lock().unwrap_or_else(|e| e.into_inner()) = Some(llm_arc);
                }
                None => {
                    // Clear every dependent handle in the same transaction.
                    *self.vlm.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    *self.classifier.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    *self.summary_llm.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    *self.llm.lock().unwrap_or_else(|e| e.into_inner()) = None;
                }
            },
        )
    }

    /// 重建 embedding provider，按当前 settings 热切 local scheduler / cloud
    /// OpenAI-compatible。用于 PATCH /settings 修改 embedding.* 后避免重启。
    pub fn reload_embedding(&self) {
        // As with LLM/PluginHub, settings and indexes belong to this exact
        // unlocked generation even though their construction happens outside
        // the short install critical section.
        let (runtime_generation, reload_generation) =
            self.begin_runtime_reload(&self.embedding_reload_generation);
        let settings_json = self.read_app_settings_json();
        let (provider, is_local) = build_embedding_from_settings(&settings_json);
        let dims = provider.dimensions();
        let scheduler_base = crate::local_scheduler::base_from_optional_settings(&settings_json);
        let reranker = Arc::new(
            attune_core::infer::reranker::LocalSchedulerRerankProvider::new(
                &scheduler_base,
                "kb.query.rerank",
                60_000,
            ),
        ) as Arc<dyn attune_core::infer::RerankProvider>;

        let vector_fingerprint =
            attune_core::embed::current_embedding_fingerprint(provider.as_ref());
        let vectors = if dims == 0 {
            None
        } else {
            match VectorIndex::new(dims) {
                Ok(mut index) => {
                    index.set_embedding_fingerprint(Some(vector_fingerprint.clone()));
                    Some(index)
                }
                Err(error) => {
                    tracing::warn!(
                        "Embedding hot-reload: document vector index reset skipped: {error}"
                    );
                    None
                }
            }
        };

        let memory_index = if dims == 0 {
            None
        } else {
            let built = {
                let vault = self.vault.lock().unwrap_or_else(|e| e.into_inner());
                attune_core::memory::MemoryVectorIndex::build_from_store(vault.store(), dims)
            };
            match built {
                Ok(index) => Some(index),
                Err(error) => {
                    tracing::warn!("Embedding hot-reload: memory index rebuild skipped: {error}");
                    None
                }
            }
        };
        let memory_len = memory_index.as_ref().map(|index| index.len());
        let candidate = EmbeddingRuntimeCandidate {
            provider,
            is_local,
            reranker,
            vectors,
            memory_index,
        };

        if !self.publish_embedding_if_current(runtime_generation, reload_generation, candidate) {
            tracing::debug!(
                "Embedding hot-reload: discarded stale provider after vault generation changed"
            );
            return;
        }

        tracing::info!(
            "Embedding hot-reload: provider rebuilt from settings (dims={dims}, local={is_local})"
        );
        if dims > 0 {
            tracing::info!("Document vector index reset after embedding reload with dims={dims}");
        }
        if let Some(memory_len) = memory_len {
            tracing::info!(
                "Memory vector index rebuilt after embedding reload with dims={dims} ({memory_len} memories)"
            );
        }
    }

    fn publish_embedding_if_current(
        &self,
        runtime_generation: u64,
        reload_generation: u64,
        candidate: EmbeddingRuntimeCandidate,
    ) -> bool {
        self.install_reload_runtime_if_current(
            runtime_generation,
            &self.embedding_reload_generation,
            reload_generation,
            move || {
                self.set_embedding_with_locality(Some(candidate.provider), candidate.is_local);
                self.set_reranker(Some(candidate.reranker));
                *self.vectors.lock().unwrap_or_else(|e| e.into_inner()) = candidate.vectors;
                *self.memory_index.lock().unwrap_or_else(|e| e.into_inner()) =
                    candidate.memory_index;
                self.invalidate_search_cache();
            },
        )
    }

    /// 初始化搜索引擎 + 分类引擎 (unlock 后调用)
    /// 使用 compare_exchange 保证幂等：并发 unlock 只有第一个线程真正执行初始化。
    pub fn init_search_engines(&self) {
        if self
            .engines_initialized
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return; // 已初始化，跳过
        }
        let runtime_generation = self.runtime_generation();
        if !self.install_runtime_if_current(runtime_generation, || {}) {
            self.engines_initialized.store(false, Ordering::SeqCst);
            return;
        }

        // v0.6.0-rc.4: 按 region 自动设 HF_ENDPOINT，让 ONNX 模型从国内镜像下载
        // hf-hub crate 读 HF_ENDPOINT 环境变量；未设走默认 huggingface.co
        if std::env::var_os("HF_ENDPOINT").is_none() {
            let region = attune_core::platform::detect_region();
            let endpoint = region.hf_endpoint();
            // SAFETY: 启动时一次性设置（init_search_engines 由 compare_exchange 保证幂等）
            // 不会有并发 set_var 竞争。
            #[allow(unsafe_code)]
            unsafe {
                std::env::set_var("HF_ENDPOINT", endpoint)
            };
            tracing::info!(
                "Region detected: {} → HF_ENDPOINT={endpoint}",
                region.label()
            );
        }

        // S8 cache: seed the in-memory model-source resolution cache from the persisted
        // selected source so the first post-unlock download skips re-probing all sources
        // (the read_selected_source/freshness layer was dead code before this wiring).
        // vault guard taken alone (no vectors/fulltext held) — respects lock ordering.
        {
            let settings = {
                let vault_guard = self.vault.lock().unwrap_or_else(|e| e.into_inner());
                vault_guard
                    .store()
                    .get_meta("app_settings")
                    .ok()
                    .flatten()
                    .and_then(|d| serde_json::from_slice::<serde_json::Value>(&d).ok())
            };
            if let Some(s) = settings {
                let region = attune_core::platform::detect_region();
                let n = attune_core::infer::model_source::seed_resolution_cache_from_settings(
                    &s, region,
                );
                if n > 0 {
                    tracing::info!(
                        "model-source cache seeded from persisted selection ({n} buckets)"
                    );
                }
            }
        }

        let settings_json = {
            let vault_guard = self.vault.lock().unwrap_or_else(|e| e.into_inner());
            vault_guard
                .store()
                .get_meta("app_settings")
                .ok()
                .flatten()
                .and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok())
        };
        let vector_dims = embedding_index_dims_from_settings(&settings_json);
        let current_vector_fingerprint = self
            .embedding()
            .map(|embedding| attune_core::embed::current_embedding_fingerprint(embedding.as_ref()));

        // Vector index (dims follow the configured embedding provider; bge-m3=1024,
        // local scheduler embedding-int8=512).
        //
        // Load this before the fulltext rebuild so post-unlock vector and
        // scheduler-native retrieval can serve immediately while Tantivy refreshes
        // in the background bootstrap task.
        //
        // 持久化策略：
        //   优先从 ~/.local/share/attune/vectors.encbin 加密加载；不存在或损坏
        //   降级为空 HNSW。写入在 start_queue_worker 批次结束时 flush（每 20 次 or
        //   每 10 分钟取近者），clear_search_engines 锁定前再 flush 一次。
        // 全局规范锁序（任意路径同时持多锁时必须遵守）：
        //   fulltext → vectors → vault  （embedding / search_cache / cluster_snapshot
        //   各为独立锁，不参与该序）。与 search/chat 热点路径一致；反序持锁 = ABBA 死锁。
        // 此处不同时持锁：先取 vault 拿 dek（语句结束即释放），再单独取 vectors 装载，
        // 两锁不重叠，故不违反规范序。
        let vectors_path = attune_core::platform::data_dir().join("vectors.encbin");
        let dek_opt = self
            .vault
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .dek_db()
            .ok();
        let mut vectors = match dek_opt {
            Some(dek) if vectors_path.exists() => {
                match VectorIndex::load_encrypted(&dek, &vectors_path, vector_dims) {
                    Ok(vi) => {
                        tracing::info!(
                            "Vector index loaded from {} (dims={}, {} entries)",
                            vectors_path.display(),
                            vector_dims,
                            vi.len()
                        );
                        Some(vi)
                    }
                    Err(e) => {
                        tracing::warn!("Vector index load failed ({e}); starting empty");
                        VectorIndex::new(vector_dims).ok()
                    }
                }
            }
            _ => VectorIndex::new(vector_dims).ok(),
        };
        if let (Some(index), Some(fingerprint)) =
            (vectors.as_mut(), current_vector_fingerprint.as_ref())
        {
            if index.embedding_fingerprint().is_none() && index.is_empty() {
                index.set_embedding_fingerprint(Some(fingerprint.clone()));
            }
        }
        if !self.install_runtime_if_current(runtime_generation, || {
            if let Ok(mut guard) = self.vectors.lock() {
                *guard = vectors;
            }
        }) {
            return;
        }

        // Fulltext index (persistent on disk)
        //
        // #83 P0 可用性修复：FTS rebuild 用 paged 查询，每页单独加释放 vault lock，
        // 避免旧逻辑全量持锁 30-60s 阻塞并发请求。
        // init_search_engines 在 axum spawn_blocking 路径调用，同步 paged 循环安全。
        {
            let tantivy_dir = attune_core::platform::data_dir().join("tantivy");
            if let Ok(ft) = FulltextIndex::open(&tantivy_dir) {
                // Set the index immediately so vault is usable while rebuild progresses.
                if !self.install_runtime_if_current(runtime_generation, || {
                    *self.fulltext.lock().unwrap_or_else(|e| e.into_inner()) = Some(ft);
                }) {
                    return;
                }
                // Now rebuild page-by-page — hold vault lock only per page, release between pages.
                const PAGE: usize = 500;
                let mut offset = 0usize;
                loop {
                    let page_items: Vec<(String, String, String, String)> = {
                        let vault_guard = self.vault.lock().unwrap_or_else(|e| e.into_inner());
                        let dek = match vault_guard.dek_db() {
                            Ok(d) => d,
                            Err(_) => break,
                        };
                        let ids = match vault_guard.store().list_item_ids_paged(offset, PAGE) {
                            Ok(ids) => ids,
                            Err(e) => {
                                tracing::warn!("#83 FTS rebuild paged query: {e}");
                                break;
                            }
                        };
                        if ids.is_empty() {
                            break;
                        }
                        let mut out = Vec::with_capacity(ids.len());
                        for id in &ids {
                            if let Ok(Some(item)) = vault_guard.store().get_item(&dek, id) {
                                out.push((item.id, item.title, item.content, item.source_type));
                            }
                        }
                        out
                    }; // vault lock released here
                    let n = page_items.len();
                    if n == 0 {
                        break;
                    }
                    if let Ok(ft_guard) = self.fulltext.lock() {
                        if let Some(ft) = ft_guard.as_ref() {
                            if let Err(e) = ft.add_documents(&page_items) {
                                tracing::warn!("#83 FTS rebuild batch add failed: {e}");
                            }
                        }
                    }
                    offset += n;
                    if n < PAGE {
                        break;
                    }
                }
                tracing::info!("#83 FTS rebuild complete ({offset} items, paged={PAGE})");
            }
        }

        // #2 #5: Embedding / Reranker（~330MB ONNX 下载）+ OCR + ASR 的获取**不再**在此
        // 同步阻塞——它们曾让 vault 解锁卡在网络下载上（解锁慢 + 失败即四类底座全不可用）。
        // 现在解锁只做"本地、零下载"的部分（上面的 fulltext / vector / memory 索引装载 +
        // 下面的 LLM 配置读取），底座模型由 `spawn_model_bootstrap` 在后台线程拉取（经
        // company-mirror → CN mirror → HF failover + 重试），进度落 `model_bootstrap`，
        // 由 /ai_stack 暴露。embedding 就绪后后台再用其 dims 重建 memory_index。
        //
        // 注：memory_index 在上面以 settings 推断 dims 先建一份兜底，让 tiered
        // assembler 在 scheduler embedding 可用前不至于完全停摆；bg 任务拿到真 dims 后会重建。
        {
            let memory_dims = embedding_index_dims_from_settings(&settings_json);
            let built = {
                let vault = self.vault.lock().unwrap_or_else(|e| e.into_inner());
                attune_core::memory::MemoryVectorIndex::build_from_store(vault.store(), memory_dims)
            };
            if let Ok(idx) = built {
                if !self.install_runtime_if_current(runtime_generation, || {
                    if let Ok(mut g) = self.memory_index.lock() {
                        *g = Some(idx);
                    }
                }) {
                    return;
                }
            }
        }

        // LLM/VLM/summary ownership belongs exclusively to `reload_llm`, which
        // reads the decrypted settings view. Rebuilding them here from the raw
        // `app_settings` row used to replace an encrypted BYOK provider with a
        // second provider whose API key was empty.
        //
        // Taxonomy is search-engine state and is initialized independently of
        // whether an LLM was configured at unlock. Inside the runtime install
        // guard, bind the classifier to the *current* main LLM. This serializes
        // correctly with same-generation member/session hot reloads: whichever
        // transaction runs last observes or republishes a coherent pair.
        let mut tax = Taxonomy::default();
        if let Ok(plugins) = Taxonomy::load_builtin_plugins() {
            for plugin in plugins {
                tax = tax.with_plugin(plugin);
            }
        }
        let (user_plugins, _errors) =
            Taxonomy::load_user_plugins(&attune_core::platform::config_dir());
        for plugin in user_plugins {
            tax = tax.with_plugin(plugin);
        }
        let tax_arc = Arc::new(tax);
        if !self.install_runtime_if_current(runtime_generation, || {
            let llm = self.llm.lock().unwrap_or_else(|e| e.into_inner()).clone();
            *self.classifier.lock().unwrap_or_else(|e| e.into_inner()) =
                llm.map(|llm| Arc::new(Classifier::new(tax_arc.clone(), llm)));
            *self.taxonomy.lock().unwrap_or_else(|e| e.into_inner()) = Some(tax_arc);
        }) {
            return;
        }

        // Web search provider（从 app_settings.web_search 加载；缺省时尝试默认）
        {
            let settings_json = {
                let vault_guard = self.vault.lock().unwrap_or_else(|e| e.into_inner());
                vault_guard
                    .store()
                    .get_meta("app_settings")
                    .ok()
                    .flatten()
                    .and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok())
                    .unwrap_or_else(|| serde_json::json!({}))
            };
            let ws_provider = attune_core::web_search::from_settings(&settings_json);
            match ws_provider {
                Some(ws) => {
                    tracing::info!("Web search: {} provider enabled", ws.provider_name());
                    if !self.install_runtime_if_current(runtime_generation, || {
                        *self.web_search.lock().unwrap_or_else(|e| e.into_inner()) = Some(ws);
                    }) {
                        return;
                    }
                }
                None => {
                    // 诊断：区分 disabled vs 无浏览器 vs 无效路径
                    let disabled = settings_json
                        .get("web_search")
                        .and_then(|w| w.get("enabled"))
                        .and_then(|v| v.as_bool())
                        == Some(false);
                    if disabled {
                        tracing::info!("Web search: disabled via settings");
                    } else {
                        let detected = attune_core::web_search_browser::detect_system_browser();
                        match detected {
                            Some(p) => tracing::warn!(
                                "Web search: 系统检测到浏览器 {} 但 provider 构造失败",
                                p.display()
                            ),
                            None => tracing::warn!(
                                "Web search: 未检测到 Chrome/Edge，浏览器搜索 fallback 不可用。\
                                 安装 google-chrome 后重启 server 即可启用。"
                            ),
                        }
                    }
                }
            }
        }

        // TagIndex (built from existing items.tags)
        let tag_index_result = {
            let vault_guard = self.vault.lock().unwrap_or_else(|e| e.into_inner());
            if let Ok(dek) = vault_guard.dek_db() {
                TagIndex::build(vault_guard.store(), &dek).ok()
            } else {
                None
            }
        };
        let _ = self.install_runtime_if_current(runtime_generation, || {
            *self.tag_index.lock().unwrap_or_else(|e| e.into_inner()) = tag_index_result;
        });
    }

    /// 手动处理一批 classify 任务（供 /classify/drain 端点调用）
    ///
    /// 从 embed_queue 中只取一批 pending classify 任务，经过本地/云端隐私边界后
    /// 调用 classifier.classify_batch，写回 items.tags 和 TagIndex，最后标记 done。
    pub fn drain_classify_batch(&self, batch_size: usize) -> attune_core::error::Result<usize> {
        // 1. 检查 classifier 是否可用
        let classifier = match self
            .classifier
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .cloned()
        {
            Some(c) => c,
            None => return Ok(0),
        };

        // A cloud classifier's availability and dequeue must both sit behind the
        // explicit privacy.llm bit. Returning before dequeue leaves rows pending.
        let classifier_is_local = classifier.is_local();
        if !classifier_is_local && !llm_privacy_enabled(&self.read_app_settings_json()) {
            return Ok(0);
        }

        // 2. Dequeue only classification tasks. The old use of
        // `dequeue_embeddings` could never return these rows after that API was
        // hardened to task_type='embed', leaving classification permanently idle.
        let (classify_tasks, dek) = {
            let vault = self.vault.lock().unwrap_or_else(|e| e.into_inner());
            let dek = vault.dek_db()?;
            let tasks = vault.store().dequeue_classify_tasks(batch_size)?;
            (tasks, dek)
        };

        if classify_tasks.is_empty() {
            return Ok(0);
        }

        // 3. Load content + privacy tier. Tier lookup errors fail closed for that
        // row (return to pending); deleted/missing rows are terminally completed.
        let items_info: Vec<(String, String, String, i64, bool)> = {
            let vault = self.vault.lock().unwrap_or_else(|e| e.into_inner());
            let mut items = Vec::with_capacity(classify_tasks.len());
            for task in &classify_tasks {
                let contains_l0 = match vault.store().get_item_privacy_tier(&task.item_id) {
                    Ok(tier) => matches!(tier, attune_core::store::audit::PrivacyTier::L0),
                    Err(attune_core::error::VaultError::NotFound(_)) => {
                        let _ = vault.store().mark_embedding_done(task.id);
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "classifier privacy-tier lookup failed for {}: {e}",
                            task.item_id
                        );
                        let _ = vault.store().mark_task_pending(task.id);
                        continue;
                    }
                };
                match vault.store().get_item(&dek, &task.item_id) {
                    Ok(Some(item)) => items.push((
                        task.item_id.clone(),
                        item.title,
                        item.content,
                        task.id,
                        contains_l0,
                    )),
                    Ok(None) | Err(attune_core::error::VaultError::NotFound(_)) => {
                        let _ = vault.store().mark_embedding_done(task.id);
                    }
                    Err(e) => {
                        tracing::warn!("classifier item load failed for {}: {e}", task.item_id);
                        let _ = vault.store().mark_task_pending(task.id);
                    }
                }
            }
            items
        };

        if items_info.is_empty() {
            return Ok(0);
        }

        // 4. Build the exact LLM wire inputs. L0 cloud rows terminate without
        // egress; other cloud rows are redacted. Local classification is unchanged.
        let cloud_enabled_now =
            classifier_is_local || llm_privacy_enabled(&self.read_app_settings_json());
        let vault_unlocked = matches!(
            self.vault.lock().unwrap_or_else(|e| e.into_inner()).state(),
            attune_core::vault::VaultState::Unlocked
        );
        let redactor = Redactor::new();
        let mut governed_items = Vec::with_capacity(items_info.len());
        for (item_id, title, content, task_id, contains_l0) in items_info {
            match govern_classification_input(
                classifier_is_local,
                cloud_enabled_now,
                vault_unlocked,
                contains_l0,
                &redactor,
                &title,
                &content,
            ) {
                Ok((title, content)) => {
                    governed_items.push((item_id, title, content, task_id));
                }
                Err(attune_core::outbound_gate::OutboundError::L0CloudBlocked) => {
                    tracing::info!(
                        "classifier skipped L0 item {item_id}: cloud classification is forbidden"
                    );
                    let vault = self.vault.lock().unwrap_or_else(|e| e.into_inner());
                    let _ = vault.store().mark_embedding_done(task_id);
                }
                Err(e) => {
                    tracing::warn!("classifier outbound gate refused {item_id}: {e}");
                    let vault = self.vault.lock().unwrap_or_else(|e| e.into_inner());
                    let _ = vault.store().mark_task_pending(task_id);
                }
            }
        }
        if governed_items.is_empty() {
            return Ok(0);
        }
        let classifier_inputs: Vec<(String, String)> = governed_items
            .iter()
            .map(|(_, title, content, _)| (title.clone(), content.clone()))
            .collect();

        let results = match classifier.classify_batch(&classifier_inputs) {
            Ok(r) => r,
            Err(e) => {
                // 失败时标记所有任务为 failed（会根据 attempts 决定重试或 abandon）
                let vault = self.vault.lock().unwrap_or_else(|e| e.into_inner());
                for (_, _, _, task_id) in &governed_items {
                    let _ = vault.store().mark_embedding_failed(*task_id, 3);
                }
                return Err(e);
            }
        };

        // 5. 写回 tags + TagIndex + 标记完成
        let mut processed = 0;
        for (i, (item_id, _, _, task_id)) in governed_items.iter().enumerate() {
            let Some(result) = results.get(i) else {
                let vault = self.vault.lock().unwrap_or_else(|e| e.into_inner());
                let _ = vault.store().mark_embedding_failed(*task_id, 3);
                continue;
            };
            let tags_json = serde_json::to_string(result)?;

            {
                let vault = self.vault.lock().unwrap_or_else(|e| e.into_inner());
                vault.store().update_tags(&dek, item_id, &tags_json)?;
                vault.store().mark_embedding_done(*task_id)?;
            }

            if let Some(index) = self
                .tag_index
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_mut()
            {
                index.upsert(item_id, result);
            }
            processed += 1;
        }

        Ok(processed)
    }

    /// Install scheduler-backed runtime handles after unlock.
    ///
    /// Local model lifecycle is scheduler-owned. Attune server does not download
    /// or load local worker assets here; it installs scheduler-backed
    /// embedding/rerank providers and leaves OCR/ASR marked externally managed.
    pub fn spawn_model_bootstrap(state: std::sync::Arc<AppState>) {
        // 防重入：解锁可被多次调用（restart 后再 unlock）。模型状态 ready
        // 只说明 scheduler 生命周期已托管；vault lock 会清掉 in-memory provider
        // handles，所以 provider 缺失时仍必须重装 handles。
        let scheduler_handles_present = state.embedding().is_some()
            && state
                .reranker
                .lock()
                .ok()
                .map(|g| g.is_some())
                .unwrap_or(false);
        if state.model_bootstrap.all_ready() && scheduler_handles_present {
            return;
        }
        let runtime_generation = state.runtime_generation();
        std::thread::spawn(move || {
            let status = &state.model_bootstrap;

            // 1) Embedding: no direct local worker bootstrap in attune-server.
            // Local inference is scheduler-native; cloud endpoints remain explicitly
            // configured and privacy-gated by `embedding_is_local=false`.
            let embed_settings_json = { state.read_app_settings_json() };
            let (provider, is_local) = build_embedding_from_settings(&embed_settings_json);
            if !state.install_runtime_if_current(runtime_generation, || {
                state.set_embedding_with_locality(Some(provider), is_local);
            }) {
                tracing::debug!(
                    "model bootstrap cancelled because the vault runtime generation changed"
                );
                return;
            }
            state.model_bootstrap.mark_ready("embedding");

            // embedding 就绪后用真 dims 重建 memory_index（解锁时用 1024 兜底建过一份）。
            if let Some(dims) = state.embedding().map(|p| p.dimensions()).filter(|d| *d > 0) {
                let built = {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    attune_core::memory::MemoryVectorIndex::build_from_store(vault.store(), dims)
                };
                if let Ok(idx) = built {
                    tracing::info!(
                        "Memory vector index rebuilt with dims={dims} ({} memories)",
                        idx.len()
                    );
                    if !state.install_runtime_if_current(runtime_generation, || {
                        if let Ok(mut g) = state.memory_index.lock() {
                            *g = Some(idx);
                        }
                    }) {
                        return;
                    }
                }
            }

            // 2) Reranker: scheduler-native task. The search layer already degrades
            // gracefully to RRF/cosine if the scheduler task is unavailable.
            let scheduler_base = crate::local_scheduler::base_from_optional_settings(
                &state.read_app_settings_json(),
            );
            if !state.install_runtime_if_current(runtime_generation, || {
                state.set_reranker(Some(Arc::new(
                    attune_core::infer::reranker::LocalSchedulerRerankProvider::new(
                        &scheduler_base,
                        "kb.query.rerank",
                        60_000,
                    ),
                )));
            }) {
                return;
            }
            state.model_bootstrap.mark_ready("reranker");

            // 3) OCR / 4) ASR: local model lifecycle is scheduler-owned. Do not
            // download OCR/ASR worker assets from attune-server bootstrap.
            status.mark_ready("ocr");
            status.mark_ready("asr");

            tracing::info!(
                "model bootstrap finished (all_ready={})",
                state.model_bootstrap.all_ready()
            );
        });
    }

    /// 按需安装本机推荐 EP 链所需的运行时软件栈（userspace runtime，**非驱动**）。
    ///
    /// 流程:`accel::cached_selection().recommend_ep_chain()` → 取每个 EP 的
    /// `runtime_stack()`（去重）→ `stack_installer::spawn_stack_bootstrap` 后台拉取缺失栈
    /// （已就位则标 Present 跳过）。与 `spawn_model_bootstrap` 平行、非阻塞、可重试。
    ///
    /// 栈装不上 → 对应 EP 运行时注册失败 → ORT 静默降级 CPU（provider.rs 不用
    /// error_on_failure 已兜住）。内核驱动不在此装（#6 consent-gated）。
    pub fn spawn_stack_bootstrap(state: std::sync::Arc<AppState>) {
        let spawn_result = std::thread::Builder::new()
            .name("attune-ep-stack-bootstrap".to_string())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let sel = attune_core::infer::accel::cached_selection();
                    let chain = sel.recommend_ep_chain();
                    // 去重收集链上需要 userspace 栈的标识（CPU/CoreML 返回 None，自动排除）。
                    let mut wanted: Vec<String> = Vec::new();
                    for ep in &chain {
                        if let Some(stack) = ep.runtime_stack() {
                            if !wanted.iter().any(|s| s == stack) {
                                wanted.push(stack.to_string());
                            }
                        }
                    }
                    if wanted.is_empty() {
                        // 纯 CPU 链（默认 build / 无加速硬件）：无栈可装，直接返回。
                        return;
                    }
                    tracing::info!("EP stack bootstrap: ensuring runtime stacks {wanted:?} (userspace only, drivers excluded)");
                    attune_core::infer::stack_installer::spawn_stack_bootstrap(state.stack_install.clone(), wanted);
                }));
                if result.is_err() {
                    tracing::error!("EP stack bootstrap panicked");
                }
            });

        if let Err(err) = spawn_result {
            tracing::warn!("failed to spawn EP stack bootstrap thread: {err}");
        }
    }

    /// 启动后台分类 worker（需要在 init_search_engines 之后调用）
    /// 使用 AtomicBool 防止重复启动；vault lock 时自动退出并重置标志。
    pub fn start_classify_worker(state: std::sync::Arc<AppState>) {
        let settings_json = state.read_app_settings_json();
        if !classify_worker_auto_enabled(&settings_json) {
            tracing::info!(
                "Classify worker skipped: automatic classification is disabled for the current LLM settings"
            );
            return;
        }

        if state
            .classifier
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_none()
        {
            return; // No classifier, no worker
        }

        if state
            .classify_worker_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::debug!("Classify worker already running, skipping");
            return;
        }

        // H1：classify worker 走 LLM 分类，复用 AiAnnotator 档位（无 LLM 速率限制，
        // 但 CPU/RAM 受治理；如未来要为分类单独建档可加 TaskKind::Classify）。
        let governor = global_registry().register(TaskKind::AiAnnotator);

        std::thread::spawn(move || {
            tracing::info!("Classify worker started");
            loop {
                // Check if vault is still unlocked
                {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    if !matches!(vault.state(), attune_core::vault::VaultState::Unlocked) {
                        break;
                    }
                }

                if !governor.should_run() {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    continue;
                }

                match state.drain_classify_batch(5) {
                    Ok(0) => std::thread::sleep(std::time::Duration::from_secs(5)),
                    Ok(n) => {
                        tracing::info!("Classified {} items", n);
                        std::thread::sleep(governor.after_work());
                    }
                    Err(e) => {
                        tracing::warn!("Classify worker error: {}", e);
                        std::thread::sleep(std::time::Duration::from_secs(10));
                    }
                }
            }
            state.classify_worker_running.store(false, Ordering::SeqCst);
            tracing::info!("Classify worker stopped (vault locked)");
        });
    }

    /// v0.7 记忆护城河：启动后台 reindex worker。
    ///
    /// 消费 [`reindex_queue`] 表 — scanner / scanner_webdav 在 attune-core 层
    /// 调 `store.delete_item` 后，无法直接持有 VectorIndex + FulltextIndex 锁
    /// 清向量与 FTS，于是写信号到此表，由本 worker 周期消费 → 调用
    /// `attune_core::reindex::purge_item_indexes`。
    ///
    /// 轮询周期：3 秒（不繁忙时几乎没开销，繁忙时及时清理 orphan）。
    /// vault lock / 引擎未初始化时静默退出并重置 atomic flag。
    pub fn start_reindex_worker(state: std::sync::Arc<AppState>) {
        if state
            .reindex_worker_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::debug!("Reindex worker already running, skipping");
            return;
        }

        std::thread::spawn(move || {
            // RAII guard 保证任何退出路径（含 reindex_item / usearch FFI
            // panic）都复位 reindex_worker_running flag。否则 worker thread panic 后
            // flag 永久 true → start_reindex_worker 的 compare_exchange 永远失败 →
            // worker 无法重启 → reindex 全停 → search 永久返回 stale 内容。
            struct WorkerFlagGuard<'a>(&'a std::sync::atomic::AtomicBool);
            impl Drop for WorkerFlagGuard<'_> {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::SeqCst);
                }
            }
            let _flag_guard = WorkerFlagGuard(&state.reindex_worker_running);

            tracing::info!("Reindex worker started");
            loop {
                // vault lock check
                {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    if !matches!(vault.state(), attune_core::vault::VaultState::Unlocked) {
                        break;
                    }
                }

                let tasks: Vec<(i64, String, String, i64)> = {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    vault.store().dequeue_reindex_tasks(10).unwrap_or_default()
                };

                if tasks.is_empty() {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    continue;
                }

                // 顺序处理（持锁短，按 task 释放，避免长占 vectors lock 影响 search）
                for (task_id, item_id, action, _prior_attempts) in tasks {
                    // 区分 Transient vs Task error。
                    // Transient（引擎未就绪 / dek 解密失败 / vault 锁定）= 时序 race，
                    //   不计 attempts 只 sleep；下次 unlock+ready 后正常处理。
                    // Task（item not found / unknown action）= 任务本身有问题，
                    //   bump attempts 让毒任务在 5 次后被 park。
                    //
                    // 之前所有错误统一 bump → 引擎未 ready 的 5 分钟 race 期内，正常任务会被
                    // 错误地 park（attempts ≥ 5），需运维手动 reset 才能恢复。
                    enum WorkerErr {
                        Transient(String),
                        Task(String),
                    }
                    let result: Result<(), WorkerErr> = (|| {
                        // Lock order MUST be fulltext → vectors → vault (canonical, matches
                        // the search/chat hot path). Acquiring vault first here would invert
                        // the order vs search/chat and deadlock (ABBA). See lock_order_abba_test.
                        let fulltext_g = state.fulltext.lock().unwrap_or_else(|e| e.into_inner());
                        let mut vectors_g = state.vectors.lock().unwrap_or_else(|e| e.into_inner());
                        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                        let dek = vault
                            .dek_db()
                            .map_err(|e| WorkerErr::Transient(format!("dek_db: {e}")))?;
                        let (Some(vectors), Some(fulltext)) =
                            (vectors_g.as_mut(), fulltext_g.as_ref())
                        else {
                            return Err(WorkerErr::Transient(
                                "vectors/fulltext not initialized".into(),
                            ));
                        };
                        match action.as_str() {
                            "purge" => attune_core::reindex::purge_item_indexes(
                                vault.store(),
                                vectors,
                                fulltext,
                                &item_id,
                            )
                            .map(|_| ())
                            .map_err(|e| WorkerErr::Task(e.to_string())),
                            // 'reindex' action 实现
                            "reindex" => {
                                let item = vault
                                    .store()
                                    .get_item(&dek, &item_id)
                                    .map_err(|e| WorkerErr::Task(e.to_string()))?
                                    .ok_or_else(|| {
                                        WorkerErr::Task(format!(
                                            "item {item_id} not found for reindex"
                                        ))
                                    })?;
                                attune_core::reindex::reindex_item(
                                    vault.store(),
                                    vectors,
                                    fulltext,
                                    &item_id,
                                    &item.title,
                                    &item.content,
                                    &item.source_type,
                                )
                                .map(|_| ())
                                .map_err(|e| WorkerErr::Task(e.to_string()))
                            }
                            other => {
                                Err(WorkerErr::Task(format!("unknown reindex action: {other}")))
                            }
                        }
                    })();

                    match result {
                        Ok(_) => {
                            {
                                let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                                let _ = vault.store().mark_reindex_done(task_id);
                            }
                            // reindex worker 改了向量/FTS → 失效 search 缓存
                            state.invalidate_search_cache();
                            tracing::info!("reindex_queue: {action} done for item={item_id}");
                        }
                        Err(WorkerErr::Transient(e)) => {
                            // 不 bump attempts；等下轮引擎/dek/vault 就绪
                            tracing::debug!(
                                "reindex_queue: task {task_id} ({action} {item_id}) transient: {e}, will retry"
                            );
                            std::thread::sleep(std::time::Duration::from_secs(2));
                        }
                        Err(WorkerErr::Task(e)) => {
                            // bump attempts → 达 5 次后 dequeue WHERE 自动 skip。
                            let new_attempts = {
                                let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                                // bump 失败（schema drift / WAL 故障）不应静默
                                // 当成"到 5 次"，否则无法区分"真毒任务"与"DB 写挂了"。
                                match vault.store().bump_reindex_attempts(task_id) {
                                    Ok(n) => n,
                                    Err(e) => {
                                        tracing::warn!(
                                            "reindex_queue: bump_reindex_attempts DB write failed for task {task_id}: {e} — forcing park"
                                        );
                                        5
                                    }
                                }
                            };
                            if new_attempts >= 5 {
                                tracing::error!(
                                    "reindex_queue: task {task_id} ({action} {item_id}) reached {new_attempts} attempts, parking — {e}"
                                );
                            } else {
                                tracing::warn!(
                                    "reindex_queue: task {task_id} ({action} {item_id}) failed (attempt {new_attempts}): {e}"
                                );
                            }
                            std::thread::sleep(std::time::Duration::from_secs(2));
                        }
                    }
                }
            }
            // flag 复位由 WorkerFlagGuard::drop 接管（含 panic 路径）
            tracing::info!("Reindex worker stopped (vault locked)");
        });
    }

    /// G3① locked-mode staging drain. Started on unlock; drains the encrypted staging
    /// area (inbound documents accepted while LOCKED) through the normal ingest pipeline,
    /// then exits. Idempotent: a staged file present == pending; on success the file is
    /// removed (the commit point), so a mid-drain crash leaves remaining files for the
    /// next unlock and never double-ingests (the pipeline's content_hash short-circuit
    /// covers anything ingested-but-not-yet-removed).
    ///
    /// Lock ordering: only takes the vault lock in a SHORT critical section per item
    /// (dek clone + ingest_document), never nesting vectors/fulltext, so it cannot ABBA
    /// with the search/chat hot path. Embedding enqueue is done inside ingest_document
    /// against the store; the reindex/embed workers (already running post-unlock) pick up.
    pub fn start_staging_drain_worker(state: std::sync::Arc<AppState>) {
        if state
            .staging_drain_worker_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::debug!("Staging drain worker already running, skipping");
            return;
        }

        std::thread::spawn(move || {
            struct FlagGuard<'a>(&'a std::sync::atomic::AtomicBool);
            impl Drop for FlagGuard<'_> {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::SeqCst);
                }
            }
            let _guard = FlagGuard(&state.staging_drain_worker_running);

            let staging = attune_core::staging::IngestStaging::open_default();
            let pending = staging.list_pending();
            if pending.is_empty() {
                return;
            }
            tracing::info!(
                "Staging drain: {} pending locked-mode ingests",
                pending.len()
            );

            let mut drained = 0usize;
            for id in pending {
                // Stop early if the vault got re-locked mid-drain (degrade gracefully).
                {
                    let v = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    if !matches!(v.state(), attune_core::vault::VaultState::Unlocked) {
                        tracing::info!(
                            "Staging drain: vault re-locked, stopping (will resume on next unlock)"
                        );
                        break;
                    }
                }

                let item = match staging.load(&id) {
                    Ok(it) => it,
                    Err(e) => {
                        // Corrupt blob/meta: skip + RETAIN for manual inspection. Never
                        // delete (would silently lose data); never crash the loop.
                        tracing::warn!("Staging drain: skip corrupt item {id}: {e}");
                        continue;
                    }
                };

                let raw = attune_core::ingest::RawDocument {
                    uri: item.meta.uri.clone(),
                    title: item.meta.title.clone(),
                    content: item.content,
                    mime_hint: item.meta.mime_hint.clone(),
                    source_kind: attune_core::ingest::SourceKind::LocalFolder,
                    source_ref: item.meta.uri.clone(),
                    modified_marker: None,
                    domain: item.meta.domain.clone(),
                    tags: item.meta.tags.clone(),
                    corpus_domain: item.meta.corpus_domain.clone(),
                    metadata: std::collections::HashMap::new(),
                };

                let ingest_options =
                    crate::local_scheduler::ingest_options_from_state(&state, None);
                let ingest_result = {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    match vault.dek_db() {
                        Ok(dek) => attune_core::ingest::ingest_document_with_options(
                            vault.store(),
                            &dek,
                            &raw,
                            &ingest_options,
                        ),
                        Err(e) => Err(e),
                    }
                };

                match ingest_result {
                    Ok(_) => {
                        // Commit point: remove staging files (idempotent done marker).
                        let _ = staging.remove(&id);
                        drained += 1;
                        state.invalidate_search_cache();
                    }
                    Err(e) => {
                        // Retain for retry on next unlock; content_hash short-circuit
                        // prevents duplicate insert if it partially succeeded.
                        tracing::warn!(
                            "Staging drain: ingest failed for {id}: {e}, retaining for retry"
                        );
                    }
                }
            }
            tracing::info!("Staging drain: drained {drained} item(s)");
        });
    }

    /// Trust-chain T8: 启动 entitlement 周期 re-verify worker。
    ///
    /// 每轮(默认 24h,按 cloud `next_verify_after_hours` 可调)对 EntitlementCache 中
    /// 每条 entitlement 跑真 verify;响应**经 SEC-1/2 门(`authorize_snapshot`)后**才
    /// 转 Active(伪造/重放/未签名 strict → 不转 Active,走宽限)。失败连续退避
    /// 1h → 4h → 24h([`attune_core::entitlement_reverify::backoff_after`]),成功重置。
    ///
    /// 锁序铁律:复用 `routes::member::run_refresh_round` —— entitlement 缓存锁独立,
    /// 写回 vault 时短取 vault 锁,**绝不**在持 entitlement 锁时取 fulltext/vectors/vault。
    /// 原子 flag 防重入 + RAII guard 复位;vault lock → 静默退出。
    pub fn start_entitlement_worker(state: std::sync::Arc<AppState>) {
        if state
            .entitlement_worker_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::debug!("Entitlement worker already running, skipping");
            return;
        }

        std::thread::spawn(move || {
            struct WorkerFlagGuard<'a>(&'a std::sync::atomic::AtomicBool);
            impl Drop for WorkerFlagGuard<'_> {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::SeqCst);
                }
            }
            let _flag_guard = WorkerFlagGuard(&state.entitlement_worker_running);

            tracing::info!("Entitlement re-verify worker started");
            // 退避状态(worker 内存,失败计数);恢复成功重置。
            let mut consecutive_failures: u32 = 0;
            // 周期默认 24h(spec §5.2 next_verify_after_hours 默认)。
            const BASE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);
            // 轮询粒度:每 60s 醒来检查 vault 状态 + 是否到下一轮(避免长 sleep 阻 vault-lock 退出)。
            const TICK: std::time::Duration = std::time::Duration::from_secs(60);
            let mut next_run = std::time::Instant::now();

            loop {
                // vault lock check —— 锁定即退出(下次 unlock 重启 worker)。
                {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    if !matches!(vault.state(), attune_core::vault::VaultState::Unlocked) {
                        break;
                    }
                }

                if std::time::Instant::now() < next_run {
                    std::thread::sleep(TICK);
                    continue;
                }

                // 一轮 re-verify(blocking 网络 + 短取 vault 锁写回,均在本 worker 线程)。
                let summary =
                    crate::routes::member::run_refresh_round(&state, &state.entitlement_cache);

                // 退避:本轮"全网络错"(cloud 不可达)→ 失败 +1 并按退避延后;否则重置。
                let interval = if summary.all_network_error {
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    let backoff =
                        attune_core::entitlement_reverify::backoff_after(consecutive_failures);
                    std::time::Duration::from_secs(backoff.num_seconds().max(0) as u64)
                } else {
                    consecutive_failures = 0;
                    BASE_INTERVAL
                };
                next_run = std::time::Instant::now() + interval;
            }
            tracing::info!("Entitlement re-verify worker stopped (vault locked)");
        });
    }

    /// 启动 WebDAV 周期同步 worker：每 15 分钟从 webdav_remotes 表读全部
    /// remote + 解密凭据，逐个增量重扫。原子 flag 防重入 + RAII guard 复位。
    pub fn start_webdav_sync_worker(state: std::sync::Arc<AppState>) {
        if state
            .webdav_sync_worker_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::debug!("WebDAV sync worker already running, skipping");
            return;
        }
        std::thread::spawn(move || {
            struct FlagGuard<'a>(&'a std::sync::atomic::AtomicBool);
            impl Drop for FlagGuard<'_> {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::SeqCst);
                }
            }
            let _guard = FlagGuard(&state.webdav_sync_worker_running);

            tracing::info!("WebDAV sync worker started");
            loop {
                // vault 锁定则退出 —— 下次 unlock 会重新 start。
                {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    if !matches!(vault.state(), attune_core::vault::VaultState::Unlocked) {
                        break;
                    }
                }

                // 从 webdav_remotes 表读全部已配置 remote + 解密凭据（snapshot 后释放锁）。
                let remotes: Vec<attune_core::store::webdav_remotes::WebDavRemoteRow> = {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    let dek = match vault.dek_db() {
                        Ok(k) => k,
                        Err(_) => break, // vault 锁定 → 退出，下次 unlock 重启
                    };
                    vault.store().list_webdav_remotes(&dek).unwrap_or_default()
                };

                for remote in remotes {
                    let config = attune_core::scanner_webdav::WebDavConfig {
                        url: remote.url.clone(),
                        username: remote.username.clone(),
                        password: remote.password.clone(),
                        depth: remote.depth,
                    };
                    // 只打印 dir_id / url，不 log password（避免凭据泄露）。
                    tracing::info!(
                        "WebDAV sync: scanning dir={} url={}",
                        remote.dir_id,
                        remote.url
                    );
                    if let Err(e) = crate::ingest_webdav::sync_webdav_dir(
                        &state,
                        &remote.dir_id,
                        config,
                        &remote.corpus_domain,
                    ) {
                        tracing::warn!("WebDAV sync for dir {} failed: {e}", remote.dir_id);
                    }
                }

                // unlock 后立即跑首轮，之后每 15 分钟一次。
                std::thread::sleep(std::time::Duration::from_secs(15 * 60));
            }
            tracing::info!("WebDAV sync worker stopped (vault locked)");
        });
    }

    /// 启动 Email 周期同步 worker：每 15 分钟从 email_accounts 表读全部账户 +
    /// 解密凭据，逐个按 UID 增量同步。原子 flag 防重入 + RAII guard 复位。
    pub fn start_email_sync_worker(state: std::sync::Arc<AppState>) {
        if state
            .email_sync_worker_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::debug!("Email sync worker already running, skipping");
            return;
        }
        std::thread::spawn(move || {
            struct FlagGuard<'a>(&'a std::sync::atomic::AtomicBool);
            impl Drop for FlagGuard<'_> {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::SeqCst);
                }
            }
            let _guard = FlagGuard(&state.email_sync_worker_running);

            tracing::info!("Email sync worker started");
            loop {
                // vault 锁定则退出 —— 下次 unlock 会重新 start。
                {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    if !matches!(vault.state(), attune_core::vault::VaultState::Unlocked) {
                        break;
                    }
                }

                // 从 email_accounts 表读全部账户 + 解密凭据（snapshot 后释放锁）。
                let accounts: Vec<attune_core::store::email_accounts::EmailAccountRow> = {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    let dek = match vault.dek_db() {
                        Ok(k) => k,
                        Err(_) => break,
                    };
                    vault.store().list_email_accounts(&dek).unwrap_or_default()
                };

                for account in accounts {
                    let config = attune_core::ingest::EmailConfig {
                        host: account.host.clone(),
                        port: account.port,
                        username: account.username.clone(),
                        password: account.password.clone(),
                        folders: account.folders.clone(),
                    };
                    // 只打印 dir_id / host / username，不 log password。
                    tracing::info!(
                        "Email sync: account dir={} host={} user={}",
                        account.dir_id,
                        account.host,
                        account.username
                    );
                    if let Err(e) = crate::ingest_email::sync_email_account(
                        &state,
                        &account.dir_id,
                        config,
                        &account.corpus_domain,
                    ) {
                        tracing::warn!("Email sync for account {} failed: {e}", account.dir_id);
                    }
                }

                // unlock 后立即跑首轮，之后每 15 分钟一次。
                std::thread::sleep(std::time::Duration::from_secs(15 * 60));
            }
            tracing::info!("Email sync worker stopped (vault locked)");
        });
    }

    /// 启动 RSS 周期同步 worker：每分钟 wake，从 rss_feeds 表读所有 enabled 订阅，
    /// 跑每个"到期"（now >= last_polled_at + poll_interval_minutes）的 feed。
    /// 原子 flag 防重入 + RAII guard 复位。
    ///
    /// 与 WebDAV/Email worker 不同点：每个 feed 有独立 poll_interval_minutes，
    /// worker 自身 tick 周期固定 1 min，到期判断在 worker 内做。这样高频订阅
    /// （5 min）和低频订阅（24h）能共用一个 worker。
    pub fn start_rss_sync_worker(state: std::sync::Arc<AppState>) {
        if state
            .rss_sync_worker_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::debug!("RSS sync worker already running, skipping");
            return;
        }
        std::thread::spawn(move || {
            struct FlagGuard<'a>(&'a std::sync::atomic::AtomicBool);
            impl Drop for FlagGuard<'_> {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::SeqCst);
                }
            }
            let _guard = FlagGuard(&state.rss_sync_worker_running);

            tracing::info!("RSS sync worker started");
            loop {
                // vault 锁定则退出 —— 下次 unlock 会重新 start。
                {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    if !matches!(vault.state(), attune_core::vault::VaultState::Unlocked) {
                        break;
                    }
                }

                // 从 rss_feeds 表读全部订阅 + 解密 URL（snapshot 后释放锁）。
                let feeds: Vec<attune_core::store::rss_feeds::RssFeedRow> = {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    let dek = match vault.dek_db() {
                        Ok(k) => k,
                        Err(_) => break, // vault 锁定 → 退出
                    };
                    vault.store().list_rss_feeds(&dek).unwrap_or_default()
                };

                let now = chrono::Utc::now();
                for feed in feeds {
                    if !feed.enabled {
                        continue;
                    }
                    // 到期判断：last_polled_at 为 None（首次）或 now - last >= interval。
                    let due = match feed.last_polled_at.as_deref() {
                        None => true,
                        Some(ts) => match chrono::DateTime::parse_from_rfc3339(ts) {
                            Ok(prev) => {
                                let elapsed =
                                    now.signed_duration_since(prev.with_timezone(&chrono::Utc));
                                elapsed
                                    >= chrono::Duration::minutes(feed.poll_interval_minutes as i64)
                            }
                            Err(_) => true,
                        },
                    };
                    if !due {
                        continue;
                    }
                    // 只打印 feed_id + name（不含 URL，URL 解密后仅在此函数内消费）。
                    tracing::info!("RSS sync: polling feed id={} name={}", feed.id, feed.name);
                    if let Err(e) = crate::ingest_rss::sync_rss_feed(&state, &feed.id) {
                        tracing::warn!("RSS sync for feed {} failed: {e}", feed.id);
                    }
                }

                // 1 min tick；feed 到期判断在 worker 内做。
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
            tracing::info!("RSS sync worker stopped (vault locked)");
        });
    }

    /// 信息监控一遍（spec 2026-06-19 A+C+D）：对**最近入库的 item** × enabled watch 做
    /// 确定性匹配 / triage / 去重，命中 upsert 进 watch_hits。**零 LLM**（evaluate 签名无 LLM
    /// 句柄）。源无关：在 item 层工作，覆盖任何已落地 connector。
    ///
    /// 增量近似（spec §11 R3）：只取最近 `recent_limit` 个 item（按 created_at desc），不全表
    /// 重算；watch_hits 的 UNIQUE(watch_id,item_id) 做幂等去重，重复 pass 不产生重复 hit。
    pub fn run_monitoring_pass(state: &std::sync::Arc<AppState>, recent_limit: usize) -> usize {
        use attune_core::monitoring::{ItemMeta, WatchMatcher};

        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = match vault.dek_db() {
            Ok(k) => k,
            Err(_) => return 0, // locked
        };
        let store = vault.store();
        let watches = store.list_enabled_watches(&dek).unwrap_or_default();
        if watches.is_empty() {
            return 0;
        }
        let summaries = store.list_items(recent_limit, 0).unwrap_or_default();
        if summaries.is_empty() {
            return 0;
        }
        // 拉每个 item 的 content + entities + 向量（向量复用已算好的，不重新 embed）。
        let vec_guard = state.vectors.lock().unwrap_or_else(|e| e.into_inner());
        let items: Vec<ItemMeta> = summaries
            .into_iter()
            .filter_map(|s| {
                let item = store.get_item(&dek, &s.id).ok().flatten()?;
                let entities = attune_core::entities::extract_entities(&item.content);
                let vector = vec_guard.as_ref().and_then(|v| v.get_vector(&s.id));
                // content_hash 不经 list_items 透出；近似去重走标题/向量足够（入库层
                // content_hash 已防完全相同内容二次入库）。
                Some(ItemMeta {
                    id: item.id,
                    title: item.title,
                    content: item.content,
                    source_type: item.source_type,
                    vector,
                    entities,
                    created_at: item.created_at,
                    content_hash: String::new(),
                })
            })
            .collect();
        drop(vec_guard);

        let now = chrono::Utc::now().to_rfc3339();
        let hits = WatchMatcher::default().evaluate(
            &items,
            &watches,
            &std::collections::HashMap::new(),
            &now,
        );
        let n = store.upsert_watch_hits(&hits).unwrap_or(0);
        if n > 0 {
            tracing::info!(
                "monitoring pass: {n} new watch hit(s) across {} watch(es)",
                watches.len()
            );
        }
        n
    }

    /// 启动后台 digest worker（spec §3.4）：每小时 tick，跑一遍监控匹配（搭车节奏），
    /// 并对到期 watch（now ≥ last_digested_at + period）落 digest（此 MVP 仅做匹配 +
    /// 标记到期；digest 卡渲染由 trigger_digest route / 后续 suggestion 卡通道暴露）。
    /// vault lock 时退出并重置标志。
    pub fn start_monitoring_worker(state: std::sync::Arc<AppState>) {
        if state
            .monitoring_worker_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::debug!("monitoring worker already running, skipping");
            return;
        }
        std::thread::spawn(move || {
            struct FlagGuard<'a>(&'a std::sync::atomic::AtomicBool);
            impl Drop for FlagGuard<'_> {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::SeqCst);
                }
            }
            let _guard = FlagGuard(&state.monitoring_worker_running);
            tracing::info!("monitoring worker started");
            loop {
                {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    if !matches!(vault.state(), attune_core::vault::VaultState::Unlocked) {
                        break;
                    }
                }
                // 一遍匹配（零成本）；命中 upsert 入 watch_hits。
                let _ = AppState::run_monitoring_pass(&state, 500);
                // 10 min tick（远低于 connector 节奏的搭车间隔，捕获新入库 item）。
                std::thread::sleep(std::time::Duration::from_secs(600));
            }
            tracing::info!("monitoring worker stopped (vault locked)");
        });
    }

    /// 启动后台目录重扫 worker（每 30 分钟扫描一次绑定目录）
    /// 使用 AtomicBool 防止重复启动；vault lock 时自动退出并重置标志。
    pub fn start_rescan_worker(state: std::sync::Arc<AppState>) {
        if state
            .rescan_worker_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::debug!("Rescan worker already running, skipping");
            return;
        }

        // H1：rescan = FileScanner 类，受治理；30 分钟周期任务，单次扫描期间也会
        // 在每个目录 dir 之间 check should_run 以便快速响应 Pause。
        let governor = global_registry().register(TaskKind::FileScanner);

        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(30 * 60)); // 30 minutes

                // Check vault still unlocked
                let (dek, dirs) = {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    if !matches!(vault.state(), attune_core::vault::VaultState::Unlocked) {
                        break;
                    }
                    let dek = match vault.dek_db() {
                        Ok(d) => d,
                        Err(_) => break,
                    };
                    let dirs = vault.store().list_bound_directories().unwrap_or_default();
                    (dek, dirs)
                };

                for dir in &dirs {
                    // H1：每个目录都给 governor 一个机会响应 Pause / 超 budget
                    while !governor.should_run() {
                        std::thread::sleep(std::time::Duration::from_millis(500));
                    }
                    if dir.path.is_empty() || dir.path.starts_with("webdav:") {
                        continue;
                    }

                    let path = std::path::Path::new(&dir.path);
                    if !path.exists() {
                        continue;
                    }

                    let file_types = dir.file_type_list();

                    // NOTE: 持锁执行 scan_directory —— 每个目录典型 <5s（文件 hash 增量 diff）。
                    // 对比 skill_evolver 的 LLM 调用（15s+，已拆三阶段），此处仍在可接受
                    // 范围内，不拆解。如未来扫描变慢（大目录 / 慢 HDD），可把文件遍历放锁
                    // 外，仅 DB 写操作持锁。
                    let ingest_options =
                        crate::local_scheduler::ingest_options_from_state(&state, None);
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    match attune_core::scanner::scan_directory_with_options(
                        vault.store(),
                        &dek,
                        &dir.id,
                        path,
                        dir.recursive,
                        &file_types,
                        &ingest_options,
                    ) {
                        Ok(r) => {
                            if r.new_files > 0 || r.updated_files > 0 || r.deleted_files > 0 {
                                tracing::info!(
                                    "Rescan {}: {} new, {} updated, {} deleted",
                                    dir.path,
                                    r.new_files,
                                    r.updated_files,
                                    r.deleted_files
                                );
                            }
                        }
                        Err(e) => tracing::warn!("Rescan {} failed: {}", dir.path, e),
                    }
                    drop(vault);
                    std::thread::sleep(governor.after_work());
                }
            }
            state.rescan_worker_running.store(false, Ordering::SeqCst);
            tracing::info!("Rescan worker stopped (vault locked)");
        });
    }

    /// 启动后台 embedding queue worker（在 init_search_engines 之后调用）
    /// 使用 AtomicBool 防止重复启动；vault lock 时自动退出并重置 AtomicBool。
    pub fn start_queue_worker(state: std::sync::Arc<AppState>) {
        if state
            .queue_worker_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::debug!("Queue worker already running, skipping");
            return;
        }

        // H1：embedding 队列受 EmbeddingQueue 治理（默认 Balanced 25% CPU / 1GB RAM）。
        // 此 worker 是 attune-server 生产路径，比 attune-core::queue::QueueWorker 多 flush 逻辑。
        let governor = global_registry().register(TaskKind::EmbeddingQueue);

        std::thread::spawn(move || {
            let batch_size = embed_queue_batch_size();
            tracing::info!("Queue worker started (batch_size={batch_size})");
            const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
            const MAX_ATTEMPTS: i32 = 3;

            // 持久化节流：累积 N 个向量或 T 时间后 flush 一次
            let mut flush_counter: usize = 0;
            let mut last_flush = std::time::Instant::now();

            loop {
                // 检查 vault 是否仍处于 unlocked 状态
                let vault_unlocked = {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    matches!(vault.state(), attune_core::vault::VaultState::Unlocked)
                };
                if !vault_unlocked {
                    break;
                }

                // H1：超 budget 或全局 pause 时短 sleep
                if !governor.should_run() {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    continue;
                }

                // 检查 embedding + vectors + fulltext 是否就绪
                let (embedding, embedding_is_local) = state.embedding_with_locality();
                let vectors_ready = state
                    .vectors
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_some();
                let fulltext_ready = state
                    .fulltext
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_some();

                if embedding.is_none() || !vectors_ready || !fulltext_ready {
                    std::thread::sleep(POLL_INTERVAL);
                    continue;
                }
                let embedding = embedding.expect("is_none() checked above");

                // Cloud embedding shares the explicit privacy.llm consent. Keep
                // rows pending while it is disabled so a later opt-in can resume.
                // This check precedes `is_available()` because a cloud provider's
                // health probe is itself network egress.
                let cloud_embedding_enabled =
                    embedding_is_local || crate::routes::privacy::outbound_enabled(&state, "llm");
                if !cloud_embedding_enabled {
                    std::thread::sleep(POLL_INTERVAL);
                    continue;
                }

                if !embedding.is_available() {
                    std::thread::sleep(POLL_INTERVAL);
                    continue;
                }

                // 取一批任务
                let tasks_result = {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    vault.store().dequeue_embeddings(batch_size)
                };
                let tasks = match tasks_result {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::warn!("Queue worker dequeue error: {}", e);
                        std::thread::sleep(POLL_INTERVAL);
                        continue;
                    }
                };

                if tasks.is_empty() {
                    std::thread::sleep(POLL_INTERVAL);
                    continue;
                }

                // 分区：embed 本 worker 处理，其余（classify 等）回 pending
                let (embed_tasks, other_tasks): (Vec<_>, Vec<_>) =
                    tasks.into_iter().partition(|t| t.task_type == "embed");

                if !other_tasks.is_empty() {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    for task in &other_tasks {
                        let _ = vault.store().mark_task_pending(task.id);
                    }
                }

                if embed_tasks.is_empty() {
                    continue;
                }

                // #82 P0 OutboundGate::Embedding enforcement.
                // When the active provider points to a cloud endpoint (embedding_is_local=false),
                // filter out tasks whose item has PrivacyTier::L0 ("永不出网").
                // Local scheduler providers are always permitted.
                let (embed_tasks, cloud_payloads) = {
                    if embedding_is_local {
                        // Local: all tasks pass, no gate needed.
                        (embed_tasks, None)
                    } else {
                        // Cloud endpoint: check per-item privacy tier.
                        // Rebuild redactor once per batch (not per-task).
                        let redactor = Redactor::new();
                        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                        let vault_unlocked =
                            matches!(vault.state(), attune_core::vault::VaultState::Unlocked);
                        let mut allowed = Vec::with_capacity(embed_tasks.len());
                        let mut payloads = Vec::with_capacity(embed_tasks.len());
                        for task in embed_tasks {
                            let is_l0 = match vault.store().get_item_privacy_tier(&task.item_id) {
                                Ok(tier) => {
                                    matches!(tier, attune_core::store::audit::PrivacyTier::L0)
                                }
                                Err(attune_core::error::VaultError::NotFound(_)) => {
                                    // Stale queue row: there is no content left to index.
                                    let _ = vault.store().mark_embedding_done(task.id);
                                    continue;
                                }
                                Err(e) => {
                                    // A failed tier lookup must never be interpreted as
                                    // cloud-safe. Preserve the row and retry later.
                                    tracing::warn!(
                                        "cloud embedding privacy-tier lookup failed for {}: {e}",
                                        task.item_id
                                    );
                                    let _ = vault.store().mark_task_pending(task.id);
                                    continue;
                                }
                            };
                            match enforce_cloud_embedding_payload(
                                cloud_embedding_enabled,
                                vault_unlocked,
                                is_l0,
                                &redactor,
                                &task.chunk_text,
                            ) {
                                Ok(redacted) => {
                                    // Keep the original task for local full-text
                                    // indexing; only this redacted parallel payload
                                    // may be sent to the cloud embedder.
                                    payloads.push(redacted);
                                    allowed.push(task);
                                }
                                Err(attune_core::outbound_gate::OutboundError::L0CloudBlocked) => {
                                    tracing::warn!(
                                        "#82 OutboundGate::Embedding: L0 chunk skipped \
                                         (item={}, chunk={}) — cloud embedding blocked for L0 content",
                                        task.item_id, task.chunk_idx
                                    );
                                    // Mark done so it doesn't re-queue and block indefinitely.
                                    let _ = vault.store().mark_embedding_done(task.id);
                                }
                                Err(
                                    attune_core::outbound_gate::OutboundError::Disabled(_)
                                    | attune_core::outbound_gate::OutboundError::VaultLocked,
                                ) => {
                                    // Consent/vault state can change between preflight
                                    // and this batch. Preserve work for a later pass.
                                    let _ = vault.store().mark_task_pending(task.id);
                                }
                                Err(e) => {
                                    tracing::warn!("#82 OutboundGate::Embedding refused: {e}");
                                    let _ = vault.store().mark_embedding_done(task.id);
                                }
                            }
                        }
                        (allowed, Some(payloads))
                    }
                };

                if embed_tasks.is_empty() {
                    continue;
                }

                let embedding_result = {
                    let texts: Vec<&str> = match cloud_payloads.as_ref() {
                        Some(payloads) => payloads.iter().map(String::as_str).collect(),
                        None => embed_tasks
                            .iter()
                            .map(|task| task.chunk_text.as_str())
                            .collect(),
                    };
                    embedding.embed(&texts)
                };
                let embeddings = match embedding_result {
                    Ok((embeddings, _usage)) => embeddings,
                    Err(e) => {
                        tracing::warn!("Embedding batch failed: {}", e);
                        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                        for task in &embed_tasks {
                            let _ = vault.store().mark_embedding_failed(task.id, MAX_ATTEMPTS);
                        }
                        std::thread::sleep(POLL_INTERVAL);
                        continue;
                    }
                };

                // Embedding generation happens above without hot-path locks.
                // Only the index/DB writeback takes fulltext → vectors → vault,
                // so scheduler latency cannot stall search/delete/update.
                let done_ids: Vec<i64> = {
                    let ft_guard = state.fulltext.lock().unwrap_or_else(|e| e.into_inner());
                    let mut vecs_guard = state.vectors.lock().unwrap_or_else(|e| e.into_inner());
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());

                    let (Some(vi), Some(ft)) = (vecs_guard.as_mut(), ft_guard.as_ref()) else {
                        tracing::debug!(
                            "Queue worker: vectors/fulltext index unavailable mid-batch"
                        );
                        drop(ft_guard);
                        drop(vecs_guard);
                        drop(vault);
                        std::thread::sleep(POLL_INTERVAL);
                        continue;
                    };

                    match attune_core::queue::index_embedding_results(
                        vault.store(),
                        vi,
                        ft,
                        &embed_tasks,
                        &embeddings,
                    ) {
                        Ok(ids) => {
                            for id in &ids {
                                let _ = vault.store().mark_embedding_done(*id);
                            }
                            ids
                        }
                        Err(e) => {
                            tracing::warn!("Embedding batch failed: {}", e);
                            for task in &embed_tasks {
                                let _ = vault.store().mark_embedding_failed(task.id, MAX_ATTEMPTS);
                            }
                            drop(ft_guard);
                            drop(vecs_guard);
                            drop(vault);
                            std::thread::sleep(POLL_INTERVAL);
                            continue;
                        }
                    }
                };

                // 定期把 vector index flush 到加密磁盘文件
                // 条件：每累计 FLUSH_BATCH_THRESHOLD 个新向量 or 距上次 flush 超过 FLUSH_INTERVAL
                const FLUSH_BATCH_THRESHOLD: usize = 100;
                const FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5 * 60);
                flush_counter += done_ids.len();
                let should_flush = flush_counter >= FLUSH_BATCH_THRESHOLD
                    || last_flush.elapsed() >= FLUSH_INTERVAL;
                if should_flush && flush_counter > 0 {
                    let dek_opt = state
                        .vault
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .dek_db()
                        .ok();
                    let vecs = state.vectors.lock().unwrap_or_else(|e| e.into_inner());
                    if let (Some(dek), Some(vi)) = (dek_opt, vecs.as_ref()) {
                        let p = attune_core::platform::data_dir().join("vectors.encbin");
                        if let Err(e) = vi.save_encrypted(&dek, &p) {
                            tracing::warn!("Vector flush failed: {e}");
                        } else {
                            tracing::info!(
                                "Vector index flushed ({} entries after +{} new)",
                                vi.len(),
                                flush_counter
                            );
                        }
                    }
                    flush_counter = 0;
                    last_flush = std::time::Instant::now();
                }

                tracing::debug!("Queue worker processed {} embed tasks", embed_tasks.len());

                // H1：批次完成后退让，让 governor 决定下次 sleep 时长
                std::thread::sleep(governor.after_work());
            }

            // 退出时重置标志 + 最后一次 flush
            state.queue_worker_running.store(false, Ordering::SeqCst);
            if flush_counter > 0 {
                let dek_opt = state
                    .vault
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .dek_db()
                    .ok();
                let vecs = state.vectors.lock().unwrap_or_else(|e| e.into_inner());
                if let (Some(dek), Some(vi)) = (dek_opt, vecs.as_ref()) {
                    let p = attune_core::platform::data_dir().join("vectors.encbin");
                    let _ = vi.save_encrypted(&dek, &p);
                }
            }
            tracing::info!("Queue worker stopped (vault locked or engines cleared)");
        });
    }

    /// 启动后台技能进化 worker（在 init_search_engines 之后调用）
    ///
    /// 每 4 小时检查一次未处理信号数；达到阈值（默认 10 条）时调用 LLM 分析失败查询
    /// 并将扩展词静默写入 app_settings，无任何用户通知或新 UI 入口。
    pub fn start_skill_evolver(state: std::sync::Arc<AppState>) {
        // Signals may contain vault-derived text but have no interactive L0
        // context, so autonomous evolution is deliberately local-only.
        if !state.llm().as_ref().is_some_and(|llm| llm.is_local()) {
            return;
        }

        if state
            .evolve_worker_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::debug!("Skill evolver already running, skipping");
            return;
        }

        // H1：SkillEvolution 受治理 + LLM 速率限制（默认 Balanced 10 calls/h）。
        // 4 小时检查一次本身已是低频，但仍接入 governor 以便：
        // (1) 全局 Pause 立即生效  (2) 切档时 LLM 配额自动调整
        let governor = global_registry().register(TaskKind::SkillEvolution);

        std::thread::spawn(move || {
            tracing::info!(
                "Skill evolver started (runs every 4h or at {} signals)",
                attune_core::skill_evolution::EVOLVE_THRESHOLD
            );
            const CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(4 * 60 * 60);

            loop {
                std::thread::sleep(CHECK_INTERVAL);

                // 检查 vault 是否仍处于 unlocked 状态
                let vault_unlocked = {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    matches!(vault.state(), attune_core::vault::VaultState::Unlocked)
                };
                if !vault_unlocked {
                    break;
                }

                // H1：被 Pause / 超 budget 时跳过本周期（4h 后再试）
                if !governor.should_run() {
                    continue;
                }

                let llm = match state.llm() {
                    Some(l) if l.is_local() => l,
                    Some(_) => {
                        tracing::debug!(
                            "Skill evolver paused: autonomous cloud LLM calls are disabled"
                        );
                        continue;
                    }
                    None => break,
                };

                // 三阶段锁释放（CRITICAL fix：旧版在 LLM 调用期间持有 vault 锁 15s+，
                // 阻塞所有并发 route）。Phase 1 锁读信号 → Phase 2 无锁跑 LLM →
                // Phase 3 锁写回。与 chat.rs 的上下文压缩路径同构。
                let signals = {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    match attune_core::skill_evolution::prepare_evolution_cycle(vault.store()) {
                        Ok(Some(s)) => s,
                        Ok(None) => continue, // 信号不足
                        Err(e) => {
                            tracing::warn!("Skill evolver prepare error: {}", e);
                            continue;
                        }
                    }
                    // vault 在此处 drop，释放锁
                };

                // H1：LLM 配额检查
                if !governor.allow_llm_call() {
                    tracing::info!(
                        "Skill evolver LLM quota exceeded (per-hour cap), skipping cycle"
                    );
                    continue;
                }

                // Phase 2（无锁）：LLM 调用，可能耗时 15s+
                let expansions =
                    match attune_core::skill_evolution::generate_expansions(llm.as_ref(), &signals)
                    {
                        Ok(e) => e,
                        Err(e) => {
                            tracing::warn!("Skill evolver LLM error: {}", e);
                            continue;
                        }
                    };

                // Phase 3（锁）：合并 + 标记已处理
                {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    match attune_core::skill_evolution::apply_evolution_result(
                        vault.store(),
                        &signals,
                        &expansions,
                    ) {
                        Ok(0) => tracing::debug!("Skill evolver: no new expansions"),
                        Ok(n) => tracing::info!("Skill evolver: {} expansion entries updated", n),
                        Err(e) => tracing::warn!("Skill evolver apply error: {}", e),
                    }
                }
            }

            state.evolve_worker_running.store(false, Ordering::SeqCst);
            tracing::info!("Skill evolver stopped (vault locked)");
        });
    }

    /// A1：启动 Memory Consolidator 后台 worker（2026-04-27）。
    ///
    /// 每 6 小时跑一次：扫 chunk_summaries 按天聚合 → LLM 总结成 episodic memory。
    /// 三阶段锁释放（与 skill_evolver 同构），每周期最多 4 个 bundle / 4 次 LLM 调用。
    /// 受 H1 [`TaskKind::MemoryConsolidation`] governor 治理 + LLM 配额限制。
    pub fn start_memory_consolidator(state: std::sync::Arc<AppState>) {
        // Memory bundles carry vault-derived text without per-request privacy
        // context. Autonomous consolidation is therefore local-LLM only.
        if !state.llm().as_ref().is_some_and(|llm| llm.is_local()) {
            return;
        }

        if state
            .memory_consolidator_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            tracing::debug!("Memory consolidator already running, skipping");
            return;
        }

        let governor = global_registry().register(TaskKind::MemoryConsolidation);

        std::thread::spawn(move || {
            tracing::info!("Memory consolidator started (runs every 6h)");
            const CYCLE: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

            loop {
                std::thread::sleep(CYCLE);

                let vault_unlocked = {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    matches!(vault.state(), attune_core::vault::VaultState::Unlocked)
                };
                if !vault_unlocked {
                    break;
                }

                if !governor.should_run() {
                    continue;
                }

                let llm = match state.llm() {
                    Some(l) if l.is_local() => l,
                    Some(_) => {
                        tracing::debug!(
                            "Memory consolidator paused: autonomous cloud LLM calls are disabled"
                        );
                        continue;
                    }
                    None => break,
                };

                // 用 std time 避免引入 chrono 到 attune-server。SystemTime 之后转 secs。
                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);

                // I4：Phase 1 同步记下 LLM model 名，避免 Phase 3 写入时与实际生成 LLM 不一致。
                let model_name = llm.model_name().to_string();

                // Phase 1（持锁）：prepare bundles。Phase 1 dek 不带出锁外，
                // Phase 3 重新取 dek 避免使用已注销的密钥（S2 修复）。
                let bundles = {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    let dek = match vault.dek_db() {
                        Ok(d) => d,
                        Err(_) => break,
                    };
                    match attune_core::memory_consolidation::prepare_consolidation_cycle(
                        vault.store(),
                        &dek,
                        now_secs,
                    ) {
                        Ok(Some(b)) => Some(b),
                        Ok(None) => None,
                        Err(e) => {
                            tracing::warn!("Memory consolidator prepare error: {}", e);
                            None
                        }
                    }
                };
                let Some(bundles) = bundles else { continue };

                // Phase 2（无锁）：每 bundle 单独 check 配额 + LLM 调用（S1 修复）。
                // 配额耗尽时剩余 bundle 留 None，下周期 INSERT OR IGNORE 保证幂等不丢失。
                let mut summaries: Vec<Option<String>> = Vec::with_capacity(bundles.len());
                let mut deferred = 0usize;
                for bundle in &bundles {
                    if !governor.allow_llm_call() {
                        deferred = bundles.len() - summaries.len();
                        for _ in 0..deferred {
                            summaries.push(None);
                        }
                        break;
                    }
                    summaries.push(
                        attune_core::memory_consolidation::generate_one_episodic_memory(
                            llm.as_ref(),
                            bundle,
                        ),
                    );
                }
                if deferred > 0 {
                    tracing::info!(
                        "Memory consolidator LLM quota exhausted mid-cycle, {} bundle(s) deferred",
                        deferred
                    );
                }

                // Phase 3（持锁）：幂等写 memories — 复查 vault 状态 + 重新取 dek（S2 修复）
                {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    if !matches!(vault.state(), attune_core::vault::VaultState::Unlocked) {
                        tracing::info!(
                            "Vault locked during consolidation, discarding {} bundle result(s)",
                            bundles.len()
                        );
                        break;
                    }
                    let dek = match vault.dek_db() {
                        Ok(d) => d,
                        Err(_) => break,
                    };
                    match attune_core::memory_consolidation::apply_consolidation_result(
                        vault.store(),
                        &dek,
                        &bundles,
                        &summaries,
                        &model_name,
                        now_secs,
                    ) {
                        Ok(0) => tracing::debug!("Memory consolidator: no new memories"),
                        Ok(n) => tracing::info!("Memory consolidator: {} new episodic memories", n),
                        Err(e) => tracing::warn!("Memory consolidator apply error: {}", e),
                    }
                }

                // ── Multi-layer memory: embed L2, build L3, demote cold ─────────
                // Embedding L2/L3 summaries is cost tier 2 (local). The L2→L3 LLM
                // pass is tier 3, gated per-call by the same governor quota.
                Self::run_memory_layering(&state, &governor, &model_name, now_secs);
            }

            state
                .memory_consolidator_running
                .store(false, Ordering::SeqCst);
            tracing::info!("Memory consolidator stopped (vault locked)");
        });
    }

    /// One layering pass: embed any not-yet-embedded L2/L3 memories into
    /// `memory_vectors` + the in-memory index, run the L2→L3 semantic cycle, then
    /// demote cold episodic memories. Called by the consolidator worker after the
    /// episodic pass. All steps are best-effort — failures only `warn`.
    fn run_memory_layering(
        state: &std::sync::Arc<AppState>,
        governor: &std::sync::Arc<attune_core::resource_governor::TaskGovernor>,
        model_name: &str,
        now_secs: i64,
    ) {
        // Embed any memories that have no memory_vectors row yet (covers freshly
        // inserted episodic rows + previously-deferred ones).
        Self::embed_pending_memories(state, now_secs);

        // L2→L3 semantic cycle (three-stage, lock discipline mirrors A1).
        let embeddings: std::collections::HashMap<String, Vec<f32>> = {
            let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
            vault
                .store()
                .list_all_memory_vectors()
                .map(|rows| {
                    rows.into_iter()
                        .map(|r| (r.memory_id, r.embedding))
                        .collect()
                })
                .unwrap_or_default()
        };
        let clusters = {
            let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
            if !matches!(vault.state(), attune_core::vault::VaultState::Unlocked) {
                return;
            }
            let dek = match vault.dek_db() {
                Ok(d) => d,
                Err(_) => return,
            };
            match attune_core::memory::prepare_semantic_cycle(vault.store(), &dek, &embeddings) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("semantic prepare error: {e}");
                    None
                }
            }
        };

        if let Some(clusters) = clusters {
            let llm = match state.llm() {
                Some(l) if l.is_local() => l,
                Some(_) => {
                    tracing::debug!(
                        "Semantic memory cycle skipped: autonomous cloud LLM calls are disabled"
                    );
                    return;
                }
                None => return,
            };
            // Per-cluster quota check (each LLM call costs 1 quota — same as A1).
            let mut summaries: Vec<Option<String>> = Vec::with_capacity(clusters.len());
            for cluster in &clusters {
                if !governor.allow_llm_call() {
                    for _ in summaries.len()..clusters.len() {
                        summaries.push(None);
                    }
                    break;
                }
                summaries.push(attune_core::memory::generate_one_semantic_memory(
                    llm.as_ref(),
                    cluster,
                ));
            }
            let new_ids: Vec<Option<String>> = {
                let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                if !matches!(vault.state(), attune_core::vault::VaultState::Unlocked) {
                    return;
                }
                let dek = match vault.dek_db() {
                    Ok(d) => d,
                    Err(_) => return,
                };
                match attune_core::memory::apply_semantic_result(
                    vault.store(),
                    &dek,
                    &clusters,
                    &summaries,
                    model_name,
                    now_secs,
                ) {
                    Ok((r, ids)) => {
                        if r.inserted > 0 {
                            tracing::info!(
                                "Memory consolidator: {} new semantic memories ({} superseded)",
                                r.inserted,
                                r.superseded,
                            );
                        }
                        ids
                    }
                    Err(e) => {
                        tracing::warn!("semantic apply error: {e}");
                        vec![]
                    }
                }
            };
            // Embed the new semantic summaries so they become searchable.
            if new_ids.iter().any(|i| i.is_some()) {
                Self::embed_pending_memories(state, now_secs);
            }
        }

        // Cold demotion — pure SQL, zero LLM. COLD_AGE default 180 days (plan §2.2).
        const COLD_AGE_SECS: i64 = 180 * 24 * 3600;
        {
            let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(vault.state(), attune_core::vault::VaultState::Unlocked) {
                match vault.store().demote_cold_memories(now_secs, COLD_AGE_SECS) {
                    Ok(0) => {}
                    Ok(n) => {
                        tracing::info!("Memory consolidator: {n} episodic memories demoted to cold")
                    }
                    Err(e) => tracing::warn!("cold demotion error: {e}"),
                }
            }
        }

        // Memory continuity (2026-06-15): re-embed memory vectors whose stored model
        // (dimension key) differs from the active embedder. A model swap that changes
        // dims silently strands old vectors (assembler's dim-guard skips them); this
        // batch heals them in place. Cost tier 2 (local re-embed, no LLM) — pausable.
        Self::run_memory_reindex_batch(state);
    }

    /// One memory-reindex pass: for the current embedding signature, take a bounded
    /// batch of stale `memory_vectors` rows and re-embed each summary in place
    /// (REPLACE to the current dimension key). Best-effort — failures only `warn`.
    ///
    /// WHY batched + folded into the consolidator loop (not its own worker): reindex
    /// shares the consolidator's vault-unlocked precondition and tier-2 embedding
    /// budget; a separate thread would duplicate the lock dance and flag plumbing.
    /// WHY a bounded batch per pass: a model swap on a large vault could otherwise
    /// pin the embedder for minutes — the cap lets the next loop tick observe a fresh
    /// pause flag / vault-lock and yield. `dek` is taken inside the vault-unlocked
    /// section and never crosses into vectors/fulltext (§Lock ordering).
    fn run_memory_reindex_batch(state: &std::sync::Arc<AppState>) {
        // Per-pass cap: re-embeds at most this many stale memories, then yields the
        // loop so pause / vault-lock take effect promptly between batches.
        const REINDEX_BATCH: usize = 64;

        if state.reindex_paused() {
            return;
        }
        // A stored memory vector has no trustworthy source-item privacy tier.
        // Without proof that its summary excludes L0 data, migration may only
        // decrypt and re-embed it on a local provider.
        let (embedder, is_local) = state.embedding_with_locality();
        if !is_local {
            tracing::debug!("memory reindex paused: cloud embedding is not L0-safe");
            return;
        }
        let embedder = match embedder {
            Some(e) if e.is_available() => e,
            _ => return,
        };
        let cur_model = attune_core::embed::current_embedding_signature(embedder.as_ref()).model;

        // Reindex each stale id under the vault lock (dek scoped here; never escapes
        // into the vectors/fulltext locks). reindex_one re-embeds via the embedder
        // (no extra lock) and REPLACEs the single memory_vectors row.
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(vault.state(), attune_core::vault::VaultState::Unlocked) {
            return;
        }
        let dek = match vault.dek_db() {
            Ok(d) => d,
            Err(_) => return,
        };
        let store = vault.store();
        let stale: Vec<String> = match store.list_stale_memory_ids(&cur_model) {
            Ok(ids) => ids.into_iter().take(REINDEX_BATCH).collect(),
            Err(e) => {
                tracing::warn!("memory reindex: list_stale failed: {e}");
                return;
            }
        };
        if stale.is_empty() {
            return;
        }
        // Record a migration row for observability/progress (best-effort).
        let mig_id = store
            .start_memory_migration("(mixed)", &cur_model, stale.len() as i64)
            .ok();
        let mut done = 0usize;
        for id in &stale {
            match attune_core::memory::migration::reindex_one(store, &dek, embedder.as_ref(), id) {
                Ok(1) => {
                    done += 1;
                    if let Some(mid) = mig_id {
                        let _ = store.bump_memory_migration_done(mid, 1);
                    }
                }
                Ok(_) => {} // memory gone / empty summary — nothing to re-embed
                Err(e) => tracing::warn!("memory reindex: reindex_one({id}) failed: {e}"),
            }
        }
        if let Some(mid) = mig_id {
            let _ = store.finish_memory_migration(mid);
        }
        tracing::info!(
            "memory reindex: re-embedded {done}/{} stale memory vector(s) to {cur_model}",
            stale.len()
        );
    }

    /// Embed every memory that lacks a `memory_vectors` row, write the vector, and
    /// upsert it into the in-memory `memory_index`. Cost tier 2 (local embedding).
    fn embed_pending_memories(state: &std::sync::Arc<AppState>, now_secs: i64) {
        let (embedder, is_local) = state.embedding_with_locality();
        // L2/L3 summaries do not retain complete source-item provenance. Treat
        // them as potentially derived from L0 and never send them to a cloud
        // embedder, even when generic LLM egress consent is enabled.
        if !is_local {
            tracing::debug!("embed_pending_memories paused: cloud embedding is not L0-safe");
            return;
        }
        let embedder = match embedder {
            Some(e) if e.is_available() => e,
            _ => return,
        };
        // Collect (memory_id, summary) for memories with no vector yet.
        let pending: Vec<(String, String)> = {
            let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
            if !matches!(vault.state(), attune_core::vault::VaultState::Unlocked) {
                return;
            }
            let dek = match vault.dek_db() {
                Ok(d) => d,
                Err(_) => return,
            };
            let store = vault.store();
            let mut out = Vec::new();
            for kind in ["episodic", "semantic"] {
                if let Ok(mems) = store.list_live_memories(&dek, kind, true) {
                    for m in mems {
                        if store.get_memory_vector(&m.id).ok().flatten().is_none() {
                            out.push((m.id, m.summary));
                        }
                    }
                }
            }
            out
        };
        if pending.is_empty() {
            return;
        }

        // Embedding providers don't expose a model name; the dimension is a stable
        // proxy — a model switch that changes dims is what makes vectors mismatch,
        // and same-dim models are interchangeable for cosine ranking.
        let model = format!("embed-dim{}", embedder.dimensions());
        for (mem_id, summary) in pending {
            let vec = match embedder.embed(&[summary.as_str()]) {
                Ok((mut v, _usage)) if !v.is_empty() => v.remove(0),
                _ => continue,
            };
            {
                let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                if !matches!(vault.state(), attune_core::vault::VaultState::Unlocked) {
                    return;
                }
                if let Err(e) = vault
                    .store()
                    .put_memory_vector(&mem_id, &vec, &model, now_secs)
                {
                    tracing::warn!("put_memory_vector failed for {mem_id}: {e}");
                    continue;
                }
            }
            if let Ok(mut g) = state.memory_index.lock() {
                if let Some(idx) = g.as_mut() {
                    let _ = idx.upsert(&mem_id, &vec);
                }
            }
        }
    }

    /// 清除搜索引擎 + 分类引擎 (lock 前调用)
    ///
    /// 顺序：先持久化 vectors（lock 前必须），再清内存。
    pub fn clear_search_engines(&self) {
        let _install_guard = self
            .runtime_install_guard
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        self.runtime_generation.fetch_add(1, Ordering::SeqCst);
        self.clear_search_engines_inner();
    }

    /// Atomically invalidate pending runtime installs, clear current handles,
    /// and lock the vault. Keeping the install guard through `Vault::lock`
    /// prevents a post-unlock bootstrap from slipping into the clear/lock gap.
    pub fn lock_vault_and_clear_runtime(&self) -> attune_core::error::Result<()> {
        let (lock_result, old_plugin_hub) = {
            let _install_guard = self
                .runtime_install_guard
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            self.runtime_generation.fetch_add(1, Ordering::SeqCst);
            self.credential_generation.fetch_add(1, Ordering::SeqCst);
            self.clear_search_engines_inner();
            // Member state and PluginHub hold account identifiers, decrypted
            // entitlement rows, and a live license key independently of the model
            // handles above. Lock must evict those too; the persisted cloud session
            // can authoritatively rebuild them only after a later unlock.
            *self.member_state.lock().unwrap_or_else(|e| e.into_inner()) =
                attune_core::member_session::MemberState::LoggedOut;
            *self
                .member_session_epoch
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = None;
            *self
                .member_verified_at
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = None;
            self.entitlement_cache.hydrate_from_rows(Vec::new());
            // Do not call reload_plugin_hub() here: it takes the same non-reentrant
            // runtime guard. Lock owns the transaction and replaces the handle
            // directly before changing VaultState.
            let old_plugin_hub =
                self.replace_plugin_hub_inner(build_plugin_hub_provider(None, None));
            let vault = self.vault.lock().unwrap_or_else(|e| e.into_inner());
            (vault.lock(), old_plugin_hub)
        };
        drop_plugin_hub_provider(old_plugin_hub);
        lock_result
    }

    fn clear_search_engines_inner(&self) {
        // Persist vectors before clearing（忽略失败：最坏情况重启需重新 embed）
        {
            let dek_opt = self
                .vault
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .dek_db()
                .ok();
            let vecs = self.vectors.lock().unwrap_or_else(|e| e.into_inner());
            if let (Some(dek), Some(vi)) = (dek_opt, vecs.as_ref()) {
                let vectors_path = attune_core::platform::data_dir().join("vectors.encbin");
                if let Err(e) = vi.save_encrypted(&dek, &vectors_path) {
                    tracing::warn!("Vector index flush on lock failed (non-fatal): {e}");
                } else {
                    tracing::info!(
                        "Vector index persisted to {} ({} entries)",
                        vectors_path.display(),
                        vi.len()
                    );
                }
            }
        }
        *self.fulltext.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.vectors.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.memory_index.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.set_embedding(None);
        *self.reranker.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.llm.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.summary_llm.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.vlm.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.web_search.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.tag_index.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self
            .cluster_snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        *self.taxonomy.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.classifier.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.search_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        // 重置初始化标志，确保再次 unlock 后能重新初始化搜索引擎
        self.engines_initialized.store(false, Ordering::SeqCst);
    }

    /// 文档变更后失效 search 结果缓存。
    ///
    /// search_cache 按 query hash 缓存结果。之前只有 vault lock (reset) 和 ingest
    /// 清缓存 — update_item / delete_item / upload / reindex worker 全都不清，导致：
    ///
    /// - 编辑文档后搜旧关键词仍命中（返回编辑前的缓存结果）
    /// - 删除文档后仍搜得到（缓存假命中）
    ///
    /// 真实 E2E 测试 STEP 4 / STEP 8 实测捕获。任何改动 items / 索引的 path 都必须调。
    pub fn invalidate_search_cache(&self) {
        self.search_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    // ── ML provider accessor 方法 (OPT-3 ArcSwap migration prep) ───────────
    //
    // 当前实现: lock+clone Arc 然后立即放锁 → 临界区毫秒内, 比正常 .lock() 短 1000x.
    // 后续 PR (v0.7): 把字段类型从 `Mutex<Option<Arc<dyn T>>>` 改成
    // `arc_swap::ArcSwap<Option<Arc<dyn T>>>`, 这些方法签名不变, 调用方代码无需改.
    //
    // 新代码 (route / async handler) 强烈建议用这些 accessor 而非 .lock() 直接访问 —
    // 准备一并 migrate 到 ArcSwap 时, 旧 .lock() 调用会编译失败 (字段类型不再是 Mutex).

    /// 读 embedding provider — lock+clone Arc. 后续 v0.7 改 ArcSwap (D-R14 受
    /// `dyn Trait` 不支持 load_full 阻碍, 需走 Arc<dyn> + ArcSwapAny<Arc<dyn>>
    /// 直接而非 Option 包装).
    pub fn embedding(&self) -> Option<Arc<dyn EmbeddingProvider>> {
        self.embedding.lock().ok().and_then(|g| g.clone())
    }

    /// Return a coherent provider/locality pair. The embedding mutex is held
    /// while reading both values, matching `set_embedding_with_locality` and
    /// closing the cloud↔local hot-swap race at every egress boundary.
    pub(crate) fn embedding_with_locality(&self) -> (Option<Arc<dyn EmbeddingProvider>>, bool) {
        match self.embedding.lock() {
            Ok(provider) => {
                let provider = provider.clone();
                let is_local = provider.is_some() && self.embedding_is_local.load(Ordering::SeqCst);
                (provider, is_local)
            }
            Err(_) => (None, false),
        }
    }

    /// Inject an embedding provider with conservative cloud locality. Tests or
    /// callers installing a proven local provider must use
    /// `set_embedding_with_locality(..., true)` explicitly.
    pub fn set_embedding(&self, p: Option<Arc<dyn EmbeddingProvider>>) {
        self.set_embedding_with_locality(p, false);
    }

    /// Atomically publish an embedding provider and its locality classification
    /// under the provider mutex. Readers can never observe a stale cloud
    /// provider paired with `is_local=true` during a hot swap.
    pub fn set_embedding_with_locality(
        &self,
        p: Option<Arc<dyn EmbeddingProvider>>,
        is_local: bool,
    ) {
        self.embedding_is_local.store(false, Ordering::SeqCst);
        if let Ok(mut g) = self.embedding.lock() {
            *g = p;
            self.embedding_is_local
                .store(is_local && g.is_some(), Ordering::SeqCst);
        }
    }

    /// 暂停/恢复记忆向量的后台 reindex（POST /memory/reindex 调）。
    pub fn set_reindex_paused(&self, pause: bool) {
        self.memory_reindex_paused.store(pause, Ordering::SeqCst);
    }

    /// 后台 loop 读它决定本周期是否跳过记忆 reindex 批。
    pub fn reindex_paused(&self) -> bool {
        self.memory_reindex_paused.load(Ordering::SeqCst)
    }

    /// 读 LLM provider — 主 chat 用.
    pub fn llm(&self) -> Option<Arc<dyn LlmProvider>> {
        self.llm.lock().ok().and_then(|g| g.clone())
    }

    pub fn set_llm(&self, p: Option<Arc<dyn LlmProvider>>) {
        if let Ok(mut g) = self.llm.lock() {
            *g = p;
        }
    }

    /// Snapshot the member-paid verifier (Arc clone, µs critical section). Used by `login_token`
    /// to prove a "paid" claim before granting `MemberState::Paid` (C1 paywall-bypass fix).
    pub fn member_verifier(&self) -> Arc<dyn attune_core::member_verifier::MemberVerifier> {
        self.member_verifier
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Replace the member-paid verifier — TEST seam. Lets a test inject a verifier that performs a
    /// real (offline, deterministic) license match so the member-gate is exercised without a live
    /// cloud, instead of bypassing it via a blanket client claim.
    pub fn set_member_verifier(&self, v: Arc<dyn attune_core::member_verifier::MemberVerifier>) {
        if let Ok(mut g) = self.member_verifier.lock() {
            *g = v;
        }
    }

    /// 读 summary LLM (摘要/分类轻量 path, 与主 chat 模型可不同).
    pub fn summary_llm(&self) -> Option<Arc<dyn LlmProvider>> {
        self.summary_llm.lock().ok().and_then(|g| g.clone())
    }

    pub fn set_summary_llm(&self, p: Option<Arc<dyn LlmProvider>>) {
        if let Ok(mut g) = self.summary_llm.lock() {
            *g = p;
        }
    }

    /// 读 reranker provider — search rerank 阶段用.
    pub fn reranker(&self) -> Option<Arc<dyn attune_core::infer::RerankProvider>> {
        self.reranker.lock().ok().and_then(|g| g.clone())
    }

    pub fn set_reranker(&self, p: Option<Arc<dyn attune_core::infer::RerankProvider>>) {
        if let Ok(mut g) = self.reranker.lock() {
            *g = p;
        }
    }

    /// 读 web search provider — chat web augmentation 用.
    pub fn web_search(&self) -> Option<Arc<dyn WebSearchProvider>> {
        self.web_search.lock().ok().and_then(|g| g.clone())
    }

    pub fn set_web_search(&self, p: Option<Arc<dyn WebSearchProvider>>) {
        if let Ok(mut g) = self.web_search.lock() {
            *g = p;
        }
    }

    /// 读 VLM provider — 图片 caption / VQA 用.
    pub fn vlm(&self) -> Option<Arc<dyn VlmProvider>> {
        self.vlm.lock().ok().and_then(|g| g.clone())
    }

    pub fn set_vlm(&self, p: Option<Arc<dyn VlmProvider>>) {
        if let Ok(mut g) = self.vlm.lock() {
            *g = p;
        }
    }

    /// 读 classifier — items 自动分类 (热路径, ingest pipeline 调).
    pub fn classifier(&self) -> Option<Arc<Classifier>> {
        self.classifier.lock().ok().and_then(|g| g.clone())
    }

    pub fn set_classifier(&self, p: Option<Arc<Classifier>>) {
        if let Ok(mut g) = self.classifier.lock() {
            *g = p;
        }
    }

    // ── Plan A1 (cache + usage) accessors ───────────────────────────────────
    // Stable API surface that Plan A2's CapabilityRouter consumes (see spec
    // 2026-05-28-cache-context-token-standard-api.md §8). Same lock+clone Arc
    // pattern as embedding/llm above; mirrors `set_*` for hot-reload symmetry.

    /// Read the in-process usage aggregator. `None` until `set_usage` is called
    /// (deferred to the vault-unlock path so the aggregator has a live Store
    /// handle to flush into).
    pub fn usage(&self) -> Option<Arc<attune_core::usage::UsageAggregator>> {
        self.usage_aggregator.lock().ok().and_then(|g| g.clone())
    }

    /// Install / replace / clear the usage aggregator. Called at vault unlock
    /// once the store is shareable (`None` is also valid — locked vault).
    pub fn set_usage(&self, agg: Option<Arc<attune_core::usage::UsageAggregator>>) {
        if let Ok(mut g) = self.usage_aggregator.lock() {
            *g = agg;
        }
    }

    /// ACP-4 Task 2 — install the usage aggregator + spawn its flusher.
    ///
    /// Resolves the A1 "instantiation deferred" blocker (audit C / A1 Task L)
    /// **without** the `Vault::store_arc` refactor: `usage_events` is an
    /// unencrypted telemetry table (token counts / model / provider / latency —
    /// no PII; `query_hash` is a BLAKE3 prefix and off by default), and the
    /// table is created by `Store::open` on the main DB. So the aggregator gets
    /// its **own** `Arc<Mutex<Store>>` opened on the same `db_path` — SQLite WAL
    /// (set by `Store::open`) makes concurrent reader/writer connections safe.
    ///
    /// Idempotent-ish: if it cannot open the DB it logs + leaves the aggregator
    /// `None` (telemetry degrades, main paths unaffected — spec §7 / §11 R8).
    /// `flush_interval_ms` follows spec §11 risk 6 (100ms laptop / 500ms local scheduler appliance);
    /// we use 200ms as a balanced default. Returns the flusher `JoinHandle` (or
    /// `None` on failure) so the caller can abort it on shutdown.
    pub fn install_usage_aggregator(&self) -> Option<tokio::task::JoinHandle<()>> {
        // Already installed → no-op.
        if self.usage().is_some() {
            return None;
        }
        let db_path = attune_core::platform::db_path();
        let store = match attune_core::store::Store::open(&db_path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    "ACP-4: usage aggregator disabled — cannot open telemetry store \
                     at {db_path:?}: {e}"
                );
                return None;
            }
        };
        let store = Arc::new(std::sync::Mutex::new(store));
        let agg = Arc::new(attune_core::usage::UsageAggregator::new(store, 200, 1000));
        let handle = agg.clone().spawn_flusher();
        self.set_usage(Some(agg));
        tracing::info!("ACP-4: usage aggregator installed (flush every 200ms)");
        Some(handle)
    }

    /// G5: the durable job queue's store handle. `None` until
    /// [`AppState::install_job_store`] runs at boot (or if the DB cannot open —
    /// office ASR routes then return 503 `job-store-unavailable`).
    pub fn job_store(&self) -> Option<std::sync::Arc<std::sync::Mutex<attune_core::store::Store>>> {
        self.job_store.lock().ok().and_then(|g| g.clone())
    }

    /// G5: open the durable job-queue store at boot (mirror of
    /// `install_usage_aggregator` — own `Arc<Mutex<Store>>` on `db_path`, WAL
    /// makes the extra connection safe). Runs `recover_on_boot` here — exactly
    /// once per process — NOT inside `Store::open` (which also runs at vault
    /// unlock etc. and would requeue legitimately-Running jobs → double
    /// execution; caught by the 8-worker race test). Idempotent.
    pub fn install_job_store(&self) {
        if self.job_store().is_some() {
            return;
        }
        let db_path = attune_core::platform::db_path();
        match attune_core::store::Store::open(&db_path) {
            Ok(s) => {
                match s.recover_on_boot() {
                    Ok(summary) if summary.requeued + summary.failed_no_retry > 0 => {
                        tracing::info!(
                            "G5: job recovery — {} requeued, {} failed (interrupted-no-retry)",
                            summary.requeued,
                            summary.failed_no_retry
                        );
                    }
                    Ok(_) => {}
                    // Non-fatal: queue still usable; interrupted Running rows are
                    // eventually failed by the worker's timeout sweep.
                    Err(e) => tracing::warn!("G5: recover_on_boot failed (non-fatal): {e}"),
                }
                if let Ok(mut g) = self.job_store.lock() {
                    *g = Some(std::sync::Arc::new(std::sync::Mutex::new(s)));
                }
                tracing::info!("G5: durable job store installed at {db_path:?}");
            }
            Err(e) => {
                tracing::warn!(
                    "G5: durable job queue disabled — cannot open store at {db_path:?}: {e}"
                );
            }
        }
    }

    /// Read the active cache backend. Defaults to `MemoryLruCache` after `new`;
    /// callers can swap to `SqliteEncryptedCache` post-unlock via
    /// `set_cache_backend`.
    pub fn cache_backend(&self) -> Option<Arc<dyn attune_core::cache::CacheBackend>> {
        self.cache_backend.lock().ok().and_then(|g| g.clone())
    }

    /// Install / replace / clear the cache backend.
    pub fn set_cache_backend(&self, c: Option<Arc<dyn attune_core::cache::CacheBackend>>) {
        if let Ok(mut g) = self.cache_backend.lock() {
            *g = c;
        }
    }
}

fn provider_is_local_llm_alias(provider: &str) -> bool {
    let normalized = provider.trim().to_ascii_lowercase();
    matches!(normalized.as_str(), "local")
        || crate::local_scheduler::provider_is_scheduler_native(&normalized)
}

fn build_plugin_hub_provider(
    url: Option<&str>,
    license_key: Option<&str>,
) -> Arc<dyn attune_core::plugin_hub::PluginHubProvider> {
    match (url, license_key) {
        (Some(url), Some(license_key)) => {
            let url = url.to_string();
            let license_key = license_key.to_string();
            let handle = std::thread::spawn(move || {
                Arc::new(attune_core::plugin_hub::HttpPluginHubProvider::new(
                    url,
                    license_key,
                )) as Arc<dyn attune_core::plugin_hub::PluginHubProvider>
            });
            handle.join().unwrap_or_else(|_| {
                tracing::warn!(
                    "plugin_hub: failed to build HttpPluginHubProvider; falling back to mock"
                );
                Arc::new(attune_core::plugin_hub::MockPluginHubProvider::default())
            })
        }
        _ => Arc::new(attune_core::plugin_hub::MockPluginHubProvider::default()),
    }
}

fn drop_plugin_hub_provider(provider: Arc<dyn attune_core::plugin_hub::PluginHubProvider>) {
    // HttpPluginHubProvider owns a reqwest blocking client whose last drop must
    // not happen inside a Tokio worker. Joining also guarantees lock/logout has
    // actually released the state-owned token handle before returning.
    let _ = std::thread::spawn(move || drop(provider)).join();
}

fn should_route_local_endpoint_to_scheduler(endpoint: &str, provider: &str) -> bool {
    crate::local_scheduler::provider_is_scheduler_native(provider)
        || (embedding_endpoint_is_local(endpoint)
            && !crate::local_scheduler::endpoint_is_scheduler(endpoint))
}

/// 按 settings + 硬件构建 LLM provider。
///
/// 优先级：
/// 1. settings.llm.endpoint 非空且是非本地 endpoint → OpenAI-compatible 云端/网关。
/// 2. settings.llm.endpoint 指向本地非 scheduler 服务 → 忽略该
///    endpoint，改走 scheduler `:8090/v1`。
/// 3. settings.llm.provider 是 local_scheduler/local 等本地别名，或硬件形态偏好
///    本地 LLM → 走 scheduler `:8090/v1`。
/// 4. 其他笔电 / 服务器 + 无 cloud config → None（chat 返回 503 引导配置）。
///
/// Attune server 不再实例化具体本地 worker；worker 必须由 scheduler 管理。
///
/// 由 `reload_llm` 在 unlock 和 settings 热切时统一调用。
fn build_llm_from_settings(
    settings_json: &Option<serde_json::Value>,
    hardware: &attune_core::platform::HardwareProfile,
) -> Option<Arc<dyn LlmProvider>> {
    let configured_llm = settings_json.as_ref().and_then(|settings| {
        let llm = settings.get("llm")?;
        let endpoint = llm
            .get("endpoint")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let api_key = llm
            .get("api_key")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let model = llm
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let provider = llm
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or("local");

        if let Some(ep) = endpoint.filter(|s| !s.is_empty()) {
            if should_route_local_endpoint_to_scheduler(&ep, provider) {
                let scheduler_base = crate::local_scheduler::base_from_optional_settings(settings_json);
                let model = if model.trim().is_empty() {
                    "llm-chat".to_string()
                } else {
                    model
                };
                tracing::warn!(
                    "LLM: local direct endpoint {ep} is not used; routing provider={provider} through scheduler infer {scheduler_base}"
                );
                let _ = api_key;
                return Some(Arc::new(LocalSchedulerInferLlmProvider::new(
                    &scheduler_base,
                    &model,
                )) as Arc<dyn LlmProvider>);
            }
            tracing::info!("LLM: using configured endpoint {ep}");
            Some(Arc::new(OpenAiLlmProvider::new(&ep, &api_key, &model)) as Arc<dyn LlmProvider>)
        } else if provider_is_local_llm_alias(provider) {
            let scheduler_base = crate::local_scheduler::base_from_optional_settings(settings_json);
            let model = if model.trim().is_empty() {
                "llm-chat".to_string()
            } else {
                model
            };
            tracing::info!(
                "LLM: provider={provider} routed through local scheduler infer {scheduler_base}"
            );
            let _ = api_key;
            Some(Arc::new(LocalSchedulerInferLlmProvider::new(
                &scheduler_base,
                &model,
            )) as Arc<dyn LlmProvider>)
        } else {
            None
        }
    });

    configured_llm.or_else(|| {
        let settings = settings_json
            .as_ref()
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        if hardware.form_factor.prefers_local_llm() || crate::local_scheduler::native_kb_ask_enabled(&settings) {
            let scheduler_base = crate::local_scheduler::base_from_optional_settings(settings_json);
            tracing::info!(
                "LLM (scheduler-native KB): using scheduler infer {scheduler_base}"
            );
            Some(Arc::new(LocalSchedulerInferLlmProvider::new(
                &scheduler_base,
                "llm-chat",
            )) as Arc<dyn LlmProvider>)
        } else {
            tracing::warn!(
                "LLM: form_factor={:?} + no cloud endpoint configured → no LLM (chat 将返回 503 提示用户配置 cloud API key per CLAUDE.md M2)",
                hardware.form_factor
            );
            None
        }
    })
}

/// 按 settings 构建 embedding provider（G4 — embedding endpoint 可配置）。
///
/// 优先级：
/// 1. `settings.embedding.provider == "local_scheduler"` + endpoint 非空 →
///    [`LocalSchedulerEmbeddingProvider`]（scheduler-native `/kb/tasks/...`）。
/// 2. `settings.embedding.endpoint` 非空且是非本地 endpoint → [`OpenAiEmbeddingProvider`]。
///    读 `endpoint` / `api_key` / `model` / `dims`（缺省 model=`bge-m3`、dims=1024，与
///    云端 embedding 对齐）。
/// 3. endpoint 指向本地非 scheduler 服务，或没有 endpoint → scheduler-native
///    `kb.query.embed`。不再在 attune-server 内加载 ORT embedding，也不再直连
///    worker-specific embedding API。
///
/// 镜像 [`build_llm_from_settings`]：endpoint 走 `settings.embedding.*`，与 `settings.llm.*`
/// 同形（provider/endpoint/api_key/model），让本地调度器设备把 embedding 指向本机 scheduler 时零特例。
/// Returns `(provider, is_local)`.
/// `is_local = true` when the provider is scheduler-local.
/// `is_local = false` when a cloud-pointing OpenAI-compat endpoint is configured.
/// The caller publishes `is_local` with the provider through
/// `set_embedding_with_locality`, so egress readers obtain one coherent
/// snapshot without re-reading settings on every batch.
/// #82 security: is an embedding endpoint a LOCAL (loopback / RFC-1918 private)
/// destination? The OutboundGate skips in-network embedding but gates cloud egress,
/// so a wrong answer leaks L0 PII. MUST parse the URL host and match the FULL host —
/// NEVER `starts_with` on the raw endpoint string, which lets these bypass the gate
/// (background security review HIGH, 2026-06-13):
///   - `http://localhost.evil.com/...`  (suffix on the "localhost" prefix)
///   - `http://10.0.0.1@evil.com/...`   (userinfo — real host is evil.com)
///   - `http://172.2.0.0/...`           (PUBLIC, but matched by a "172.2" prefix)
///
/// Uses std `Ipv4Addr::is_private()` (10/8, 172.16-31/12, 192.168/16) + `is_loopback()`
/// (127/8, ::1), which are RFC-1918/RFC-4291 correct.
fn embedding_endpoint_is_local(endpoint: &str) -> bool {
    attune_core::net::destination::is_local_network_url(endpoint)
}

fn embedding_provider_is_scheduler_native(provider: &str) -> bool {
    crate::local_scheduler::provider_is_scheduler_native(provider)
}

fn embedding_default_dims(_provider: &str, model: &str) -> usize {
    if model.eq_ignore_ascii_case("embedding-int8") {
        512
    } else {
        1024
    }
}

fn embedding_index_dims_from_settings(settings_json: &Option<serde_json::Value>) -> usize {
    settings_json
        .as_ref()
        .and_then(|settings| {
            let embedding = settings.get("embedding")?;
            let endpoint = embedding
                .get("endpoint")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if endpoint.is_empty() {
                return None;
            }
            let provider = embedding
                .get("provider")
                .and_then(|v| v.as_str())
                .unwrap_or("openai_compat");
            let model = embedding
                .get("model")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    if embedding_provider_is_scheduler_native(provider)
                        || crate::local_scheduler::endpoint_is_scheduler(endpoint)
                        || should_route_local_endpoint_to_scheduler(endpoint, provider)
                    {
                        "embedding-int8"
                    } else {
                        "bge-m3"
                    }
                });
            Some(
                embedding
                    .get("dims")
                    .and_then(|v| v.as_u64())
                    .map(|d| d as usize)
                    .filter(|d| *d > 0)
                    .unwrap_or_else(|| embedding_default_dims(provider, model)),
            )
        })
        .unwrap_or(512)
}

fn build_embedding_from_settings(
    settings_json: &Option<serde_json::Value>,
) -> (Arc<dyn EmbeddingProvider>, bool) {
    // 1. settings.embedding.provider=local_scheduler + endpoint 非空 → local scheduler KB task.
    // 2. Cloud endpoint → OpenAI-compatible.
    // 3. Local direct endpoint or absent endpoint → local scheduler default.
    let configured = settings_json.as_ref().and_then(|settings| {
        let embedding = settings.get("embedding")?;
        let endpoint = embedding
            .get("endpoint")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())?;
        let provider = embedding
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or("openai_compat");
        let route_to_scheduler =
            embedding_provider_is_scheduler_native(provider)
                || crate::local_scheduler::endpoint_is_scheduler(&endpoint)
                || should_route_local_endpoint_to_scheduler(&endpoint, provider);
        let api_key = embedding.get("api_key").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let model = embedding
            .get("model")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(if route_to_scheduler {
                "embedding-int8"
            } else {
                "bge-m3"
            })
            .to_string();
        let default_dims = embedding_default_dims(provider, &model);
        // dims 默认随 provider/model 走：bge-m3=1024；local scheduler embedding-int8=512。
        // 自定义 endpoint 模型维度不同时显式配。
        let dims = embedding
            .get("dims")
            .and_then(|v| v.as_u64())
            .map(|d| d as usize)
            .filter(|d| *d > 0)
            .unwrap_or(default_dims);
        // #82: determine local_destination for OutboundGate; loopback/RFC1918 = local.
        // MUST parse the host (not `starts_with` on the raw string) — see
        // `embedding_endpoint_is_local` for the bypass classes this closes.
        let is_local = embedding_endpoint_is_local(&endpoint);
        if route_to_scheduler {
            let task = embedding
                .get("task")
                .and_then(|v| v.as_str())
                .unwrap_or("kb.query.embed");
            let poll_timeout_ms = embedding
                .get("poll_timeout_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(60_000);
            tracing::info!(
                "Embedding: local scheduler endpoint {endpoint} (provider={provider}, task={task}, model={model}, dims={dims}, local={is_local})"
            );
            let scheduler_base = if attune_core::net::destination::is_safe_local_scheduler_url(
                &endpoint,
            ) && (crate::local_scheduler::endpoint_is_scheduler(&endpoint)
                || embedding_provider_is_scheduler_native(provider))
            {
                attune_core::edge_cloud::capacity::normalize_scheduler_base(&endpoint)
            } else {
                crate::local_scheduler::base_from_optional_settings(settings_json)
            };
            let scheduler_is_local =
                attune_core::net::destination::is_safe_local_scheduler_url(&scheduler_base);
            return Some((
                Arc::new(LocalSchedulerEmbeddingProvider::new(
                    &scheduler_base,
                    task,
                    &model,
                    dims,
                    poll_timeout_ms,
                )) as Arc<dyn EmbeddingProvider>,
                scheduler_is_local,
            ));
        }

        // 2. settings.embedding.endpoint 非空且非本地 → OpenAI 兼容云端/网关
        tracing::info!("Embedding: OpenAI-compatible endpoint {endpoint} (model={model}, dims={dims}, local={is_local})");
        Some((
            Arc::new(OpenAiEmbeddingProvider::new(&endpoint, &api_key, &model, dims))
                as Arc<dyn EmbeddingProvider>,
            is_local,
        ))
    });
    if let Some((p, is_local)) = configured {
        return (p, is_local);
    }

    let scheduler_base = crate::local_scheduler::base_from_optional_settings(settings_json);
    tracing::info!("Embedding: defaulting to local scheduler endpoint {scheduler_base}");
    (
        Arc::new(LocalSchedulerEmbeddingProvider::new(
            &scheduler_base,
            "kb.query.embed",
            "embedding-int8",
            512,
            60_000,
        )),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_stale_hot_reload_cannot_resurrect_handles_after_vault_lock() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = attune_core::vault::Vault::open_memory(dir.path()).expect("vault");
        vault.setup("P@ss-runtime-lock-race").expect("setup");
        let state = Arc::new(AppState::new(vault, false));

        // Seed every credential/model-bearing runtime so the lock path must
        // clear real handles rather than pass vacuously.
        state.set_llm(Some(Arc::new(attune_core::llm::MockLlmProvider::new(
            "old-cloud-llm",
        ))));
        state.set_embedding_with_locality(
            Some(Arc::new(attune_core::embed::MockEmbeddingProvider::new(4))),
            false,
        );
        state.set_reranker(Some(Arc::new(attune_core::infer::MockRerankProvider::new(
            vec![0.5],
        ))));
        state.reload_plugin_hub(Some("https://old-hub.invalid"), Some("old-license-token"));
        assert_ne!(
            state
                .plugin_hub
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .name(),
            "mock"
        );
        *state.member_state.lock().unwrap_or_else(|e| e.into_inner()) =
            attune_core::member_session::MemberState::Paid {
                account_id: "account-old".into(),
                license_id: "license-old".into(),
                llm_quota_remaining: 10,
            };
        state
            .entitlement_cache
            .upsert(attune_core::store::plugin_entitlements::EntitlementRow {
                plugin_id: "paid-plugin".into(),
                license_id: "license-old".into(),
                decrypt_key: Some("decrypt-secret".into()),
                tier: "paid".into(),
                status: "active".into(),
                trial_expires: None,
                signing_pubkey_hex: "00".into(),
                last_verified_at: "2026-07-15T00:00:00Z".into(),
                grace_started_at: None,
                updated_at: "2026-07-15T00:00:00Z".into(),
            });

        // Model a hot reload that has already decrypted/built candidates but
        // is blocked before publish. This is the precise window in which the
        // old code could install a token-bearing provider after lock cleanup.
        let (stale_generation, stale_credential_generation, stale_plugin_reload_generation) =
            state.begin_credential_reload(&state.plugin_hub_reload_generation);
        let (_, _, stale_llm_reload_generation) =
            state.begin_credential_reload(&state.llm_reload_generation);
        let (_, stale_embedding_reload_generation) =
            state.begin_runtime_reload(&state.embedding_reload_generation);
        let (candidate_ready_tx, candidate_ready_rx) = std::sync::mpsc::sync_channel(0);
        let (resume_tx, resume_rx) = std::sync::mpsc::sync_channel(0);
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(0);
        let reload_state = state.clone();
        let reload = std::thread::spawn(move || {
            let stale_plugin_hub = build_plugin_hub_provider(
                Some("https://stale-hub.invalid"),
                Some("stale-license-token"),
            );
            let stale_llm = Arc::new(attune_core::llm::MockLlmProvider::new("stale-cloud-llm"))
                as Arc<dyn LlmProvider>;
            let stale_embedding = EmbeddingRuntimeCandidate {
                provider: Arc::new(attune_core::embed::MockEmbeddingProvider::new(8)),
                is_local: false,
                reranker: Arc::new(attune_core::infer::MockRerankProvider::new(vec![0.9])),
                vectors: VectorIndex::new(8).ok(),
                memory_index: None,
            };
            candidate_ready_tx.send(()).expect("candidate ready");
            resume_rx.recv().expect("resume stale publish");
            let plugin_installed = reload_state.publish_plugin_hub_if_current(
                stale_generation,
                stale_credential_generation,
                stale_plugin_reload_generation,
                stale_plugin_hub,
            );
            let llm_installed = reload_state.publish_llm_if_current(
                stale_generation,
                stale_credential_generation,
                stale_llm_reload_generation,
                Some(stale_llm),
            );
            let embedding_installed = reload_state.publish_embedding_if_current(
                stale_generation,
                stale_embedding_reload_generation,
                stale_embedding,
            );
            result_tx
                .send((plugin_installed, llm_installed, embedding_installed))
                .expect("publish result");
        });

        candidate_ready_rx
            .recv()
            .expect("candidate build completed");
        state
            .lock_vault_and_clear_runtime()
            .expect("lock must not deadlock while replacing PluginHub");
        resume_tx.send(()).expect("resume stale reload");
        assert_eq!(
            result_rx.recv().expect("publish result"),
            (false, false, false)
        );
        reload.join().expect("reload thread");

        // Public hot-reload entry points must also fail closed while locked.
        state.reload_plugin_hub(
            Some("https://post-lock-hub.invalid"),
            Some("post-lock-token"),
        );
        state.reload_llm();
        state.reload_embedding();

        assert!(matches!(
            state
                .vault
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .state(),
            attune_core::vault::VaultState::Locked
        ));
        assert!(state.llm().is_none(), "LLM handle resurrected after lock");
        assert!(
            state.summary_llm().is_none(),
            "summary LLM clone resurrected after lock"
        );
        assert!(
            state.vlm().is_none(),
            "VLM LLM clone resurrected after lock"
        );
        assert!(
            state.embedding().is_none(),
            "embedding handle resurrected after lock"
        );
        assert!(
            state.reranker().is_none(),
            "reranker resurrected after lock"
        );
        assert_eq!(
            state
                .plugin_hub
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .name(),
            "mock",
            "license-bearing PluginHub resurrected after lock"
        );
        assert!(matches!(
            &*state.member_state.lock().unwrap_or_else(|e| e.into_inner()),
            attune_core::member_session::MemberState::LoggedOut
        ));
        assert!(
            state.entitlement_cache.snapshot().is_empty(),
            "decrypted entitlement cache survived lock"
        );
    }

    #[test]
    fn account_generation_rejects_stale_provider_while_vault_stays_unlocked() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = attune_core::vault::Vault::open_memory(dir.path()).expect("vault");
        vault.setup("P@ss-account-generation").expect("setup");
        let state = Arc::new(AppState::new(vault, false));

        let (runtime_generation, credential_generation, reload_generation) =
            state.begin_credential_reload(&state.plugin_hub_reload_generation);
        state.invalidate_credential_generation();

        assert!(
            !state.publish_plugin_hub_if_current(
                runtime_generation,
                credential_generation,
                reload_generation,
                build_plugin_hub_provider(
                    Some("https://stale-account.invalid"),
                    Some("stale-account-token"),
                ),
            ),
            "an account switch must invalidate candidates even when the vault remains unlocked"
        );
        assert_eq!(
            state
                .plugin_hub
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .name(),
            "mock"
        );
    }

    #[test]
    fn provider_reload_generation_is_last_start_wins() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = attune_core::vault::Vault::open_memory(dir.path()).expect("vault");
        vault.setup("P@ss-provider-generation").expect("setup");
        let state = Arc::new(AppState::new(vault, false));

        let (old_runtime, old_credentials, old_reload) =
            state.begin_credential_reload(&state.plugin_hub_reload_generation);
        let (new_runtime, new_credentials, new_reload) =
            state.begin_credential_reload(&state.plugin_hub_reload_generation);

        assert!(!state.publish_plugin_hub_if_current(
            old_runtime,
            old_credentials,
            old_reload,
            build_plugin_hub_provider(None, None),
        ));
        assert!(state.publish_plugin_hub_if_current(
            new_runtime,
            new_credentials,
            new_reload,
            build_plugin_hub_provider(None, None),
        ));
    }

    #[test]
    fn embedding_provider_and_locality_are_published_as_one_snapshot() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let vault = attune_core::vault::Vault::open_memory(tmp.path()).expect("vault");
        let state = Arc::new(AppState::new(vault, false));
        let start = Arc::new(std::sync::Barrier::new(2));

        let writer_state = state.clone();
        let writer_start = start.clone();
        let writer = std::thread::spawn(move || {
            writer_start.wait();
            for _ in 0..10_000 {
                writer_state.set_embedding_with_locality(
                    Some(Arc::new(attune_core::embed::MockEmbeddingProvider::new(3))),
                    false,
                );
                writer_state.set_embedding_with_locality(
                    Some(Arc::new(attune_core::embed::MockEmbeddingProvider::new(4))),
                    true,
                );
            }
        });

        start.wait();
        for _ in 0..20_000 {
            let (provider, is_local) = state.embedding_with_locality();
            if is_local {
                assert_eq!(
                    provider.expect("local snapshot has provider").dimensions(),
                    4,
                    "a stale cloud provider must never be paired with local=true"
                );
            }
        }
        writer.join().expect("embedding writer");
    }

    #[test]
    fn cloud_embedding_gate_requires_consent_and_returns_redacted_wire_payload() {
        let redactor = Redactor::new();
        let raw = "客户电话 13800138000";

        assert!(matches!(
            enforce_cloud_embedding_payload(false, true, false, &redactor, raw),
            Err(attune_core::outbound_gate::OutboundError::Disabled(
                OutboundKind::Embedding
            ))
        ));

        let wire = enforce_cloud_embedding_payload(true, true, false, &redactor, raw)
            .expect("consented non-L0 payload should pass");
        assert!(!wire.contains("13800138000"));
        assert_ne!(wire, raw, "the provider must receive the gate output");

        assert!(matches!(
            enforce_cloud_embedding_payload(true, true, true, &redactor, raw),
            Err(attune_core::outbound_gate::OutboundError::L0CloudBlocked)
        ));
    }

    #[test]
    fn classifier_gate_keeps_local_input_and_redacts_only_consented_non_l0_cloud_input() {
        let redactor = Redactor::new();
        let title = "联系人 13800138000";
        let content = "邮箱 alice@example.com";

        let local = govern_classification_input(true, false, true, true, &redactor, title, content)
            .expect("local classifier must remain unaffected");
        assert_eq!(local, (title.to_string(), content.to_string()));

        assert!(matches!(
            govern_classification_input(false, false, true, false, &redactor, title, content,),
            Err(attune_core::outbound_gate::OutboundError::Disabled(
                OutboundKind::Llm
            ))
        ));
        assert!(matches!(
            govern_classification_input(false, true, true, true, &redactor, title, content,),
            Err(attune_core::outbound_gate::OutboundError::L0CloudBlocked)
        ));

        let cloud =
            govern_classification_input(false, true, true, false, &redactor, title, content)
                .expect("consented L1 cloud input should pass");
        assert!(!cloud.0.contains("13800138000"));
        assert!(!cloud.1.contains("alice@example.com"));
    }

    #[test]
    fn classify_worker_defaults_off_for_scheduler_native_llm_but_remains_configurable() {
        let scheduler = Some(serde_json::json!({
            "llm": {
                "provider": "local_scheduler",
                "endpoint": "http://127.0.0.1:8090",
                "model": "llm-summary"
            }
        }));
        assert!(
            !classify_worker_auto_enabled_with_override(&scheduler, None),
            "scheduler-native KB ask must not start the generic background classifier by default"
        );

        let explicit_on = Some(serde_json::json!({
            "classification": {"auto_worker_enabled": true},
            "llm": {
                "provider": "local_scheduler",
                "endpoint": "http://127.0.0.1:8090"
            }
        }));
        assert!(
            classify_worker_auto_enabled_with_override(&explicit_on, None),
            "operators may explicitly opt into scheduler-backed classification"
        );

        let cloud = Some(serde_json::json!({
            "llm": {
                "provider": "openai_compat",
                "endpoint": "https://api.openai.com/v1",
                "model": "gpt-test"
            }
        }));
        assert!(
            classify_worker_auto_enabled_with_override(&cloud, None),
            "cloud/BYOK LLM settings retain the previous automatic classifier behavior"
        );
        assert!(
            !classify_worker_auto_enabled_with_override(&cloud, Some(false)),
            "environment override may still disable the worker globally"
        );
        assert!(
            classify_worker_auto_enabled_with_override(&scheduler, Some(true)),
            "environment override may force-enable the worker for targeted validation"
        );
    }

    #[test]
    fn cloud_classifier_keeps_pending_without_consent_finishes_l0_and_redacts_l1() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("vault.db");
        let vault = attune_core::vault::Vault::open(&db, dir.path()).unwrap();
        vault.setup("P@ss-classifier-gate").unwrap();
        let state = AppState::new(vault, false);
        let mock = std::sync::Arc::new(attune_core::llm::MockLlmProvider::new("cloud-mock"));
        *state.classifier.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::sync::Arc::new(
            Classifier::new(std::sync::Arc::new(Taxonomy::default()), mock.clone()),
        ));

        {
            let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
            let dek = vault.dek_db().unwrap();
            let item_id = vault
                .store()
                .insert_item(
                    &dek,
                    "L0 title 13800138000",
                    "private content",
                    None,
                    "note",
                    None,
                    None,
                )
                .unwrap();
            vault
                .store()
                .set_item_privacy_tier(&item_id, attune_core::store::audit::PrivacyTier::L0)
                .unwrap();
            vault.store().enqueue_classify(&item_id, 1).unwrap();
        }

        assert_eq!(state.drain_classify_batch(5).unwrap(), 0);
        {
            let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
            assert_eq!(vault.store().pending_count_by_type("classify").unwrap(), 1);
            vault
                .store()
                .set_meta(
                    attune_core::llm_settings::SETTINGS_META_KEY,
                    &serde_json::to_vec(&serde_json::json!({
                        "privacy": {"llm": true}
                    }))
                    .unwrap(),
                )
                .unwrap();
        }

        assert_eq!(state.drain_classify_batch(5).unwrap(), 0);
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(vault.store().reset_stuck_processing().unwrap(), 0);
        assert_eq!(vault.store().pending_count_by_type("classify").unwrap(), 0);
        assert!(
            mock.last_received_user().is_none(),
            "L0 must never reach cloud"
        );
        drop(vault);

        mock.push_response(r#"{"core":{},"universal":{},"plugin":{}}"#);
        {
            let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
            let dek = vault.dek_db().unwrap();
            let item_id = vault
                .store()
                .insert_item(
                    &dek,
                    "L1 phone 13800138000",
                    "contact alice@example.com",
                    None,
                    "note",
                    None,
                    None,
                )
                .unwrap();
            vault.store().enqueue_classify(&item_id, 1).unwrap();
        }
        assert_eq!(state.drain_classify_batch(5).unwrap(), 1);
        let outbound = mock
            .last_received_user()
            .expect("L1 classification should call cloud after consent");
        assert!(!outbound.contains("13800138000"));
        assert!(!outbound.contains("alice@example.com"));
    }

    #[test]
    fn autonomous_workers_do_not_start_with_cloud_llm() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("vault.db");
        let vault = attune_core::vault::Vault::open(&db, dir.path()).unwrap();
        vault.setup("P@ss-bootstrap-runtime").unwrap();
        let state = std::sync::Arc::new(AppState::new(vault, false));
        state.set_llm(Some(std::sync::Arc::new(
            attune_core::llm::MockLlmProvider::new("cloud-mock"),
        )));

        AppState::start_skill_evolver(state.clone());
        AppState::start_memory_consolidator(state.clone());

        assert!(!state.evolve_worker_running.load(Ordering::SeqCst));
        assert!(!state.memory_consolidator_running.load(Ordering::SeqCst));
    }

    #[test]
    fn webdav_sync_worker_flag_prevents_double_start() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let flag = AtomicBool::new(false);
        // 首次 compare_exchange 成功。
        assert!(flag
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok());
        // 二次失败 —— worker 不会重复起。
        assert!(flag
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err());
    }

    /// Plan A1 Task L — AppState must expose `cache_backend()` (Some after `new`
    /// because the in-memory L1 needs no vault DEK) and `usage()` (None initially;
    /// set by `set_usage` once a vault-bound aggregator has been built). The
    /// accessor signatures here are what Plan A2's router will consume.
    #[test]
    fn appstate_exposes_cache_backend_and_usage_accessors() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("vault.db");
        let vault = attune_core::vault::Vault::open(&db, dir.path()).unwrap();
        let state = AppState::new(vault, false);
        assert!(
            state.cache_backend().is_some(),
            "in-memory L1 cache backend must be installed at startup"
        );
        assert!(
            state.usage().is_none(),
            "usage aggregator stays None until set_usage is called post-vault-unlock"
        );
        // set_usage is None-tolerant (no-op when arg is None).
        state.set_usage(None);
        assert!(
            state.usage().is_none(),
            "set_usage(None) leaves aggregator None"
        );
    }

    #[test]
    fn model_bootstrap_reinstalls_scheduler_handles_after_clear() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("vault.db");
        let vault = attune_core::vault::Vault::open(&db, dir.path()).unwrap();
        vault.setup("P@ss-bootstrap-reinstall").unwrap();
        let state = std::sync::Arc::new(AppState::new(vault, false));

        for class in attune_core::infer::bootstrap_status::MODEL_CLASSES {
            state.model_bootstrap.mark_ready(class);
        }
        assert!(state.model_bootstrap.all_ready());
        assert!(state.embedding().is_none());

        AppState::spawn_model_bootstrap(state.clone());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while (state.embedding().is_none()
            || state
                .reranker
                .lock()
                .ok()
                .map(|g| g.is_none())
                .unwrap_or(true))
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert!(
            state.embedding().is_some(),
            "ready bootstrap state must not prevent provider handle reinstall"
        );
        assert!(
            state
                .reranker
                .lock()
                .ok()
                .map(|g| g.is_some())
                .unwrap_or(false),
            "reranker handle should be reinstalled with embedding"
        );
    }

    /// G4 — cloud settings.embedding.endpoint drives the embedding provider to an
    /// OpenAI-compatible endpoint (1536 dims here proves the configured dims are read).
    /// We assert dimensions rather than network behaviour (covered by embed.rs unit test).
    #[test]
    fn embedding_cloud_endpoint_selects_openai_compat() {
        let settings = Some(serde_json::json!({
            "embedding": {
                "endpoint": "https://api.openai.com/v1",
                "api_key": "sk-x",
                "model": "text-embedding-3-small",
                "dims": 1536
            }
        }));
        let (provider, is_local) = build_embedding_from_settings(&settings);
        // OpenAiEmbeddingProvider reports the configured dims.
        assert_eq!(
            provider.dimensions(),
            1536,
            "configured embedding endpoint + dims must route to OpenAI-compatible provider"
        );
        assert!(
            !is_local,
            "cloud endpoint must be classified as non-local for OutboundGate"
        );
    }

    /// G4 — empty/absent embedding settings must NOT pick OpenAI or a direct local worker.
    /// It defaults to scheduler-native embedding-int8.
    #[test]
    fn embedding_settings_empty_endpoint_defaults_to_scheduler() {
        let settings = Some(serde_json::json!({ "embedding": { "endpoint": "" } }));
        let (provider, is_local) = build_embedding_from_settings(&settings);
        assert_eq!(
            provider.dimensions(),
            512,
            "empty endpoint must route to scheduler embedding-int8"
        );
        assert!(is_local, "scheduler fallback must be classified as local");
    }

    #[test]
    fn scheduler_native_embedding_does_not_enable_scheduler_llm_fallback_on_laptop() {
        let mut hardware = attune_core::platform::HardwareProfile::default();
        hardware.form_factor = attune_core::platform::FormFactor::Laptop;
        let settings = Some(serde_json::json!({
            "embedding": {
                "provider": "local_scheduler",
                "endpoint": "http://127.0.0.1:8090",
                "model": "embedding-int8"
            }
        }));
        assert!(
            build_llm_from_settings(&settings, &hardware).is_none(),
            "local embedding must not silently select scheduler chat"
        );
    }

    #[test]
    fn explicit_cloud_llm_endpoint_wins_over_scheduler_native_embedding() {
        let mut hardware = attune_core::platform::HardwareProfile::default();
        hardware.form_factor = attune_core::platform::FormFactor::Laptop;
        let settings = Some(serde_json::json!({
            "llm": {
                "provider": "openai_compat",
                "endpoint": "https://api.openai.com/v1",
                "model": "gpt-test"
            },
            "embedding": {
                "provider": "local_scheduler",
                "endpoint": "http://127.0.0.1:8090",
                "model": "embedding-int8"
            }
        }));
        let llm = build_llm_from_settings(&settings, &hardware)
            .expect("explicit cloud LLM endpoint should still build");
        assert!(!llm.is_local());
    }

    #[test]
    fn local_scheduler_embedding_int8_defaults_to_512_dims() {
        let settings = Some(serde_json::json!({
            "embedding": {
                "provider": "local_scheduler",
                "endpoint": "http://127.0.0.1:8090",
                "model": "embedding-int8"
            }
        }));
        let (provider, is_local) = build_embedding_from_settings(&settings);
        assert_eq!(provider.dimensions(), 512);
        assert_eq!(embedding_index_dims_from_settings(&settings), 512);
        assert!(is_local, "loopback local scheduler endpoint is local");
    }

    #[test]
    fn embedding_local_direct_endpoint_is_routed_to_scheduler() {
        let settings = Some(serde_json::json!({
            "embedding": {
                "provider": "openai_compat",
                "endpoint": "http://localhost:18080/v1",
                "model": "embedding-int8"
            }
        }));
        let (provider, is_local) = build_embedding_from_settings(&settings);
        assert_eq!(
            provider.dimensions(),
            512,
            "local direct embedding endpoints must be replaced by scheduler"
        );
        assert_eq!(embedding_index_dims_from_settings(&settings), 512);
        assert!(is_local);
    }

    /// #82 P0 adversarial: cloud endpoint classified as non-local so OutboundGate will
    /// apply the L0 check on cloud-bound embedding calls.
    #[test]
    fn embedding_cloud_endpoint_is_not_local() {
        let settings = Some(serde_json::json!({
            "embedding": {
                "endpoint": "https://api.openai.com/v1",
                "api_key": "sk-test",
                "model": "text-embedding-3-small",
                "dims": 1536
            }
        }));
        let (_provider, is_local) = build_embedding_from_settings(&settings);
        assert!(!is_local, "cloud endpoint (api.openai.com) must not be classified as local — #82 gate requires is_local=false");
    }

    /// #82 security regression (background review HIGH 2026-06-13): the local
    /// classifier MUST parse the host, not `starts_with` the raw URL. Each of these
    /// would bypass the privacy gate under the old prefix match → leak L0 PII.
    #[test]
    fn embedding_endpoint_is_local_anchored_no_bypass() {
        // genuinely local → true
        for ep in [
            "http://localhost:18080",
            "http://127.0.0.1:18080",
            "http://192.168.1.50:8090",
            "http://10.0.0.5/v1",
            "http://172.16.0.1/v1",
            "http://172.31.255.254/v1",
            "http://[::1]:18080",
        ] {
            assert!(embedding_endpoint_is_local(ep), "{ep} must be local");
        }
        // bypass attempts + genuinely-public → false (gated)
        for ep in [
            "http://localhost.evil.com/v1",   // suffix on "localhost" prefix
            "http://127.0.0.1.evil.com/v1",   // suffix on "127." prefix
            "http://192.168.1.1@evil.com/v1", // userinfo — real host is evil.com
            "http://10.0.0.1@evil.com/v1",    // userinfo
            "http://172.2.0.0/v1",            // PUBLIC, but matched old "172.2" prefix
            "http://172.32.0.1/v1",           // PUBLIC, just outside RFC1918 172.16-31
            "https://api.openai.com/v1",      // cloud
            "http://11.0.0.1/v1",             // public (not 10/8)
        ] {
            assert!(
                !embedding_endpoint_is_local(ep),
                "{ep} must NOT be local (would bypass privacy gate)"
            );
        }
    }
}
