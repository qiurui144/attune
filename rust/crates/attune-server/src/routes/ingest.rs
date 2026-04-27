use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use attune_core::chunker;

use crate::state::SharedState;

#[derive(Deserialize)]
pub struct IngestRequest {
    pub title: String,
    pub content: String,
    #[serde(default = "default_source_type")]
    pub source_type: String,
    pub url: Option<String>,
    pub domain: Option<String>,
    pub tags: Option<Vec<String>>,
}

fn default_source_type() -> String {
    "note".into()
}

/// JSON ingest 内容上限（防止大负载写放大攻击）
const MAX_INGEST_CONTENT: usize = 2 * 1024 * 1024; // 2 MB
const MAX_INGEST_TITLE: usize = 500;

pub async fn ingest(
    State(state): State<SharedState>,
    Json(body): Json<IngestRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if body.title.len() > MAX_INGEST_TITLE {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({"error": format!("title too long (max {MAX_INGEST_TITLE} bytes)")})),
        ));
    }
    if body.content.len() > MAX_INGEST_CONTENT {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({"error": format!("content too large: {} bytes (max {MAX_INGEST_CONTENT})", body.content.len())})),
        ));
    }
    let vault = state.vault.lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "vault lock poisoned"}))))?;
    let dek = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    let id = vault
        .store()
        .insert_item(
            &dek,
            &body.title,
            &body.content,
            body.url.as_deref(),
            &body.source_type,
            body.domain.as_deref(),
            body.tags.as_deref(),
        )
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?;

    // Invalidate search cache after new item inserted
    {
        if let Ok(mut cache) = state.search_cache.lock() {
            cache.clear();
        }
    }

    // Add to fulltext index
    {
        let ft_guard = state.fulltext.lock()
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": "ft lock poisoned"}))))?;
        if let Some(ft) = ft_guard.as_ref() {
            let _ = ft.add_document(&id, &body.title, &body.content, &body.source_type);
        }
    }

    // F2 (W3 batch A, 2026-04-27)：与 /upload 同模式写 chunk_breadcrumbs sidecar
    // per spec docs/superpowers/specs/2026-04-27-w3-batch-a-design.md §4 + reviewer I2
    if let Err(e) = vault.store().upsert_chunk_breadcrumbs_from_content(&id, &body.content) {
        tracing::warn!("F2 upsert_chunk_breadcrumbs failed for item {id}: {e}");
    }

    // Enqueue for embedding: two-layer indexing (sections L1 + chunks L2)
    // chunk_counter 声明在外层以便响应返回 `chunks_queued`（与 /upload 对齐）
    let mut chunk_counter = 0usize;
    {
        let sections = chunker::extract_sections(&body.content);

        // Level 1: section-level embeddings
        for (section_idx, section_text) in &sections {
            if !section_text.trim().is_empty() {
                if let Err(e) = vault.store().enqueue_embedding(
                    &id, chunk_counter, section_text, 1, 1, *section_idx,
                ) {
                    tracing::warn!("enqueue_embedding L1 failed for item {id}: {e}");
                }
                chunk_counter += 1;
            }
        }

        // Level 2: paragraph chunk embeddings
        for (section_idx, section_text) in &sections {
            for chunk_text in
                chunker::chunk(section_text, chunker::DEFAULT_CHUNK_SIZE, chunker::DEFAULT_OVERLAP)
            {
                if let Err(e) = vault.store().enqueue_embedding(
                    &id, chunk_counter, &chunk_text, 2, 2, *section_idx,
                ) {
                    tracing::warn!("enqueue_embedding L2 failed for item {id}: {e}");
                }
                chunk_counter += 1;
            }
        }
    }

    // Auto-enqueue classification
    if let Err(e) = vault.store().enqueue_classify(&id, 3) {
        tracing::warn!("enqueue_classify failed for item {id}: {e}");
    }

    Ok(Json(serde_json::json!({
        "id": id,
        "status": "ok",
        "chunks_queued": chunk_counter
    })))
}
