use std::cell::RefCell;

use crate::state::SharedState;
use attune_core::ingest::{
    ingest_document, ingest_document_replacing, DocumentSink, IngestOutcome, RawDocument,
    SourceConnector,
};
use attune_core::scanner_webdav::{WebDavConfig, WebDavConnector};
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct BindRemoteRequest {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    #[serde(default = "default_depth")]
    pub depth: u32,
    /// 语料领域（legal / tech / medical / patent / general），驱动 F-Pro
    /// 跨域防污染。缺省 general。
    pub corpus_domain: Option<String>,
}
fn default_depth() -> u32 {
    1
}

/// POST /api/v1/index/bind-remote — 绑定远程 WebDAV 目录并扫描入库。
///
/// route 层直接驱动 WebDavConnector，ETag 增量判断在此内联（对应 Task 11 的
/// sync_webdav_dir 公共函数抽取点）。响应 scan 字段形态与重构前完全一致。
pub async fn bind_remote(
    State(state): State<SharedState>,
    Json(body): Json<BindRemoteRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if body.depth > 2 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "depth must be <= 2 to prevent runaway directory traversal"})),
        ));
    }
    let config = WebDavConfig {
        url: body.url.clone(),
        username: body.username.clone(),
        password: body.password.clone(),
        depth: body.depth,
    };

    // 创建/复用 bound_dirs 记录（webdav: 前缀标记远程目录）。
    let dir_id = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let _ = vault.dek_db().map_err(|e| {
            (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?;

        vault
            .store()
            .bind_directory(&format!("webdav:{}", body.url), false, &["md", "txt"])
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": e.to_string()})),
                )
            })?
    };

    // 决策 4：落库加密 remote 配置，让周期 worker 能读回凭据自动重扫。
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db().map_err(|e| {
            (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
        })?;
        let input = attune_core::store::webdav_remotes::WebDavRemoteInput {
            dir_id: dir_id.clone(),
            url: body.url.clone(),
            username: body.username.clone(),
            password: body.password.clone(),
            depth: body.depth,
            corpus_domain: body.corpus_domain.clone().unwrap_or_else(|| "general".into()),
        };
        if let Err(e) = vault.store().upsert_webdav_remote(&dek, &input) {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("persist webdav remote: {e}")})),
            ));
        }
    }

    // WebDAV I/O 是阻塞的 —— 整段在 spawn_blocking 里跑，不阻塞 axum worker。
    let corpus_domain_for_dir = body.corpus_domain.clone().unwrap_or_else(|| "general".into());
    let state_clone = state.clone();
    let dir_id_clone = dir_id.clone();
    let scan = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, String> {
        let vault = state_clone.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db().map_err(|e| e.to_string())?;
        let store = vault.store();
        let connector = WebDavConnector::new(config);

        // 统计计数器 —— RefCell 让 sink closure 与外层共享可变访问。
        let total = RefCell::new(0usize);
        let new_files = RefCell::new(0usize);
        let updated_files = RefCell::new(0usize);
        let skipped_files = RefCell::new(0usize);
        let errors: RefCell<Vec<String>> = RefCell::new(Vec::new());

        let mut sink: DocumentSink<'_> = Box::new(|mut raw: RawDocument| {
            // corpus_domain 由 bind-remote 请求参数决定，回填到每份文档。
            raw.corpus_domain = Some(corpus_domain_for_dir.clone());

            let source_ref = raw.source_ref.clone();
            let etag = raw.modified_marker.clone().unwrap_or_default();
            let filename = source_ref.rsplit('/').next().unwrap_or(&source_ref).to_string();

            *total.borrow_mut() += 1;

            // ETag 增量判断：与 indexed_files.file_hash 比对，ETag 未变则跳过。
            let existing = store.get_indexed_file(&source_ref).ok().flatten();
            if let Some(ref ex) = existing {
                if ex.file_hash == etag {
                    *skipped_files.borrow_mut() += 1;
                    return;
                }
            }

            // 内容已变（或首次入库）：删旧 item + 入队 purge + 记 doc_update 信号。
            let old_item_id: Option<String> = existing.as_ref().and_then(|ex| {
                ex.item_id.as_ref().map(|id| {
                    let _ = store.delete_item(id);
                    if let Err(e) = store.enqueue_reindex(id, "purge") {
                        tracing::warn!("bind_remote: enqueue_reindex(purge) failed for {id}: {e}");
                    }
                    if let Err(e) = store.record_signal_event("doc_update", id, None) {
                        tracing::debug!("bind_remote: record_signal_event failed for {id}: {e}");
                    }
                    id.clone()
                })
            });

            // 走统一 ingest_document pipeline（含 L2 embedding + classify）。
            let outcome = if let Some(ref old_id) = old_item_id {
                ingest_document_replacing(store, &dek, &raw, old_id)
            } else {
                ingest_document(store, &dek, &raw)
            };

            match outcome {
                Ok(IngestOutcome::Inserted { item_id, .. }) => {
                    let _ = store.upsert_indexed_file(&dir_id_clone, &source_ref, &etag, &item_id);
                    if old_item_id.is_some() {
                        *updated_files.borrow_mut() += 1;
                    } else {
                        *new_files.borrow_mut() += 1;
                    }
                }
                Ok(IngestOutcome::Updated { item_id, .. }) => {
                    let _ = store.upsert_indexed_file(&dir_id_clone, &source_ref, &etag, &item_id);
                    *updated_files.borrow_mut() += 1;
                }
                Ok(IngestOutcome::Duplicate { .. }) | Ok(IngestOutcome::Skipped { .. }) => {
                    *skipped_files.borrow_mut() += 1;
                }
                Err(e) => {
                    errors.borrow_mut().push(format!("{filename}: ingest {e}"));
                }
            }
        });

        connector
            .fetch_documents(&mut sink)
            .map_err(|e| e.to_string())?;
        drop(sink);

        Ok(serde_json::json!({
            "total_files": *total.borrow(),
            "new_files": *new_files.borrow(),
            "updated_files": *updated_files.borrow(),
            "skipped_files": *skipped_files.borrow(),
            "errors": *errors.borrow(),
        }))
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e})),
        )
    })?;

    Ok(Json(serde_json::json!({
        "status": "ok",
        "dir_id": dir_id,
        "scan": scan,
    })))
}
