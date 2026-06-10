use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use attune_core::classifier::Classifier;
use attune_core::clusterer::ClusterSnapshot;
use attune_core::embed::{EmbeddingProvider, OllamaProvider};
use attune_core::index::FulltextIndex;
use attune_core::llm::{LlmProvider, OllamaLlmProvider, OpenAiLlmProvider};
use attune_core::resource_governor::{global_registry, TaskKind};
use attune_core::tag_index::TagIndex;
use attune_core::taxonomy::Taxonomy;
use attune_core::vault::Vault;
use attune_core::vectors::VectorIndex;
use attune_core::vlm::{LlmVlmProvider, VlmProvider};
use attune_core::web_search::WebSearchProvider;

const SEARCH_CACHE_CAPACITY: usize = 256;
const SEARCH_CACHE_TTL_SECS: u64 = 30;

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
    pub llm: Mutex<Option<Arc<dyn LlmProvider>>>,
    pub summary_llm: Mutex<Option<Arc<dyn LlmProvider>>>,
    pub web_search: Mutex<Option<Arc<dyn WebSearchProvider>>>,
    /// VLM provider — 图片 caption / VQA。由 init_search_engines 用主 LLM 构造；
    /// 无 vision-capable LLM 时为 None（caption 静默跳过）。
    pub vlm: Mutex<Option<Arc<dyn VlmProvider>>>,
    pub tag_index: Mutex<Option<TagIndex>>,
    pub cluster_snapshot: Mutex<Option<ClusterSnapshot>>,
    pub taxonomy: Mutex<Option<Arc<Taxonomy>>>,
    pub classifier: Mutex<Option<Arc<Classifier>>>,
    pub require_auth: bool,
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
    /// WebDAV 周期同步 worker 是否在运行（防重复启动）。
    pub webdav_sync_worker_running: AtomicBool,
    /// Email 周期同步 worker 运行标志（防重入）。
    pub email_sync_worker_running: AtomicBool,
    /// RSS 周期同步 worker 运行标志（防重入）。
    pub rss_sync_worker_running: AtomicBool,
    pub search_cache: Mutex<LruCache<u64, CachedSearch>>,
    /// Office helper async job registry (v0.7.1) — in-memory ASR transcription
    /// jobs. Not persisted; restart cancels all in-flight. See
    /// `attune_core::office_job_queue` + `docs/superpowers/specs/2026-05-20-office-helper-design.md` §1.
    pub office_jobs: std::sync::Arc<attune_core::office_job_queue::JobRegistry>,
    /// Sprint 1 Phase B: project recommendation broadcast channel.
    /// upload.rs / chat.rs 收到信号后 send；ws.rs subscribe 推送给前端。
    pub recommendation_tx: tokio::sync::broadcast::Sender<serde_json::Value>,
    /// Sprint 2: 启动时加载的 plugins（attune-pro / 用户 / 社区）
    pub plugin_registry: std::sync::Arc<attune_core::plugin_registry::PluginRegistry>,
    /// 会员登录状态 — 控制 SettingsLocks 灰显 / 锁定 PATCH /settings 字段.
    /// 默认 LoggedOut (本地 self-host). login 后由 cloud_client.me() 推导.
    pub member_state: Mutex<attune_core::member_session::MemberState>,
    /// C1 paywall-bypass fix: server-side verifier for a "paid" claim. `login_token` MUST run
    /// this before granting `MemberState::Paid` so a forged `{tier:paid, license_id:..}` cannot
    /// reach a billable tier-3 op. Default = `CloudMemberVerifier` (verifies against the cloud
    /// session, fail-closed). Tests inject a verifier that performs a real (offline) match.
    pub member_verifier:
        Mutex<std::sync::Arc<dyn attune_core::member_verifier::MemberVerifier>>,
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
}

