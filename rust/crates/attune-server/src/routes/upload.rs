use axum::extract::{Multipart, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::state::SharedState;
use attune_core::ingest::{RawDocument, SourceKind};

#[derive(Debug, Deserialize, Default)]
pub struct UploadQuery {
    /// OCR profile id (contract / receipt / screenshot / ancient / custom).
    /// 不传 = 走默认 300 DPI; 不存在 = registry 自动回退默认.
    #[serde(default)]
    pub profile: Option<String>,
}

/// Upload size cap. 提到 100 MB：扫描版 PDF 常在 30-80MB，整本 OCR 很合理。
/// 超过此值通常是高清扫描+彩图，建议用户预处理（pdftoppm 降 DPI、jpeg 压缩）。
///
/// ⚠ **必须与 `lib.rs` 中 `/api/v1/upload` 路由的 `DefaultBodyLimit::max(...)` 同步修改。**
/// 框架层限制早于此检查触发（在 multipart 解码前拦截），此处检查是第二道防线，
/// 防止 DefaultBodyLimit 被删除或未生效时的 OOM。两处写不一致会产生误导性行为。
const MAX_UPLOAD_BYTES: usize = 100 * 1024 * 1024; // 100 MB

pub async fn upload_file(
    State(state): State<SharedState>,
    Query(q): Query<UploadQuery>,
    mut multipart: Multipart,
) -> AppResult<Json<serde_json::Value>> {
    // First, read multipart data without holding any locks
    let (filename, data) = {
        let field = multipart
            .next_field()
            .await
            .map_err(|e| AppError::BadRequest(e.to_string()))?
            .ok_or_else(|| AppError::BadRequest("no file provided".into()))?;

        let filename = field.file_name().unwrap_or("unknown").to_string();
        let data = field
            .bytes()
            .await
            .map_err(|e| AppError::BadRequest(e.to_string()))?;
        (filename, data)
    };

    if data.len() > MAX_UPLOAD_BYTES {
        return Err(AppError::PayloadTooLarge(format!(
            "file too large: {} bytes (max {})",
            data.len(),
            MAX_UPLOAD_BYTES
        )));
    }

    // upload 单次可塞大量 chunks，接同款 embedding 队列 backpressure 防 server hung
    const EMBEDDING_QUEUE_BACKPRESSURE_LIMIT: usize = 10_000;
    {
        let vault = state
            .vault
            .lock()
            .map_err(|_| AppError::Internal("vault lock poisoned".into()))?;
        if let Ok(pending) = vault.store().pending_count_by_type("embed") {
            if pending > EMBEDDING_QUEUE_BACKPRESSURE_LIMIT {
                // rich error: retry 信号字段, 走 Detailed 保完整 body
                return Err(AppError::detailed(
                    StatusCode::SERVICE_UNAVAILABLE,
                    serde_json::json!({
                        "error": format!("embedding queue backpressure ({pending} pending > {EMBEDDING_QUEUE_BACKPRESSURE_LIMIT} limit), retry later"),
                        "pending_embeddings": pending,
                        "retry_after_seconds": 30,
                    }),
                ));
            }
        }
    }

    // Now lock vault for DB operations (no more await points after this)
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault
        .dek_db()
        .map_err(|e| AppError::Forbidden(e.to_string()))?;

    // content_hash 短路 + 入库走统一 pipeline。OCR profile 透传给 parser。
    // upload 无来源域 / 用户标签 / 语料领域 —— domain/tags/corpus_domain 传 None。
    let raw = RawDocument {
        uri: format!("upload://{filename}"),
        title: String::new(), // 让 parser 从内容提取标题
        content: data.to_vec(),
        mime_hint: Some(mime_from_filename(&filename).to_string()),
        source_kind: SourceKind::LocalFolder,
        source_ref: format!("upload://{filename}"),
        modified_marker: None,
        domain: None,
        tags: None,
        corpus_domain: None,
        metadata: std::collections::HashMap::new(),
    };

    let outcome = attune_core::ingest::ingest_document_with_profile(
        vault.store(),
        &dek,
        &raw,
        q.profile.as_deref(),
    )
    .map_err(|e| AppError::Unprocessable(e.to_string()))?;

    let (item_id, chunks_queued, is_new) = match &outcome {
        attune_core::ingest::IngestOutcome::Inserted { item_id, chunks_enqueued } => {
            (item_id.clone(), *chunks_enqueued, true)
        }
        attune_core::ingest::IngestOutcome::Duplicate { item_id } => {
            tracing::info!("upload content_hash dedup hit: filename={filename} existing_item={item_id}");
            // dedup 分支 response 与成功分支字段对齐，client 两分支读同名字段。
            return Ok(Json(serde_json::json!({
                "id": item_id,
                "title": filename,
                "chunks_queued": 0,
                "status": "duplicate",
                "dedup_reason": "content_hash",
            })));
        }
        attune_core::ingest::IngestOutcome::Updated { item_id, .. } => {
            (item_id.clone(), 0usize, true)
        }
        attune_core::ingest::IngestOutcome::Skipped { reason } => {
            return Err(AppError::Unprocessable(reason.clone()));
        }
    };

    // 留存原始上传文件（AES-GCM 加密），供「查看证据原文」核对 OCR 转录。
    // items.content 只存解析后的文本；律师须能回看原始扫描/图片核验识别准确度。
    // 失败不阻塞上传 — 但记 warn（原件丢失 = 证据无法回溯核验）。
    if is_new {
        if let Err(e) = vault.store().insert_item_blob(
            &dek,
            &item_id,
            &filename,
            mime_from_filename(&filename),
            &data[..],
        ) {
            tracing::warn!("insert_item_blob failed for item {item_id}: {e}");
        }
    }

    // 即时 FTS 索引（搜索不依赖 AI 即可工作）。
    // ingest_document 不碰 VectorIndex / FulltextIndex（server AppState 独立 Mutex），
    // FTS 即时写由此 server 薄壳补充（锁顺序与 embed_worker 不相交）。
    // parsed_title：parser 从内容提取的真实标题（如 Markdown H1），用于响应 + FTS；
    // get_item 失败时回退原始文件名。
    let parsed_title = if is_new {
        if let Ok(Some(item)) = vault.store().get_item(&dek, &item_id) {
            let ft_guard = state.fulltext.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ft) = ft_guard.as_ref() {
                let _ = ft.add_document(&item_id, &item.title, &item.content, "file");
            }
            item.title.clone()
        } else {
            filename.clone()
        }
    } else {
        filename.clone()
    };

    // 释放 vault guard，让后续 spawn task 能独立 lock vault
    drop(vault);

    // 新文档入库 → 失效 search 缓存，否则之前搜过的 query
    // 命中旧缓存，新文档搜不到
    state.invalidate_search_cache();

    // Sprint 1 Phase B: 异步跑 ProjectRecommender，命中阈值通过 ws 推送
    {
        let filename_clone = filename.clone();
        let item_id_clone = item_id.clone();
        let state_clone = state.clone();
        tokio::spawn(async move {
            let vault_guard = state_clone.vault.lock();
            let vault_guard = vault_guard.unwrap_or_else(|e| e.into_inner());
            if !matches!(vault_guard.state(), attune_core::vault::VaultState::Unlocked) {
                return;
            }
            // 抽 entities — 用 filename 当样本（chunk-level entities 可在 Phase D 优化）
            let new_ents = attune_core::entities::extract_entities(&filename_clone);
            if new_ents.is_empty() {
                return;
            }
            // 收集所有 active project 的 entities（基于 title 简化）
            let projects = match vault_guard.store().list_projects(false) {
                Ok(v) => v,
                Err(_) => return,
            };
            let project_ents_storage: Vec<(String, Vec<attune_core::entities::Entity>)> = projects
                .iter()
                .map(|p| (p.id.clone(), attune_core::entities::extract_entities(&p.title)))
                .collect();
            let project_entities: Vec<(&String, Vec<attune_core::entities::Entity>)> = project_ents_storage
                .iter()
                .map(|(id, ents)| (id, ents.clone()))
                .collect();
            let candidates = attune_core::project_recommender::recommend_for_file(
                vault_guard.store(),
                &item_id_clone,
                &new_ents,
                Some(project_entities),
            )
            .unwrap_or_default();
            if candidates.is_empty() {
                return;
            }
            let title_map: std::collections::HashMap<String, String> = projects
                .iter()
                .map(|p| (p.id.clone(), p.title.clone()))
                .collect();
            let payload = serde_json::json!({
                "type": "project_recommendation",
                "trigger": "file_uploaded",
                "file_id": item_id_clone,
                "candidates": candidates.iter().map(|c| serde_json::json!({
                    "project_id": c.project_id,
                    "project_title": title_map.get(&c.project_id).cloned().unwrap_or_default(),
                    "score": c.score,
                    "overlapping_entities": c.overlapping_entities,
                })).collect::<Vec<_>>(),
            });
            let _ = state_clone.recommendation_tx.send(payload);
        });
    }

    // 行业 workflow trigger（各 vertical 的跨实体推理 / 行业自动化）由对应 vertical
    // 插件通过 plugin loader 注册到运行时 trigger map，不在 attune-core/server 内置。

    // Sprint 2 Phase A: file_added trigger — 基于 plugin_registry 匹配 workflow
    let item_id_for_wf = item_id.clone();
    let state_for_wf = state.clone();
    tokio::spawn(async move {
        let vault_guard = state_for_wf.vault.lock();
        let vault_guard = vault_guard.unwrap_or_else(|e| e.into_inner());
        if !matches!(vault_guard.state(), attune_core::vault::VaultState::Unlocked) {
            return;
        }
        // 找该 file_id 归属的 project
        let projects = match vault_guard.store().list_projects(false) {
            Ok(v) => v,
            Err(_) => return,
        };
        let mut matched_project: Option<String> = None;
        for p in &projects {
            if let Ok(files) = vault_guard.store().list_files_for_project(&p.id) {
                if files.iter().any(|f| f.file_id == item_id_for_wf) {
                    matched_project = Some(p.id.clone());
                    break;
                }
            }
        }
        let Some(pid) = matched_project else {
            return; // 未归 project，不触发任何 workflow
        };
        // 从 registry 取所有匹配的 workflow（clone 出来避免 borrow 跨 await）
        let registry = state_for_wf.plugin_registry.clone();
        let matched: Vec<(String, attune_core::workflow::Workflow)> = registry
            .workflows_by_trigger("file_added")
            .into_iter()
            .map(|lwf| (lwf.plugin_id.clone(), lwf.workflow.clone()))
            .collect();
        if matched.is_empty() {
            return; // 没装任何 file_added workflow plugin
        }
        // Sprint 2 Phase D：拉 vault dek_db 透传给 workflow（write_annotation 需要加密 content）。
        // vault 在前面 state() 检查中已确认 unlocked，理论上 dek_db() 不会失败；
        // 兜底：拿不到 dek 就跳过整批 workflow（不静默退化为 stub 写入）。
        let dek = match vault_guard.dek_db() {
            Ok(k) => k,
            Err(e) => {
                tracing::warn!("workflow trigger skipped: vault dek_db unavailable: {e}");
                return;
            }
        };
        for (plugin_id, workflow) in matched {
            let mut data = std::collections::BTreeMap::new();
            data.insert("file_id".into(), serde_json::json!(item_id_for_wf));
            data.insert("project_id".into(), serde_json::json!(pid));
            let event = attune_core::workflow::WorkflowEvent {
                event_type: "file_added".into(),
                data,
            };
            match attune_core::workflow::run_workflow(
                &workflow,
                &event,
                Some(vault_guard.store()),
                Some(&dek),
            ) {
                Ok(_result) => {
                    let payload = serde_json::json!({
                        "type": "workflow_complete",
                        "workflow_id": workflow.id,
                        "plugin_id": plugin_id,
                        "file_id": item_id_for_wf,
                        "project_id": pid,
                    });
                    let _ = state_for_wf.recommendation_tx.send(payload);
                }
                Err(e) => {
                    tracing::warn!(
                        "workflow {} (plugin {}) failed: {}",
                        workflow.id, plugin_id, e
                    );
                }
            }
        }
    });

    Ok(Json(serde_json::json!({
        "id": item_id,
        "title": parsed_title,
        "chunks_queued": chunks_queued,
        "status": "processing"
    })))
}

/// 由文件名扩展名推断 MIME —— 用于 A1 原件留存的 Content-Type，
/// 以及取回路由让浏览器正确内联预览图片 / PDF。
fn mime_from_filename(filename: &str) -> &'static str {
    let ext = filename
        .rsplit('.')
        .next()
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tiff" | "tif" => "image/tiff",
        "gif" => "image/gif",
        "txt" | "md" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "docx" => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        }
        "doc" => "application/msword",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        _ => "application/octet-stream",
    }
}
