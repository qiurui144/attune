use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use crate::state::SharedState;

#[derive(Debug, Serialize, Deserialize)]
pub struct VaultProfile {
    pub version: u32,
    pub exported_at: String,
    pub vault_version: String,
    pub item_count: usize,
    /// Map of item_id → classification tags JSON
    pub tags: std::collections::HashMap<String, serde_json::Value>,
    /// Cluster snapshot (if available)
    pub cluster_snapshot: Option<serde_json::Value>,
    /// Histograms for quick preview (dimension → top values)
    pub histograms: std::collections::HashMap<String, Vec<serde_json::Value>>,
}

/// GET /api/v1/profile/export — 导出当前分类结果 + 聚类 + 直方图
pub async fn export(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap();
    let dek = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    // Read all item tags
    let ids = vault.store().list_all_item_ids()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;

    let mut tags_map = std::collections::HashMap::new();
    for id in &ids {
        if let Ok(Some(json)) = vault.store().get_tags_json(&dek, id) {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&json) {
                tags_map.insert(id.clone(), parsed);
            }
        }
    }
    let vault_version_str = vault_core::version().to_string();
    drop(vault);

    // Histograms snapshot
    let mut histograms = std::collections::HashMap::new();
    if let Some(index) = state.tag_index.lock().unwrap().as_ref() {
        for dim in index.all_dimensions() {
            if dim == "entities" { continue; }
            let hist = index.histogram(&dim);
            let values: Vec<serde_json::Value> = hist.into_iter().take(20)
                .map(|(v, c)| serde_json::json!({"value": v, "count": c}))
                .collect();
            histograms.insert(dim, values);
        }
    }

    // Cluster snapshot
    let cluster_snapshot = state.cluster_snapshot.lock().unwrap()
        .as_ref()
        .and_then(|s| serde_json::to_value(s).ok());

    let item_count = tags_map.len();
    let profile = VaultProfile {
        version: 1,
        exported_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs().to_string())
            .unwrap_or_else(|_| "0".to_string()),
        vault_version: vault_version_str,
        item_count,
        tags: tags_map,
        cluster_snapshot,
        histograms,
    };

    Ok(Json(serde_json::to_value(&profile).unwrap_or(serde_json::json!({}))))
}

/// POST /api/v1/profile/import — 导入分类结果（合并，覆盖已有同 ID 条目的 tags）
pub async fn import(
    State(state): State<SharedState>,
    Json(profile): Json<VaultProfile>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if profile.version != 1 {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({
            "error": format!("unsupported profile version: {}", profile.version)
        }))));
    }

    let vault = state.vault.lock().unwrap();
    let dek = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    let existing_ids: std::collections::HashSet<String> = vault.store()
        .list_all_item_ids()
        .unwrap_or_default()
        .into_iter()
        .collect();

    let mut merged = 0;
    let mut skipped = 0;

    for (item_id, tags_value) in &profile.tags {
        if !existing_ids.contains(item_id) {
            skipped += 1;
            continue;
        }
        let json_str = serde_json::to_string(tags_value)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))))?;
        if vault.store().update_tags(&dek, item_id, &json_str).unwrap_or(false) {
            merged += 1;
        }
    }
    drop(vault);

    // Rebuild tag index to pick up merged tags
    {
        let vault = state.vault.lock().unwrap();
        let dek = vault.dek_db().map_err(|e| {
            (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
        })?;
        if let Ok(new_index) = vault_core::tag_index::TagIndex::build(vault.store(), &dek) {
            *state.tag_index.lock().unwrap() = Some(new_index);
        }
    }

    Ok(Json(serde_json::json!({
        "status": "ok",
        "merged": merged,
        "skipped": skipped,
        "note": "skipped items are tags for item_ids not present in this vault"
    })))
}
