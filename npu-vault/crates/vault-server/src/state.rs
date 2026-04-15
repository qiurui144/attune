use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use vault_core::classifier::Classifier;
use vault_core::clusterer::ClusterSnapshot;
use vault_core::embed::{EmbeddingProvider, OllamaProvider};
use vault_core::index::FulltextIndex;
use vault_core::llm::{LlmProvider, OllamaLlmProvider, OpenAiLlmProvider};
use vault_core::tag_index::TagIndex;
use vault_core::taxonomy::Taxonomy;
use vault_core::vault::Vault;
use vault_core::vectors::VectorIndex;
use vault_core::web_search::WebSearchProvider;

const SEARCH_CACHE_CAPACITY: usize = 256;
const SEARCH_CACHE_TTL_SECS: u64 = 30;

pub struct CachedSearch {
    pub query: String,
    pub results: Vec<vault_core::search::SearchResult>,
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
    pub reranker: Mutex<Option<Arc<dyn vault_core::infer::RerankProvider>>>,
    pub llm: Mutex<Option<Arc<dyn LlmProvider>>>,
    pub web_search: Mutex<Option<Arc<dyn WebSearchProvider>>>,
    pub tag_index: Mutex<Option<TagIndex>>,
    pub cluster_snapshot: Mutex<Option<ClusterSnapshot>>,
    pub taxonomy: Mutex<Option<Arc<Taxonomy>>>,
    pub classifier: Mutex<Option<Arc<Classifier>>>,
    pub require_auth: bool,
    /// 防止重复启动 QueueWorker 后台线程
    pub queue_worker_running: AtomicBool,
    /// 防止重复启动 ClassifyWorker 后台线程
    pub classify_worker_running: AtomicBool,
    /// 防止重复启动 RescanWorker 后台线程
    pub rescan_worker_running: AtomicBool,
    /// 防止并发 unlock 重复初始化搜索引擎（重建索引会清空内存向量）
    pub engines_initialized: AtomicBool,
    pub search_cache: Mutex<LruCache<u64, CachedSearch>>,
}