impl AppState {
    pub fn new(vault: Vault, require_auth: bool) -> Self {
        let (recommendation_tx, _rx) = tokio::sync::broadcast::channel::<serde_json::Value>(64);
        // 2026-05-20: 启动时 LicenseCache::load 的 paid-plugin 解密 key fallback 是死路径.
        // 历史 cloud_client.list_licenses() 下发的 license_key 是 Bearer token, 不是
        // SignedLicense code — attune-cli 已经跳过写 LicenseCache (see main.rs:784-786);
        // 这里读出来也永远是 None. 直接走明文 scan; encrypted plugin 走 plugin_sync 路径
        // (它从 cloud_client.EntitledPlugin.decrypt_key 直接拿 key, 不经此 cache).
        let cached_license_key: Option<Vec<u8>> = None;
        let plugin_registry = match attune_core::plugin_registry::PluginRegistry::default_plugins_dir() {
            Ok(dir) => match attune_core::plugin_registry::PluginRegistry::scan_with_key(&dir, cached_license_key.as_deref()) {
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
        Self {
            vault: Mutex::new(vault),
            fulltext: Mutex::new(None),
            vectors: Mutex::new(None),
            memory_index: Mutex::new(None),
            embedding: Mutex::new(None),
            reranker: Mutex::new(None),
            llm: Mutex::new(None),
            summary_llm: Mutex::new(None),
            web_search: Mutex::new(None),
            vlm: Mutex::new(None),
            tag_index: Mutex::new(None),
            cluster_snapshot: Mutex::new(None),
            taxonomy: Mutex::new(None),
            classifier: Mutex::new(None),
            require_auth,
            queue_worker_running: AtomicBool::new(false),
            classify_worker_running: AtomicBool::new(false),
            rescan_worker_running: AtomicBool::new(false),
            evolve_worker_running: AtomicBool::new(false),
            memory_consolidator_running: AtomicBool::new(false),
            reindex_worker_running: AtomicBool::new(false),
            webdav_sync_worker_running: AtomicBool::new(false),
            email_sync_worker_running: AtomicBool::new(false),
            rss_sync_worker_running: AtomicBool::new(false),
            engines_initialized: AtomicBool::new(false),
            search_cache: Mutex::new(LruCache::new(
                NonZeroUsize::new(SEARCH_CACHE_CAPACITY).expect("SEARCH_CACHE_CAPACITY is non-zero const")
            )),
            office_jobs: attune_core::office_job_queue::JobRegistry::new(),
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
            // C1: default verifier proves paid claims against the cloud session (fail-closed).
            member_verifier: Mutex::new(std::sync::Arc::new(
                attune_core::member_verifier::CloudMemberVerifier::default(),
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
                    tracing::info!("ACP-5: no agent flows loaded ({e}); chat uses free-form RAG only");
                    None
                }
            },
        }
    }

    /// G2 (2026-05-01) — 按 settings 切换 PluginHub provider
    /// 由 PATCH /api/v1/settings 在 pluginhub 字段变化时调
    pub fn reload_plugin_hub(&self, url: Option<&str>, license_key: Option<&str>) {
        let new_provider: std::sync::Arc<dyn attune_core::plugin_hub::PluginHubProvider> =
            match (url, license_key) {
                (Some(u), Some(k)) if !u.is_empty() && !k.is_empty() => {
                    tracing::info!("plugin_hub: switching to HttpPluginHubProvider @ {u}");
                    std::sync::Arc::new(attune_core::plugin_hub::HttpPluginHubProvider::new(u, k))
                }
                _ => {
                    tracing::info!("plugin_hub: using MockPluginHubProvider (no url/license configured)");
                    std::sync::Arc::new(attune_core::plugin_hub::MockPluginHubProvider::default())
                }
            };
        if let Ok(mut guard) = self.plugin_hub.lock() {
            *guard = new_provider;
        }
    }

    /// 仅重建 state.llm + classifier，按当前 settings 重新选 provider。
    /// 用于 wizard / Settings PATCH 修改 llm.* 字段后热切，避免要求重启。
    /// 由 settings.rs 在 body.get("llm").is_some() 时调用。
    pub fn reload_llm(&self) {
        let settings_json = {
            let vault_guard = self.vault.lock().unwrap_or_else(|e| e.into_inner());
            vault_guard.store().get_meta("app_settings").ok().flatten()
                .and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok())
        };
        let llm_result = build_llm_from_settings(&settings_json, &self.hardware);
        match llm_result {
            Some(llm_arc) => {
                tracing::info!("LLM hot-reload: provider rebuilt from settings");
                // 同时刷新 classifier (它持有 llm Arc 复本，需要更新)
                if let Some(tax_arc) = self.taxonomy.lock().unwrap_or_else(|e| e.into_inner()).clone() {
                    *self.classifier.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(std::sync::Arc::new(Classifier::new(tax_arc, llm_arc.clone())));
                }
                // VLM 同步热切（依赖主 LLM，LLM 换了 VLM 也要跟着换）
                *self.vlm.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(Arc::new(LlmVlmProvider::new(llm_arc.clone())) as Arc<dyn VlmProvider>);
                *self.llm.lock().unwrap_or_else(|e| e.into_inner()) = Some(llm_arc);
            }
            None => {
                tracing::warn!("LLM hot-reload: settings yielded no LLM provider — chat will be disabled");
                // 先清依赖 LLM 的 vlm / classifier，再清 llm —— LLM 禁用后二者立即不可用
                *self.vlm.lock().unwrap_or_else(|e| e.into_inner()) = None;
                *self.classifier.lock().unwrap_or_else(|e| e.into_inner()) = None;
                *self.llm.lock().unwrap_or_else(|e| e.into_inner()) = None;
            }
        }
    }

    /// 初始化搜索引擎 + 分类引擎 (unlock 后调用)
    /// 使用 compare_exchange 保证幂等：并发 unlock 只有第一个线程真正执行初始化。
    pub fn init_search_engines(&self) {
        if self.engines_initialized
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return; // 已初始化，跳过
        }

        // v0.6.0-rc.4: 按 region 自动设 HF_ENDPOINT，让 ONNX 模型从国内镜像下载
        // hf-hub crate 读 HF_ENDPOINT 环境变量；未设走默认 huggingface.co
        if std::env::var_os("HF_ENDPOINT").is_none() {
            let region = attune_core::platform::detect_region();
            let endpoint = region.hf_endpoint();
            // SAFETY: 启动时一次性设置（init_search_engines 由 compare_exchange 保证幂等）
            // 不会有并发 set_var 竞争。
            #[allow(unsafe_code)]
            unsafe { std::env::set_var("HF_ENDPOINT", endpoint) };
            tracing::info!("Region detected: {} → HF_ENDPOINT={endpoint}", region.label());
        }
        // Fulltext index (persistent on disk)
        {
            let tantivy_dir = attune_core::platform::data_dir().join("tantivy");
            if let Ok(ft) = FulltextIndex::open(&tantivy_dir) {
                // Rebuild fulltext index from all items (ensures consistency after unlock)
                {
                    let vault_guard = self.vault.lock().unwrap_or_else(|e| e.into_inner());
                    if let Ok(dek) = vault_guard.dek_db() {
                        if let Ok(ids) = vault_guard.store().list_all_item_ids() {
                            for id in &ids {
                                if let Ok(Some(item)) = vault_guard.store().get_item(&dek, id) {
                                    let _ = ft.add_document(&item.id, &item.title, &item.content, &item.source_type);
                                }
                            }
                        }
                    }
                }
                *self.fulltext.lock().unwrap_or_else(|e| e.into_inner()) = Some(ft);
            }
        }

        // Vector index (1024 dims for bge-m3)。
        //
        // 持久化策略：
        //   优先从 ~/.local/share/attune/vectors.encbin 加密加载；不存在或损坏
        //   降级为空 HNSW。写入在 start_queue_worker 批次结束时 flush（每 20 次 or
        //   每 10 分钟取近者），clear_search_engines 锁定前再 flush 一次。
        // 锁序：先取 vault（拿 dek）再取 vectors —— 与文档化全局序
        // vault → vectors → fulltext → embedding 一致，杜绝 vectors→vault 反序持锁。
        let vectors_path = attune_core::platform::data_dir().join("vectors.encbin");
        let dek_opt = self
            .vault
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .dek_db()
            .ok();
        if let Ok(mut guard) = self.vectors.lock() {
            *guard = match dek_opt {
                Some(dek) if vectors_path.exists() => {
                    match VectorIndex::load_encrypted(&dek, &vectors_path, 1024) {
                        Ok(vi) => {
                            tracing::info!("Vector index loaded from {} ({} entries)",
                                vectors_path.display(), vi.len());
                            Some(vi)
                        }
                        Err(e) => {
                            tracing::warn!("Vector index load failed ({e}); starting empty");
                            VectorIndex::new(1024).ok()
                        }
                    }
                }
                _ => VectorIndex::new(1024).ok(),
            };
        }

        // Embedding 提供者选择：
        // - 默认 ONNX (Xenova/bge-m3 quantized, CPU) — 自包含、零外部依赖
        // - ATTUNE_EMBEDDING_BACKEND=ollama 强制走 Ollama bge-m3 (full precision, GPU 可用)
        //   benchmark / Pro 部署用，质量更好但需要 Ollama 运行
        if let Ok(mut guard) = self.embedding.lock() {
            let prefer_ollama = std::env::var("ATTUNE_EMBEDDING_BACKEND")
                .map(|v| v.eq_ignore_ascii_case("ollama"))
                .unwrap_or(false);

            let provider: Arc<dyn EmbeddingProvider> = if prefer_ollama {
                tracing::info!("Embedding: Ollama bge-m3 (ATTUNE_EMBEDDING_BACKEND=ollama)");
                Arc::new(OllamaProvider::default())
            } else {
                match attune_core::infer::embedding::OrtEmbeddingProvider::qwen3_embedding_0_6b() {
                    Ok(p) => {
                        tracing::info!("Embedding: OrtEmbeddingProvider (Xenova/bge-m3 ONNX quantized)");
                        Arc::new(p)
                    }
                    Err(e) => {
                        tracing::info!("ONNX embedding unavailable ({e}), falling back to Ollama bge-m3");
                        Arc::new(OllamaProvider::default())
                    }
                }
            };
            *guard = Some(provider);
        }

        // Multi-layer memory (2026-05-18): build the memory vector index from the
        // memory_vectors sidecar. Dimension = active embedding provider's; rows from
        // a different model graceful-skip inside build_from_store.
        {
            let dims = self
                .embedding
                .lock()
                .ok()
                .and_then(|g| g.as_ref().map(|p| p.dimensions()))
                .filter(|d| *d > 0)
                .unwrap_or(1024);
            let built = {
                let vault = self.vault.lock().unwrap_or_else(|e| e.into_inner());
                attune_core::memory::MemoryVectorIndex::build_from_store(vault.store(), dims)
            };
            match built {
                Ok(idx) => {
                    tracing::info!("Memory vector index loaded ({} memories)", idx.len());
                    if let Ok(mut g) = self.memory_index.lock() {
                        *g = Some(idx);
                    }
                }
                Err(e) => tracing::warn!("Memory vector index build failed ({e}); tiered assembler disabled until rebuilt"),
            }
        }

        // Try loading OrtRerankProvider
        if let Ok(mut guard) = self.reranker.lock() {
            match attune_core::infer::reranker::OrtRerankProvider::bge_reranker_v2_m3() {
                Ok(r) => {
                    tracing::info!("Reranker: OrtRerankProvider (bge-reranker-v2-m3)");
                    *guard = Some(Arc::new(r));
                }
                Err(e) => {
                    tracing::info!("Reranker unavailable ({e}), will use vector cosine fallback");
                }
            }
        }

        // v0.6.0-rc.4: 按 tier 后台拉取 whisper ggml 模型（不阻塞启动）
        // 失败仅 warn — 用户上传音频时若 detect_asr_backend 仍返 None 会在 ai_stack
        // status note 给出再次下载提示。
        let tier = attune_core::platform::classify_hardware(&self.hardware);
        if tier.is_supported() {
            std::thread::spawn(move || {
                match attune_core::asr::fetch_for_tier(tier) {
                    Ok(path) => {
                        tracing::info!(
                            "ASR ggml ready at {} (tier={})",
                            path.display(),
                            tier.label()
                        );
                    }
                    Err(e) => {
                        tracing::warn!("ASR ggml auto-fetch failed (tier={}): {e}", tier.label());
                    }
                }
            });
        }

        // LLM 四级优先级见 build_llm_from_settings 文档
        let settings_json = {
            let vault_guard = self.vault.lock().unwrap_or_else(|e| e.into_inner());
            vault_guard.store().get_meta("app_settings").ok().flatten()
                .and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok())
        };

