//! GET /api/v1/folder-links — UI 读用户关联的本地知识库目录.
//!
//! 写入由 attune-cli `link-folder` 完成 (持久化在 ~/.config/npu-vault/folder-links.json).
//! 此 endpoint 只读, 给 Web UI 显示已关联列表.

use axum::Json;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderLink {
    pub project: String,
    pub folder: String,
    pub linked_at: String,
}

pub async fn list_folder_links() -> Json<serde_json::Value> {
    let p = attune_core::platform::config_dir().join("folder-links.json");
    // OPT-7: async_fs 包装 spawn_blocking, 防 Axum worker 阻塞
    let exists = attune_core::async_fs::try_exists(&p).await.unwrap_or(false);
    let links: Vec<FolderLink> = if exists {
        attune_core::async_fs::read_to_string(&p)
            .await
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    Json(serde_json::json!({
        "links": links,
        "config_path": p.to_string_lossy(),
    }))
}