impl AppState {
    pub fn new(vault: Vault, require_auth: bool) -> Self {
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
            engines_initialized: AtomicBool::new(false),
            search_cache: Mutex::new(LruCache::new(
                NonZeroUsize::new(SEARCH_CACHE_CAPACITY).unwrap()
            )),
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
        // Fulltext index (persistent on disk)
        {
            let tantivy_dir = vault_core::platform::data_dir().join("tantivy");
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

        // Vector index (1024 dims for bge-m3)
        if let Ok(mut guard) = self.vectors.lock() {
            *guard = VectorIndex::new(1024).ok();
        }

        // Try ONNX embedding first; fall back to Ollama if model not available
        if let Ok(mut guard) = self.embedding.lock() {
            let provider: Arc<dyn EmbeddingProvider> =
                match vault_core::infer::embedding::OrtEmbeddingProvider::qwen3_embedding_0_6b() {
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
            match vault_core::infer::reranker::OrtRerankProvider::bge_reranker_v2_m3() {
                Ok(r) => {
                    tracing::info!("Reranker: OrtRerankProvider (bge-reranker-v2-m3)");
                    *guard = Some(Arc::new(r));
                }
                Err(e) => {
                    tracing::info!("Reranker unavailable ({e}), will use vector cosine fallback");
                }
            }
        }

        // LLM 三级优先级：1. 配置文件 llm.endpoint  2. Ollama 自动探测  3. 无 LLM
        let llm_result: Option<Arc<dyn LlmProvider>> = {
            // 级别 1：读取 settings 中的 llm 配置
            let configured_llm = {
                let vault_guard = self.vault.lock().unwrap_or_else(|e| e.into_inner());
                vault_guard.store().get_meta("app_settings").ok().flatten()
                    .and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok())
                    .and_then(|settings| {
                        let endpoint = settings.get("llm")
                            .and_then(|l| l.get("endpoint"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let api_key = settings.get("llm")
                            .and_then(|l| l.get("api_key"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let model = settings.get("llm")
                            .and_then(|l| l.get("model"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("gpt-4o-mini")
                            .to_string();
                        endpoint.map(|ep| {
                            tracing::info!("LLM: using configured endpoint {ep}");
                            Arc::new(OpenAiLlmProvider::new(&ep, &api_key, &model))
                                as Arc<dyn LlmProvider>
                        })
                    })
            };

            // 级别 2：Ollama 自动探测
            configured_llm.or_else(|| {
                OllamaLlmProvider::auto_detect().ok().map(|llm| {
                    tracing::info!("LLM: using Ollama auto-detect");
                    Arc::new(llm) as Arc<dyn LlmProvider>
                })
            })
            // 级别 3：None（Chat 功能禁用）
        };

        if let Some(llm_arc) = llm_result {
            let mut tax = Taxonomy::default();
            if let Ok(plugins) = Taxonomy::load_builtin_plugins() {
                for p in plugins {
                    tax = tax.with_plugin(p);
                }
            }
            // Load user plugins from config_dir/plugins/*.yaml
            let (user_plugins, _errors) = Taxonomy::load_user_plugins(&vault_core::platform::config_dir());
            for p in user_plugins {
                tax = tax.with_plugin(p);
            }
            let tax_arc = Arc::new(tax);

            *self.classifier.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(Arc::new(Classifier::new(tax_arc.clone(), llm_arc.clone())));
            *self.taxonomy.lock().unwrap_or_else(|e| e.into_inner()) = Some(tax_arc);
            *self.llm.lock().unwrap_or_else(|e| e.into_inner()) = Some(llm_arc);
        }

        // Web search provider（从 app_settings.web_search 加载）
        {
            let ws_provider = {
                let vault_guard = self.vault.lock().unwrap_or_else(|e| e.into_inner());
                vault_guard.store().get_meta("app_settings").ok().flatten()
                    .and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok())
                    .and_then(|settings| vault_core::web_search::from_settings(&settings))
            };
            if let Some(ws) = ws_provider {
                tracing::info!("Web search: {} provider enabled", ws.provider_name());
                *self.web_search.lock().unwrap_or_else(|e| e.into_inner()) = Some(ws);
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
    pub fn drain_classify_batch(&self, batch_size: usize) -> vault_core::error::Result<usize> {
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

        std::thread::spawn(move || {
            tracing::info!("Classify worker started");
            loop {
                // Check if vault is still unlocked
                {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    if !matches!(vault.state(), vault_core::vault::VaultState::Unlocked) {
                        break;
                    }
                }

                match state.drain_classify_batch(5) {
                    Ok(0) => std::thread::sleep(std::time::Duration::from_secs(5)),
                    Ok(n) => {
                        tracing::info!("Classified {} items", n);
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

        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(30 * 60)); // 30 minutes

                // Check vault still unlocked
                let (dek, dirs) = {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    if !matches!(vault.state(), vault_core::vault::VaultState::Unlocked) {
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

                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    match vault_core::scanner::scan_directory(
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

        std::thread::spawn(move || {
            tracing::info!("Queue worker started");
            const BATCH_SIZE: usize = 10;
            const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
            const MAX_ATTEMPTS: i32 = 3;

            loop {
                // 检查 vault 是否仍处于 unlocked 状态
                let vault_unlocked = {
                    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                    matches!(vault.state(), vault_core::vault::VaultState::Unlocked)
                };
                if !vault_unlocked {
                    break;
                }

                // 检查 embedding + vectors + fulltext 是否就绪
                let embedding = state.embedding.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let vectors_ready = state.vectors.lock().unwrap_or_else(|e| e.into_inner()).is_some();
                let fulltext_ready = state.fulltext.lock().unwrap_or_else(|e| e.into_inner()).is_some();

                if embedding.is_none() || !vectors_ready || !fulltext_ready {
                    std::thread::sleep(POLL_INTERVAL);
                    continue;
                }
                let embedding = embedding.unwrap();

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
                                    vault_core::vectors::VectorMeta {
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

                tracing::debug!("Queue worker processed {} embed tasks", embed_tasks.len());
            }

            // 退出时重置标志
            state.queue_worker_running.store(false, Ordering::SeqCst);
            tracing::info!("Queue worker stopped (vault locked or engines cleared)");
        });
    }

    /// 清除搜索引擎 + 分类引擎 (lock 前调用)
    pub fn clear_search_engines(&self) {
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