        let llm_result = build_llm_from_settings(&settings_json, &self.hardware);

        let summary_llm_result: Option<Arc<dyn LlmProvider>> = {
            let summary_model = settings_json.as_ref()
                .and_then(|settings| settings.get("summary_model").and_then(|v| v.as_str()))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| self.hardware.recommended_summary_model())
                .to_string();

            let preferred_models = [summary_model.as_str(), "qwen2.5:7b", "qwen2.5:3b", "qwen2.5:1.5b", "llama3.2:1b"];
            match OllamaLlmProvider::auto_detect_with_preferred(&preferred_models) {
                Ok(llm) => {
                    tracing::info!("Summary LLM: using Ollama auto-detect with preferred model {summary_model}");
                    Some(Arc::new(llm) as Arc<dyn LlmProvider>)
                }
                Err(e) => {
                    tracing::warn!("Summary LLM unavailable ({summary_model}): {e}");
                    None
                }
            }
        };

        if let Some(llm_arc) = llm_result {
            let mut tax = Taxonomy::default();
            if let Ok(plugins) = Taxonomy::load_builtin_plugins() {
                for p in plugins {
                    tax = tax.with_plugin(p);
                }
            }
            // Load user plugins from config_dir/plugins/*.yaml
            let (user_plugins, _errors) = Taxonomy::load_user_plugins(&attune_core::platform::config_dir());
            for p in user_plugins {
                tax = tax.with_plugin(p);
            }
            let tax_arc = Arc::new(tax);

            *self.classifier.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(Arc::new(Classifier::new(tax_arc.clone(), llm_arc.clone())));
            *self.taxonomy.lock().unwrap_or_else(|e| e.into_inner()) = Some(tax_arc);
            *self.llm.lock().unwrap_or_else(|e| e.into_inner()) = Some(llm_arc);
        }

        if let Some(summary_llm_arc) = summary_llm_result {
            *self.summary_llm.lock().unwrap_or_else(|e| e.into_inner()) = Some(summary_llm_arc);
        }

        // VLM：用主 LLM 构造薄适配器（vision-capable model 可直接处理图片）
        {
            let llm_opt = self.llm.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(llm_arc) = llm_opt {
                let vlm: Arc<dyn VlmProvider> = Arc::new(LlmVlmProvider::new(llm_arc));
                *self.vlm.lock().unwrap_or_else(|e| e.into_inner()) = Some(vlm);
                tracing::info!("VLM: LlmVlmProvider initialized (backed by main LLM)");
            }
        }

        // Web search provider（从 app_settings.web_search 加载；缺省时尝试默认）
        {
            let settings_json = {
                let vault_guard = self.vault.lock().unwrap_or_else(|e| e.into_inner());
                vault_guard.store().get_meta("app_settings").ok().flatten()
                    .and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok())
                    .unwrap_or_else(|| serde_json::json!({}))
            };
            let ws_provider = attune_core::web_search::from_settings(&settings_json);
            match ws_provider {
                Some(ws) => {
                    tracing::info!("Web search: {} provider enabled", ws.provider_name());
                    *self.web_search.lock().unwrap_or_else(|e| e.into_inner()) = Some(ws);
                }
                None => {
                    // 诊断：区分 disabled vs 无浏览器 vs 无效路径
                    let disabled = settings_json.get("web_search")
                        .and_then(|w| w.get("enabled"))
                        .and_then(|v| v.as_bool()) == Some(false);
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
        *self.tag_index.lock().unwrap_or_else(|e| e.into_inner()) = tag_index_result;
    }

    /// 手动处理一批 classify 任务（供 /classify/drain 端点调用）
    ///
    /// 从 embed_queue 中取出一批 pending 任务，过滤出 task_type == "classify" 的条目，
    /// 调用 classifier.classify_batch 批量分类，写回 items.tags 和 TagIndex，
    /// 最后标记任务为 done。非 classify 的任务会被重新标记为 pending。
    pub fn drain_classify_batch(&self, batch_size: usize) -> attune_core::error::Result<usize> {
        // 1. 检查 classifier 是否可用
        let classifier = match self.classifier.lock().unwrap_or_else(|e| e.into_inner()).as_ref().cloned() {
            Some(c) => c,
            None => return Ok(0),
        };

        // 2. Dequeue 一批任务并按 task_type 分区
        let (classify_tasks, dek) = {
            let vault = self.vault.lock().unwrap_or_else(|e| e.into_inner());
            let dek = vault.dek_db()?;
            let tasks = vault.store().dequeue_embeddings(batch_size)?;
            let (classify, other): (Vec<_>, Vec<_>) = tasks
                .into_iter()
                .partition(|t| t.task_type == "classify");
            // 非 classify 任务回到 pending 留给 QueueWorker 处理
            for task in &other {
                vault.store().mark_task_pending(task.id)?;
            }
            (classify, dek)
        };

        if classify_tasks.is_empty() {
            return Ok(0);
        }

        // 3. 获取任务对应 item 的 (title, content)
        let items_info: Vec<(String, String, String, i64)> = {
            let vault = self.vault.lock().unwrap_or_else(|e| e.into_inner());
            classify_tasks
                .iter()
                .filter_map(|t| match vault.store().get_item(&dek, &t.item_id) {
                    Ok(Some(item)) => {
                        Some((t.item_id.clone(), item.title, item.content, t.id))
                    }
                    _ => None,
                })
                .collect()
        };

        if items_info.is_empty() {
            return Ok(0);
        }

        // 4. 批量分类（阻塞调用 LLM，可能较慢）
        let classifier_inputs: Vec<(String, String)> = items_info
            .iter()
            .map(|(_, title, content, _)| (title.clone(), content.clone()))
            .collect();

        let results = match classifier.classify_batch(&classifier_inputs) {
            Ok(r) => r,
            Err(e) => {
                // 失败时标记所有任务为 failed（会根据 attempts 决定重试或 abandon）
                let vault = self.vault.lock().unwrap_or_else(|e| e.into_inner());
                for task in &classify_tasks {
                    let _ = vault.store().mark_embedding_failed(task.id, 3);
                }
                return Err(e);
            }
        };

        // 5. 写回 tags + TagIndex + 标记完成
        let mut processed = 0;
        for (i, (item_id, _, _, task_id)) in items_info.iter().enumerate() {
            if i >= results.len() {
                break;
            }
            let result = &results[i];
            let tags_json = serde_json::to_string(result)?;

            {
                let vault = self.vault.lock().unwrap_or_else(|e| e.into_inner());
                vault.store().update_tags(&dek, item_id, &tags_json)?;
                vault.store().mark_embedding_done(*task_id)?;
            }

            if let Some(index) = self.tag_index.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                index.upsert(item_id, result);
            }
            processed += 1;
        }

        Ok(processed)
    }

    /// 启动后台分类 worker（需要在 init_search_engines 之后调用）
    /// 使用 AtomicBool 防止重复启动；vault lock 时自动退出并重置标志。
    pub fn start_classify_worker(state: std::sync::Arc<AppState>) {
        if state.classifier.lock().unwrap_or_else(|e| e.into_inner()).is_none() {
            return; // No classifier, no worker
        }

        if state.classify_worker_running.compare_exchange(
            false, true, Ordering::SeqCst, Ordering::SeqCst,
        ).is_err() {
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
        if state.reindex_worker_running.compare_exchange(
            false, true, Ordering::SeqCst, Ordering::SeqCst,
        ).is_err() {
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
                    enum WorkerErr { Transient(String), Task(String) }
                    let result: Result<(), WorkerErr> = (|| {
                        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                        let dek = vault.dek_db().map_err(|e| WorkerErr::Transient(format!("dek_db: {e}")))?;
                        let mut vectors_g = state.vectors.lock().unwrap_or_else(|e| e.into_inner());
                        let fulltext_g = state.fulltext.lock().unwrap_or_else(|e| e.into_inner());
                        let (Some(vectors), Some(fulltext)) = (vectors_g.as_mut(), fulltext_g.as_ref()) else {
                            return Err(WorkerErr::Transient("vectors/fulltext not initialized".into()));
                        };
                        match action.as_str() {
                            "purge" => {
                                attune_core::reindex::purge_item_indexes(vault.store(), vectors, fulltext, &item_id)
                                    .map(|_| ())
                                    .map_err(|e| WorkerErr::Task(e.to_string()))
                            }
                            // 'reindex' action 实现
                            "reindex" => {
                                let item = vault.store().get_item(&dek, &item_id)
                                    .map_err(|e| WorkerErr::Task(e.to_string()))?
                                    .ok_or_else(|| WorkerErr::Task(format!("item {item_id} not found for reindex")))?;
                                attune_core::reindex::reindex_item(
                                    vault.store(), vectors, fulltext, &item_id,
                                    &item.title, &item.content, &item.source_type,
                                )
                                .map(|_| ())
                                .map_err(|e| WorkerErr::Task(e.to_string()))
                            }
                            other => Err(WorkerErr::Task(format!("unknown reindex action: {other}"))),
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
                    tracing::info!("WebDAV sync: scanning dir={} url={}", remote.dir_id, remote.url);
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
                        tracing::warn!(
                            "Email sync for account {} failed: {e}",
                            account.dir_id
                        );
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
                                let elapsed = now
                                    .signed_duration_since(prev.with_timezone(&chrono::Utc));
                                elapsed >= chrono::Duration::minutes(
                                    feed.poll_interval_minutes as i64,
                                )
                            }
                            Err(_) => true,
                        },
                    };
                    if !due {
                        continue;
                    }
                    // 只打印 feed_id + name（不含 URL，URL 解密后仅在此函数内消费）。
                    tracing::info!(
                        "RSS sync: polling feed id={} name={}",
                        feed.id,
                        feed.name
                    );
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

    /// 启动后台目录重扫 worker（每 30 分钟扫描一次绑定目录）
    /// 使用 AtomicBool 防止重复启动；vault lock 时自动退出并重置标志。
    pub fn start_rescan_worker(state: std::sync::Arc<AppState>) {
        if state.rescan_worker_running.compare_exchange(
            false, true, Ordering::SeqCst, Ordering::SeqCst,
        ).is_err() {
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

                    let file_types: Vec<String> = dir
                        .file_types
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();

                    // NOTE: 持锁执行 scan_directory —— 每个目录典型 <5s（文件 hash 增量 diff）。
                    // 对比 skill_evolver 的 LLM 调用（15s+，已拆三阶段），此处仍在可接受
                    // 范围内，不拆解。如未来扫描变慢（大目录 / 慢 HDD），可把文件遍历放锁
                    // 外，仅 DB 写操作持锁。
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    match attune_core::scanner::scan_directory(
                        vault.store(),
                        &dek,
                        &dir.id,
                        path,
                        dir.recursive,
                        &file_types,
                    ) {
                        Ok(r) => {
                            if r.new_files > 0 || r.updated_files > 0 {
                                tracing::info!(
                                    "Rescan {}: {} new, {} updated",
                                    dir.path,
                                    r.new_files,
                                    r.updated_files
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
        if state.queue_worker_running.compare_exchange(
            false, true, Ordering::SeqCst, Ordering::SeqCst,
        ).is_err() {
            tracing::debug!("Queue worker already running, skipping");
            return;
        }

        // H1：embedding 队列受 EmbeddingQueue 治理（默认 Balanced 25% CPU / 1GB RAM）。
        // 此 worker 是 attune-server 生产路径，比 attune-core::queue::QueueWorker 多 flush 逻辑。
        let governor = global_registry().register(TaskKind::EmbeddingQueue);

        std::thread::spawn(move || {
            tracing::info!("Queue worker started");
            const BATCH_SIZE: usize = 32;  // 与 attune-core/src/queue.rs 保持一致
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
                let embedding = state.embedding.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let vectors_ready = state.vectors.lock().unwrap_or_else(|e| e.into_inner()).is_some();
                let fulltext_ready = state.fulltext.lock().unwrap_or_else(|e| e.into_inner()).is_some();

                if embedding.is_none() || !vectors_ready || !fulltext_ready {
                    std::thread::sleep(POLL_INTERVAL);
                    continue;
                }
                let embedding = embedding.expect("is_none() checked above");

                if !embedding.is_available() {
                    std::thread::sleep(POLL_INTERVAL);
                    continue;
                }

                // 取一批任务
                let tasks_result = {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    vault.store().dequeue_embeddings(BATCH_SIZE)
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

                // 调 attune-core::queue::embed_and_index_batch 共享批处理。
                // 锁顺序：vault → vectors → fulltext，全程持锁直到 done_ids 取出。
                let done_ids: Vec<i64> = {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    let mut vecs_guard = state.vectors.lock().unwrap_or_else(|e| e.into_inner());
                    let ft_guard = state.fulltext.lock().unwrap_or_else(|e| e.into_inner());

                    let (Some(vi), Some(ft)) = (vecs_guard.as_mut(), ft_guard.as_ref()) else {
                        tracing::debug!("Queue worker: vectors/fulltext index unavailable mid-batch");
                        drop(ft_guard);
                        drop(vecs_guard);
                        drop(vault);
                        std::thread::sleep(POLL_INTERVAL);
                        continue;
                    };

                    match attune_core::queue::embed_and_index_batch(
                        vault.store(),
                        embedding.as_ref(),
                        vi,
                        ft,
                        &embed_tasks,
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
                    let dek_opt = state.vault.lock().unwrap_or_else(|e| e.into_inner())
                        .dek_db().ok();
                    let vecs = state.vectors.lock().unwrap_or_else(|e| e.into_inner());
                    if let (Some(dek), Some(vi)) = (dek_opt, vecs.as_ref()) {
                        let p = attune_core::platform::data_dir().join("vectors.encbin");
                        if let Err(e) = vi.save_encrypted(&dek, &p) {
                            tracing::warn!("Vector flush failed: {e}");
                        } else {
                            tracing::info!("Vector index flushed ({} entries after +{} new)",
                                vi.len(), flush_counter);
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
                let dek_opt = state.vault.lock().unwrap_or_else(|e| e.into_inner())
                    .dek_db().ok();
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
        // 需要 LLM 才能运行
        if state.llm.lock().unwrap_or_else(|e| e.into_inner()).is_none() {
            return;
        }

        if state.evolve_worker_running.compare_exchange(
            false, true, Ordering::SeqCst, Ordering::SeqCst,
        ).is_err() {
            tracing::debug!("Skill evolver already running, skipping");
            return;
        }

        // H1：SkillEvolution 受治理 + LLM 速率限制（默认 Balanced 10 calls/h）。
        // 4 小时检查一次本身已是低频，但仍接入 governor 以便：
        // (1) 全局 Pause 立即生效  (2) 切档时 LLM 配额自动调整
        let governor = global_registry().register(TaskKind::SkillEvolution);

        std::thread::spawn(move || {
            tracing::info!("Skill evolver started (runs every 4h or at {} signals)",
                attune_core::skill_evolution::EVOLVE_THRESHOLD);
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

                let llm = match state.llm.lock().unwrap_or_else(|e| e.into_inner()).as_ref().cloned() {
                    Some(l) => l,
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
                    tracing::info!("Skill evolver LLM quota exceeded (per-hour cap), skipping cycle");
                    continue;
                }

                // Phase 2（无锁）：LLM 调用，可能耗时 15s+
                let expansions = match attune_core::skill_evolution::generate_expansions(llm.as_ref(), &signals) {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::warn!("Skill evolver LLM error: {}", e);
                        continue;
                    }
                };

                // Phase 3（锁）：合并 + 标记已处理
                {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    match attune_core::skill_evolution::apply_evolution_result(vault.store(), &signals, &expansions) {
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
        // 需要 LLM 才能运行
        if state.llm.lock().unwrap_or_else(|e| e.into_inner()).is_none() {
            return;
        }

        if state.memory_consolidator_running.compare_exchange(
            false, true, Ordering::SeqCst, Ordering::SeqCst,
        ).is_err() {
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

                let llm = match state.llm.lock().unwrap_or_else(|e| e.into_inner()).as_ref().cloned() {
                    Some(l) => l,
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
                        vault.store(), &dek, now_secs,
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
                        for _ in 0..deferred { summaries.push(None); }
                        break;
                    }
                    summaries.push(
                        attune_core::memory_consolidation::generate_one_episodic_memory(
                            llm.as_ref(), bundle,
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
                        vault.store(), &dek, &bundles, &summaries, &model_name, now_secs,
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

            state.memory_consolidator_running.store(false, Ordering::SeqCst);
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
                .map(|rows| rows.into_iter().map(|r| (r.memory_id, r.embedding)).collect())
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
            let llm = match state.llm.lock().unwrap_or_else(|e| e.into_inner()).as_ref().cloned() {
                Some(l) => l,
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
                    vault.store(), &dek, &clusters, &summaries, model_name, now_secs,
                ) {
                    Ok((r, ids)) => {
                        if r.inserted > 0 {
                            tracing::info!(
                                "Memory consolidator: {} new semantic memories ({} superseded)",
                                r.inserted, r.superseded,
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
                    Ok(n) => tracing::info!("Memory consolidator: {n} episodic memories demoted to cold"),
                    Err(e) => tracing::warn!("cold demotion error: {e}"),
                }
            }
        }
    }

    /// Embed every memory that lacks a `memory_vectors` row, write the vector, and
    /// upsert it into the in-memory `memory_index`. Cost tier 2 (local embedding).
    fn embed_pending_memories(state: &std::sync::Arc<AppState>, now_secs: i64) {
        let embedder = match state.embedding.lock().unwrap_or_else(|e| e.into_inner()).as_ref().cloned() {
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
                if let Err(e) = vault.store().put_memory_vector(&mem_id, &vec, &model, now_secs) {
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
        // Persist vectors before clearing（忽略失败：最坏情况重启需重新 embed）
        {
            let dek_opt = self.vault.lock().unwrap_or_else(|e| e.into_inner())
                .dek_db().ok();
            let vecs = self.vectors.lock().unwrap_or_else(|e| e.into_inner());
            if let (Some(dek), Some(vi)) = (dek_opt, vecs.as_ref()) {
                let vectors_path = attune_core::platform::data_dir().join("vectors.encbin");
                if let Err(e) = vi.save_encrypted(&dek, &vectors_path) {
                    tracing::warn!("Vector index flush on lock failed (non-fatal): {e}");
                } else {
                    tracing::info!("Vector index persisted to {} ({} entries)",
                        vectors_path.display(), vi.len());
                }
            }
        }
        *self.fulltext.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.vectors.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.embedding.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.reranker.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.llm.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.vlm.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.web_search.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.tag_index.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.cluster_snapshot.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.taxonomy.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.classifier.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.search_cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
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
        self.search_cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
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

    /// 写 embedding provider. settings hot-reload 路径调.
    pub fn set_embedding(&self, p: Option<Arc<dyn EmbeddingProvider>>) {
        if let Ok(mut g) = self.embedding.lock() {
            *g = p;
        }
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
    pub fn member_verifier(
        &self,
    ) -> Arc<dyn attune_core::member_verifier::MemberVerifier> {
        self.member_verifier
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Replace the member-paid verifier — TEST seam. Lets a test inject a verifier that performs a
    /// real (offline, deterministic) license match so the member-gate is exercised without a live
    /// cloud, instead of bypassing it via a blanket client claim.
    pub fn set_member_verifier(
        &self,
        v: Arc<dyn attune_core::member_verifier::MemberVerifier>,
    ) {
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
    /// `flush_interval_ms` follows spec §11 risk 6 (100ms laptop / 500ms K3);
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

/// 按 settings + 硬件构建 LLM provider。
///
/// 四级优先级：
/// 1. settings.llm.endpoint 非空 → OpenAI-compatible（hiapi / DeepSeek / Qwen 等）
/// 2. settings.llm.provider == "local" + model 非空 → OllamaLlmProvider::with_model
/// 3. form_factor.prefers_local_llm() (K3 一体机) → Ollama auto-detect
/// 4. 其他笔电 / 服务器 + 无 cloud config → None（chat 返回 503 引导配置）
///
/// 抽出为自由函数后，可以同时被 init_search_engines (启动 unlock 一次)
/// 和 reload_llm (settings 改 llm 字段后热切) 复用。
fn build_llm_from_settings(
    settings_json: &Option<serde_json::Value>,
    hardware: &attune_core::platform::HardwareProfile,
) -> Option<Arc<dyn LlmProvider>> {
    let configured_llm = settings_json.as_ref().and_then(|settings| {
        let llm = settings.get("llm")?;
        let endpoint = llm.get("endpoint").and_then(|v| v.as_str()).map(|s| s.to_string());
        let api_key = llm.get("api_key").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let model = llm.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let provider = llm.get("provider").and_then(|v| v.as_str()).unwrap_or("local");

        if let Some(ep) = endpoint.filter(|s| !s.is_empty()) {
            tracing::info!("LLM: using configured endpoint {ep}");
            Some(Arc::new(OpenAiLlmProvider::new(&ep, &api_key, &model)) as Arc<dyn LlmProvider>)
        } else if provider == "local" && !model.is_empty() {
            tracing::info!("LLM: using Ollama with configured model {model}");
            Some(Arc::new(OllamaLlmProvider::with_model(&model)) as Arc<dyn LlmProvider>)
        } else {
            None
        }
    });

    configured_llm.or_else(|| {
        if hardware.form_factor.prefers_local_llm() {
            OllamaLlmProvider::auto_detect().ok().map(|llm| {
                tracing::info!("LLM (K3 form factor): using Ollama auto-detect");
                Arc::new(llm) as Arc<dyn LlmProvider>
            })
        } else {
            tracing::warn!(
                "LLM: form_factor={:?} + no cloud endpoint configured → no LLM (chat 将返回 503 提示用户配置 cloud API key per CLAUDE.md M2)",
                hardware.form_factor
            );
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(state.usage().is_none(), "set_usage(None) leaves aggregator None");
    }
}
