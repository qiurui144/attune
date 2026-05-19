use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use attune_core::ingest::{ingest_document, IngestOutcome, RawDocument, SourceKind};

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

/// OSS-S15 fix: embedding 队列深度上限。R18 复现 5p mixed 60min 后累积 30K+ pending
/// embeddings，server 进入 5min hung（后台 worker 串行 drain Ollama HTTP，前端读路径
/// 锁竞争阻塞）。超过此阈值 ingest 入口返回 503 backpressure 强制客户端 retry-after。
const EMBEDDING_QUEUE_BACKPRESSURE_LIMIT: usize = 10_000;

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
    // OSS-S15 fix: 检查 embedding 队列深度，超阈值返回 503 强制 backpressure
    if let Ok(pending) = vault.store().pending_count_by_type("embed") {
        if pending > EMBEDDING_QUEUE_BACKPRESSURE_LIMIT {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": format!("embedding queue backpressure ({pending} pending > {EMBEDDING_QUEUE_BACKPRESSURE_LIMIT} limit), retry later"),
                    "pending_embeddings": pending,
                    "retry_after_seconds": 30,
                })),
            ));
        }
    }
    let dek = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    // JSON ingest 的 content 已是纯文本 —— 包成 RawDocument 走统一 pipeline。
    // source_ref 用 url（缺失则用 title），让同源重复内容能命中 content_hash 短路。
    // domain / tags 经 RawDocument 一等字段透传给 insert_item（行为不变）。
    let source_ref = body.url.clone().unwrap_or_else(|| body.title.clone());
    let raw = RawDocument {
        uri: body.url.clone().unwrap_or_else(|| format!("note://{source_ref}")),
        title: body.title.clone(),
        content: body.content.clone().into_bytes(),
        mime_hint: Some("text/plain".into()),
        source_kind: SourceKind::LocalFolder,
        source_ref,
        modified_marker: None,
        domain: body.domain.clone(),
        tags: body.tags.clone(),
        corpus_domain: None,
        metadata: std::collections::HashMap::new(),
    };

    let outcome = ingest_document(vault.store(), &dek, &raw).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    let (id, chunks_queued) = match &outcome {
        IngestOutcome::Inserted { item_id, chunks_enqueued } => (item_id.clone(), *chunks_enqueued),
        IngestOutcome::Duplicate { item_id } => (item_id.clone(), 0),
        IngestOutcome::Updated { item_id, .. } => (item_id.clone(), 0),
        IngestOutcome::Skipped { reason } => {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({"error": reason})),
            ));
        }
    };

    // 此路由只调 ingest_document（非 _replacing），outcome 只会是 Inserted/Duplicate/Skipped，
    // 不会出现 Updated。仅 Inserted 需失效 search 缓存 + 即时 FTS（搜索不等 embedding）；
    // Duplicate 数据库状态未变，无需失效。
    if matches!(outcome, IngestOutcome::Inserted { .. }) {
        if let Ok(mut cache) = state.search_cache.lock() {
            cache.clear();
        }
        let ft_guard = state.fulltext.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ft) = ft_guard.as_ref() {
            let _ = ft.add_document(&id, &body.title, &body.content, &body.source_type);
        }
    }

    Ok(Json(serde_json::json!({
        "id": id,
        "status": "ok",
        "chunks_queued": chunks_queued
    })))
}
