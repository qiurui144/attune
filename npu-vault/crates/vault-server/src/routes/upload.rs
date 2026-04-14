use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::Json;

use crate::state::SharedState;
use vault_core::{chunker, parser};

const MAX_UPLOAD_BYTES: usize = 20 * 1024 * 1024; // 20 MB

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
    let vault = state.vault.lock().unwrap();
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
        let ft_guard = state.fulltext.lock().unwrap();
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

    Ok(Json(serde_json::json!({
        "id": item_id,
        "title": title,
        "chunks_queued": chunk_counter,
        "status": "processing"
    })))
}
