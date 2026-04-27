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
    pub embedding: Mutex<Option<Arc<dyn EmbeddingProvider>>>,
    pub reranker: Mutex<Option<Arc<dyn attune_core::infer::RerankProvider>>>,
    pub llm: Mutex<Option<Arc<dyn LlmProvider>>>,
    pub web_search: Mutex<Option<Arc<dyn WebSearchProvider>>>,
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
    pub search_cache: Mutex<LruCache<u64, CachedSearch>>,
    /// Sprint 1 Phase B: project recommendation broadcast channel.
    /// upload.rs / chat.rs 收到信号后 send；ws.rs subscribe 推送给前端。
    pub recommendation_tx: tokio::sync::broadcast::Sender<serde_json::Value>,
    /// Sprint 2: 启动时加载的 plugins（attune-pro / 用户 / 社区）
    pub plugin_registry: std::sync::Arc<attune_core::plugin_registry::PluginRegistry>,
}

impl AppState {
    pub fn new(vault: Vault, require_auth: bool) -> Self {
        let (recommendation_tx, _rx) = tokio::sync::broadcast::channel::<serde_json::Value>(64);
        let plugin_registry = match attune_core::plugin_registry::PluginRegistry::default_plugins_dir() {
            Ok(dir) => match attune_core::plugin_registry::PluginRegistry::scan(&dir) {
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
            embedding: Mutex::new(None),
            reranker: Mutex::new(None),
            llm: Mutex::new(None),
            web_search: Mutex::new(None),
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
            engines_initialized: AtomicBool::new(false),
            search_cache: Mutex::new(LruCache::new(
                NonZeroUsize::new(SEARCH_CACHE_CAPACITY).expect("SEARCH_CACHE_CAPACITY is non-zero const")
            )),
            // 启动时检测一次硬件，后续复用（避免每次 GET/PATCH 都同步读 /proc 等）
            hardware: attune_core::platform::HardwareProfile::detect(),
            recommendation_tx,
            plugin_registry,
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
        if let Ok(mut guard) = self.vectors.lock() {
            let vectors_path = attune_core::platform::data_dir().join("vectors.encbin");
            let dek_opt = self.vault.lock().unwrap_or_else(|e| e.into_inner())
                .dek_db().ok();
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

        // Try ONNX embedding first; fall back to Ollama if model not available
        if let Ok(mut guard) = self.embedding.lock() {
            let provider: Arc<dyn EmbeddingProvider> =
                match attune_core::infer::embedding::OrtEmbeddingProvider::qwen3_embedding_0_6b() {
                    Ok(p) => {
                        tracing::info!("Embedding: OrtEmbeddingProvider (Qwen3-Embedding-0.6B)");
                        Arc::new(p)
                    }
                    Err(e) => {
                        tracing::info!("ONNX embedding unavailable ({e}), falling back to Ollama bge-m3");
                        Arc::new(OllamaProvider::default())
                    }
                };
            *guard = Some(provider);
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

        // LLM 四级优先级：1. 配置文件 llm.endpoint（OpenAI-compatible）
        //                  2. provider=local + 明确指定 model → OllamaLlmProvider::with_model
        //                  3. Ollama 自动探测（PREFERRED_MODELS 列表）
        //                  4. 无 LLM（Chat 功能禁用）
        let llm_result: Option<Arc<dyn LlmProvider>> = {
            let configured_llm = {
                let vault_guard = self.vault.lock().unwrap_or_else(|e| e.into_inner());
                vault_guard.store().get_meta("app_settings").ok().flatten()
                    .and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok())
                    .and_then(|settings| {
                        let llm = settings.get("llm")?;
                        let endpoint = llm.get("endpoint").and_then(|v| v.as_str()).map(|s| s.to_string());
                        let api_key = llm.get("api_key").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let model = llm.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let provider = llm.get("provider").and_then(|v| v.as_str()).unwrap_or("local");

                        if let Some(ep) = endpoint {
                            // 级别 1：明确配置了 endpoint → OpenAI-compatible（含 Qwen/DeepSeek 等）
                            tracing::info!("LLM: using configured endpoint {ep}");
                            Some(Arc::new(OpenAiLlmProvider::new(&ep, &api_key, &model)) as Arc<dyn LlmProvider>)
                        } else if provider == "local" && !model.is_empty() {
                            // 级别 2：local provider + 明确指定 model → Ollama with_model
                            tracing::info!("LLM: using Ollama with configured model {model}");
                            Some(Arc::new(OllamaLlmProvider::with_model(&model)) as Arc<dyn LlmProvider>)
                        } else {
                            None
                        }
                    })
            };

            // 级别 3：Ollama 自动探测
            configured_llm.or_else(|| {
                OllamaLlmProvider::auto_detect().ok().map(|llm| {
                    tracing::info!("LLM: using Ollama auto-detect");
                    Arc::new(llm) as Arc<dyn LlmProvider>
                })
            })
            // 级别 4：None（Chat 功能禁用）
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

                // 批量 embed
                let texts: Vec<&str> = embed_tasks.iter().map(|t| t.chunk_text.as_str()).collect();
                let embeddings = match embedding.embed(&texts) {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::warn!("Embedding failed: {}", e);
                        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                        for task in &embed_tasks {
                            let _ = vault.store().mark_embedding_failed(task.id, MAX_ATTEMPTS);
                        }
                        std::thread::sleep(POLL_INTERVAL);
                        continue;
                    }
                };

                // 写入向量索引 + 全文索引，收集成功处理的 task id
                let mut done_ids: Vec<i64> = Vec::new();
                for (i, task) in embed_tasks.iter().enumerate() {
                    if i >= embeddings.len() {
                        break;
                    }
                    {
                        if let Ok(mut vecs) = state.vectors.lock() {
                            if let Some(ref mut vi) = *vecs {
                                let _ = vi.add(
                                    &embeddings[i],
                                    attune_core::vectors::VectorMeta {
                                        item_id: task.item_id.clone(),
                                        chunk_idx: task.chunk_idx as usize,
                                        level: task.level as u8,
                                        section_idx: task.section_idx as usize,
                                    },
                                );
                            }
                        }
                    }
                    if task.level == 1 {
                        if let Ok(ft_guard) = state.fulltext.lock() {
                            if let Some(ref ft) = *ft_guard {
                                let _ = ft.add_document(
                                    &task.item_id, "", &task.chunk_text, "file",
                                );
                            }
                        }
                    }
                    done_ids.push(task.id);
                }

                // 循环外：一次性标记完成（单次加锁，避免批量锁竞争）
                if !done_ids.is_empty() {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    for id in &done_ids {
                        let _ = vault.store().mark_embedding_done(*id);
                    }
                }

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
            }

            state.memory_consolidator_running.store(false, Ordering::SeqCst);
            tracing::info!("Memory consolidator stopped (vault locked)");
        });
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
        *self.web_search.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.tag_index.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.cluster_snapshot.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.taxonomy.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.classifier.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.search_cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
        // 重置初始化标志，确保再次 unlock 后能重新初始化搜索引擎
        self.engines_initialized.store(false, Ordering::SeqCst);
    }
}
