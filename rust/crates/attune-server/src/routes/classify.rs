use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use crate::error::{AppError, AppResult};
use crate::state::SharedState;

/// POST /api/v1/classify/{id} — 单条重分类（同步，阻塞直到完成）
pub async fn classify_one(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let classifier_arc = state.classifier.lock().unwrap_or_else(|e| e.into_inner()).as_ref().cloned();
    let classifier = match classifier_arc {
        Some(c) => c,
        None => {
            // rich error: 带 hint, 走 Detailed 保完整 body
            return Err(AppError::detailed(StatusCode::SERVICE_UNAVAILABLE, serde_json::json!({
                "error": "classification unavailable",
                "hint": "install Ollama and a local chat model suitable for your hardware"
            })));
        }
    };

    let (title, content) = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db().map_err(|e| {
            AppError::Forbidden(e.to_string())
        })?;
        match vault.store().get_item(&dek, &id) {
            Ok(Some(item)) => (item.title, item.content),
            Ok(None) => return Err(AppError::NotFound("not found".into())),
            Err(e) => return Err(AppError::Internal(e.to_string())),
        }
    };

    let result = tokio::task::spawn_blocking(move || classifier.classify_one(&title, &content))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let tags_json = serde_json::to_string(&result)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db().map_err(|e| AppError::Forbidden(e.to_string()))?;
        vault.store().update_tags(&dek, &id, &tags_json)
            .map_err(|e| AppError::Internal(e.to_string()))?;
    }

    {
        let mut tag_index = state.tag_index.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(index) = tag_index.as_mut() {
            index.upsert(&id, &result);
        }
    }

    Ok(Json(serde_json::json!({"status": "ok", "id": id, "tags": result})))
}

/// POST /api/v1/classify/rebuild — 全量重分类入队
pub async fn rebuild(
    State(state): State<SharedState>,
) -> AppResult<Json<serde_json::Value>> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|e| {
        AppError::Forbidden(e.to_string())
    })?;

    let ids = vault.store().list_all_item_ids()
        .map_err(|e| AppError::Internal(e.to_string()))?;

    for id in &ids {
        vault.store().enqueue_classify(id, 4)
            .map_err(|e| AppError::Internal(e.to_string()))?;
    }

    Ok(Json(serde_json::json!({
        "status": "ok",
        "enqueued": ids.len(),
        "note": "classify tasks enqueued with priority=4; process via /classify/{id} or manual trigger"
    })))
}

/// POST /api/v1/classify/drain — 手动处理一批待分类任务
///
/// 从 embed_queue 取出一批任务，对 classify 类型的条目调用 LLM 分类，
/// 写回 items.tags + TagIndex。非 classify 任务回到 pending 队列。
pub async fn drain(
    State(state): State<SharedState>,
) -> AppResult<Json<serde_json::Value>> {
    let state_clone = state.clone();
    let processed = tokio::task::spawn_blocking(move || state_clone.drain_classify_batch(5))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "status": "ok",
        "processed": processed
    })))
}

/// GET /api/v1/classify/status — 分类队列统计
pub async fn status(
    State(state): State<SharedState>,
) -> AppResult<Json<serde_json::Value>> {
    let pending = {
        let vault = state.vault.lock()
            .map_err(|_| AppError::Internal("vault lock poisoned".into()))?;
        let _ = vault.dek_db().map_err(|e| {
            AppError::Forbidden(e.to_string())
        })?;
        vault.store().pending_embedding_count()
            .map_err(|e| AppError::Internal(e.to_string()))?
    };
    let classifier_ready = state.classifier.lock()
        .map_err(|_| AppError::Internal("classifier lock poisoned".into()))?
        .is_some();
    let model = state.llm.lock()
        .map_err(|_| AppError::Internal("llm lock poisoned".into()))?
        .as_ref()
        .map(|l| l.model_name().to_string())
        .unwrap_or_default();
    let tag_count = state.tag_index.lock()
        .map_err(|_| AppError::Internal("tag_index lock poisoned".into()))?
        .as_ref()
        .map(|i| i.item_count())
        .unwrap_or(0);

    Ok(Json(serde_json::json!({
        "classifier_ready": classifier_ready,
        "model": model,
        "pending_tasks": pending,
        "classified_items": tag_count,
    })))
}
