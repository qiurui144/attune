use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use vault_core::chunker;

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

pub async fn ingest(
    State(state): State<SharedState>,
    Json(body): Json<IngestRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap();
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
        let mut cache = state.search_cache.lock().unwrap();
        cache.clear();
    }

    // Add to fulltext index
    {
        let ft_guard = state.fulltext.lock().unwrap();
        if let Some(ft) = ft_guard.as_ref() {
            let _ = ft.add_document(&id, &body.title, &body.content, &body.source_type);
        }
    }

    // Enqueue for embedding: two-layer indexing (sections L1 + chunks L2)
    {
        let sections = chunker::extract_sections(&body.content);
        let mut chunk_counter = 0;

        // Level 1: section-level embeddings
        for (section_idx, section_text) in &sections {
            if !section_text.trim().is_empty() {
                let _ = vault
                    .store()
                    .enqueue_embedding(&id, chunk_counter, section_text, 1, 1, *section_idx);
                chunk_counter += 1;
            }
        }

        // Level 2: paragraph chunk embeddings
        for (section_idx, section_text) in &sections {
            for chunk_text in
                chunker::chunk(section_text, chunker::DEFAULT_CHUNK_SIZE, chunker::DEFAULT_OVERLAP)
            {
                let _ = vault.store().enqueue_embedding(
                    &id,
                    chunk_counter,
                    &chunk_text,
                    2,
                    2,
                    *section_idx,
                );
                chunk_counter += 1;
            }
        }
    }

    // Auto-enqueue classification
    let _ = vault.store().enqueue_classify(&id, 3);

    Ok(Json(serde_json::json!({
        "id": id,
        "status": "ok"
    })))
}
