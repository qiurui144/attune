use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use crate::error::{AppError, AppResult};
use crate::state::SharedState;

/// GET /api/v1/clusters — 当前聚类快照
pub async fn list(
    State(state): State<SharedState>,
) -> AppResult<Json<serde_json::Value>> {
    let snapshot = state.cluster_snapshot.lock()
        .map_err(|_| AppError::Internal("lock poisoned".into()))?
        .clone();
    match snapshot {
        Some(s) => {
            let val = serde_json::to_value(&s)
                .map_err(|e| AppError::Internal(e.to_string()))?;
            Ok(Json(val))
        }
        None => Ok(Json(serde_json::json!({
            "clusters": [],
            "note": "no cluster snapshot yet, POST /clusters/rebuild to generate"
        }))),
    }
}

/// GET /api/v1/clusters/{id} — 某聚类详情
pub async fn detail(
    State(state): State<SharedState>,
    Path(id): Path<i32>,
) -> AppResult<Json<serde_json::Value>> {
    let snapshot = state.cluster_snapshot.lock()
        .map_err(|_| AppError::Internal("lock poisoned".into()))?;
    match snapshot.as_ref() {
        Some(s) => {
            match s.clusters.iter().find(|c| c.id == id) {
                Some(c) => {
                    let val = serde_json::to_value(c)
                        .map_err(|e| AppError::Internal(e.to_string()))?;
                    Ok(Json(val))
                }
                None => Err(AppError::NotFound("cluster not found".into())),
            }
        }
        None => Err(AppError::NotFound("no snapshot".into())),
    }
}

/// POST /api/v1/clusters/rebuild — 手动触发聚类
///
/// 流程：
///   1. 枚举所有未删除 item
///   2. 从 vector index 取每个 item 的均值向量（未 embed 完成的跳过）
///   3. HDBSCAN 聚类（需要 >= 20 个有向量的 item，由 Clusterer::min_items 控制）
///   4. LLM 为每个簇生成 name + summary
///   5. 写入 state.cluster_snapshot
pub async fn rebuild(
    State(state): State<SharedState>,
) -> AppResult<Json<serde_json::Value>> {
    use attune_core::clusterer::{Clusterer, ClusterInput};

    // 取 LLM（聚类命名依赖 LLM）
    let llm = state.llm.lock()
        .map_err(|_| AppError::Internal("llm lock".into()))?
        .as_ref().cloned();
    let llm = match llm {
        Some(l) => l,
        // rich error: 带 hint 字段, 走 Detailed 保完整 body
        None => return Err(AppError::detailed(StatusCode::SERVICE_UNAVAILABLE, serde_json::json!({
            "error": "LLM 不可用，无法为聚类命名",
            "hint": "请确保 Ollama 已安装并拉取 chat 模型"
        }))),
    };

    // 1. 取所有 item IDs
    let (ids, dek) = {
        let vault = state.vault.lock()
            .map_err(|_| AppError::Internal("vault lock".into()))?;
        let dek = vault
            .dek_db()
            .map_err(|e| AppError::Forbidden(e.to_string()))?;
        let ids = vault.store().list_all_item_ids()
            .map_err(|e| AppError::Internal(e.to_string()))?;
        (ids, dek)
    };

    // 2. 构建 ClusterInput：逐 item 取均值向量
    let mut inputs: Vec<ClusterInput> = Vec::with_capacity(ids.len());
    let mut missing_vec = 0usize;
    {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let vecs = state.vectors.lock().unwrap_or_else(|e| e.into_inner());
        let vecs_ref = vecs.as_ref();
        for id in &ids {
            let embedding = match vecs_ref.and_then(|v| v.get_vector(id)) {
                Some(v) => v,
                None => { missing_vec += 1; continue; }
            };
            if let Ok(Some(item)) = vault.store().get_item(&dek, id) {
                let snippet: String = item.content.chars().take(200).collect();
                inputs.push(ClusterInput {
                    item_id: item.id,
                    title: item.title,
                    content_snippet: snippet,
                    embedding,
                });
            }
        }
    }

    // 3. 跑聚类（heavy, spawn_blocking 避免阻塞 runtime）
    // HDBSCAN 默认 min_cluster_size=5，给向量少于 10 时会 panic out-of-bounds；
    // 安全起见 min_items=10，少于此直接返回空 snapshot。
    let clusterer = Clusterer::new(llm).with_min_items(10);
    let snapshot = tokio::task::spawn_blocking(move || clusterer.rebuild(inputs))
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))?
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let cluster_count = snapshot.clusters.len();
    let noise_count = snapshot.noise_item_ids.len();

    // 4. 写入 state
    *state.cluster_snapshot.lock().unwrap_or_else(|e| e.into_inner()) = Some(snapshot);

    Ok(Json(serde_json::json!({
        "status": "ok",
        "total_items": ids.len(),
        "items_with_vectors": ids.len() - missing_vec,
        "missing_vectors": missing_vec,
        "clusters": cluster_count,
        "noise_items": noise_count,
    })))
}
