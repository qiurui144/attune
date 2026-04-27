use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use crate::routes::errors::{internal, vault_locked, RouteError};
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
    /// v2+: 所有批注（用户思考痕迹，个人知识库核心资产）
    /// 兼容性：v1 profile 不含此字段，import 时 `None` 即可
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Vec<serde_json::Value>>,
}

/// GET /api/v1/profile/export — 导出当前分类结果 + 聚类 + 直方图
pub async fn export(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
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
    // 批注（v2+）：每个 item 的所有批注
    let mut all_annotations = Vec::new();
    for id in &ids {
        if let Ok(anns) = vault.store().list_annotations(&dek, id) {
            for a in anns {
                if let Ok(v) = serde_json::to_value(&a) {
                    all_annotations.push(v);
                }
            }
        }
    }
    let vault_version_str = attune_core::version().to_string();
    drop(vault);

    // Histograms snapshot
    let mut histograms = std::collections::HashMap::new();
    if let Some(index) = state.tag_index.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
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
    let cluster_snapshot = state.cluster_snapshot.lock().unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|s| serde_json::to_value(s).ok());

    let item_count = tags_map.len();
    let profile = VaultProfile {
        version: 2,  // v2：包含 annotations
        exported_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs().to_string())
            .unwrap_or_else(|_| "0".to_string()),
        vault_version: vault_version_str,
        item_count,
        tags: tags_map,
        cluster_snapshot,
        histograms,
        annotations: if all_annotations.is_empty() { None } else { Some(all_annotations) },
    };

    Ok(Json(serde_json::to_value(&profile).unwrap_or(serde_json::json!({}))))
}

/// POST /api/v1/profile/import — 导入分类结果（合并，覆盖已有同 ID 条目的 tags）
pub async fn import(
    State(state): State<SharedState>,
    Json(profile): Json<VaultProfile>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !matches!(profile.version, 1 | 2) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({
            "error": format!("unsupported profile version: {} (supported: 1, 2)", profile.version)
        }))));
    }

    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
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
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db().map_err(|e| {
            (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
        })?;
        if let Ok(new_index) = attune_core::tag_index::TagIndex::build(vault.store(), &dek) {
            *state.tag_index.lock().unwrap_or_else(|e| e.into_inner()) = Some(new_index);
        }
    }

    Ok(Json(serde_json::json!({
        "status": "ok",
        "merged": merged,
        "skipped": skipped,
        "note": "skipped items are tags for item_ids not present in this vault"
    })))
}

/// GET /api/v1/profile/topic_distribution — F1 主题分布查询（W4，2026-04-27）
///
/// 简化版 F1: 按 source_type 聚合 items，返回 [(source_type, count)] 数组。
/// 给桌面"我的画像"页一个最小可用的后端 API，前端可基于此画饼图/条形图。
/// 真正的"主题分布雷达 + 知识盲区识别"需要读 classification_result.core，留 W5+ 增强。
pub async fn topic_distribution(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, RouteError> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|_| vault_locked())?;
    let pairs = vault
        .store()
        .aggregate_items_by_source_type()
        .map_err(|e| internal("aggregate_items_by_source_type", e))?;
    let total: i64 = pairs.iter().map(|(_, c)| *c).sum();
    let distribution: Vec<serde_json::Value> = pairs
        .into_iter()
        .map(|(source, count)| {
            serde_json::json!({
                "source_type": source,
                "count": count,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({
        "total_items": total,
        "distribution": distribution,
        "note": "v1 aggregates by source_type. Future versions will include classification.core dimensions.",
    })))
}
