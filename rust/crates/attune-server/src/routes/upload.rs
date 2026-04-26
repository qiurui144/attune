use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::Json;

use crate::state::SharedState;
use attune_core::{chunker, parser};

/// Upload size cap. 提到 100 MB：扫描版 PDF 常在 30-80MB，整本 OCR 很合理。
/// 超过此值通常是高清扫描+彩图，建议用户预处理（pdftoppm 降 DPI、jpeg 压缩）。
///
/// ⚠ **必须与 `lib.rs` 中 `/api/v1/upload` 路由的 `DefaultBodyLimit::max(...)` 同步修改。**
/// 框架层限制早于此检查触发（在 multipart 解码前拦截），此处检查是第二道防线，
/// 防止 DefaultBodyLimit 被删除或未生效时的 OOM。两处写不一致会产生误导性行为。
const MAX_UPLOAD_BYTES: usize = 100 * 1024 * 1024; // 100 MB

pub async fn upload_file(
    State(state): State<SharedState>,
    mut multipart: Multipart,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // First, read multipart data without holding any locks
    let (filename, data) = {
        let field = multipart
            .next_field()
            .await
            .map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": e.to_string()})),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "no file provided"})),
                )
            })?;

        let filename = field.file_name().unwrap_or("unknown").to_string();
        let data = field.bytes().await.map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?;
        (filename, data)
    };

    if data.len() > MAX_UPLOAD_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({
                "error": format!("file too large: {} bytes (max {})", data.len(), MAX_UPLOAD_BYTES)
            })),
        ));
    }

    let (title, content) = parser::parse_bytes(&data, &filename).map_err(|e| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    if content.trim().is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({"error": "empty content"})),
        ));
    }

    // Now lock vault for DB operations (no more await points after this)
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db().map_err(|e| {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    let item_id = vault
        .store()
        .insert_item(&dek, &title, &content, None, "file", None, None)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?;

    // Add to fulltext index immediately (search works without AI)
    {
        let ft_guard = state.fulltext.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ft) = ft_guard.as_ref() {
            let _ = ft.add_document(&item_id, &title, &content, "file");
        }
    }

    // Enqueue for embedding: Level 1 (sections) + Level 2 (chunks)
    let sections = chunker::extract_sections(&content);
    let mut chunk_counter: usize = 0;

    for (section_idx, section_text) in &sections {
        if !section_text.trim().is_empty() {
            vault
                .store()
                .enqueue_embedding(&item_id, chunk_counter, section_text, 1, 1, *section_idx)
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({"error": e.to_string()})),
                    )
                })?;
            chunk_counter += 1;
        }
    }
    for (section_idx, section_text) in &sections {
        for chunk_text in chunker::chunk(section_text, chunker::DEFAULT_CHUNK_SIZE, chunker::DEFAULT_OVERLAP) {
            vault
                .store()
                .enqueue_embedding(&item_id, chunk_counter, &chunk_text, 2, 2, *section_idx)
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({"error": e.to_string()})),
                    )
                })?;
            chunk_counter += 1;
        }
    }

    // Auto-enqueue classification
    let _ = vault.store().enqueue_classify(&item_id, 3);

    // 释放 vault guard，让后续 spawn task 能独立 lock vault
    drop(vault);

    // Sprint 1 Phase B: 异步跑 ProjectRecommender，命中阈值通过 ws 推送
    {
        let title_clone = title.clone();
        let item_id_clone = item_id.clone();
        let state_clone = state.clone();
        tokio::spawn(async move {
            let vault_guard = state_clone.vault.lock();
            let vault_guard = vault_guard.unwrap_or_else(|e| e.into_inner());
            if !matches!(vault_guard.state(), attune_core::vault::VaultState::Unlocked) {
                return;
            }
            // 抽 entities — 简化：用 title 当样本（chunk-level entities 可在 Phase D 优化）
            let new_ents = attune_core::entities::extract_entities(&title_clone);
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

    // 行业 workflow trigger（如 law-pro/evidence_chain_inference）由 attune-pro 在
    // Sprint 2 plugin loader 注册到运行时 trigger map，不在 attune-core/server 内置。

    Ok(Json(serde_json::json!({
        "id": item_id,
        "title": title,
        "chunks_queued": chunk_counter,
        "status": "processing"
    })))
}
