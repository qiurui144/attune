use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::state::{BackgroundScanTaskStatus, SharedState};
use attune_core::crypto::Key32;
use attune_core::ingest::IngestOptions;
use attune_core::scanner;
use attune_core::store::Store;

#[derive(Deserialize)]
pub struct BindRequest {
    pub path: String,
    #[serde(default = "default_true")]
    pub recursive: bool,
    #[serde(default = "default_file_types")]
    pub file_types: Vec<String>,
    /// v0.6 Phase B F-Pro：bind 时声明 corpus 领域用于跨域 retrieval 防污染。
    /// 'legal' / 'tech' / 'medical' / 'patent' / 'academic' / 'general'(默认)。
    #[serde(default = "default_corpus_domain")]
    pub corpus_domain: String,
    /// If true, return after binding the directory row and run the expensive
    /// scan/parse/enqueue path in a background worker. Existing API callers keep
    /// the synchronous behavior by omitting this field.
    #[serde(default, alias = "async_scan")]
    pub background: bool,
}

#[derive(Deserialize)]
pub struct RescanRequest {
    pub dir_id: String,
    #[serde(default, alias = "async_scan")]
    pub background: bool,
}

fn default_corpus_domain() -> String {
    "general".to_string()
}

fn default_true() -> bool {
    true
}

fn default_file_types() -> Vec<String> {
    vec![
        "md".into(),
        "txt".into(),
        "py".into(),
        "js".into(),
        "rs".into(),
    ]
}

#[derive(Deserialize)]
pub struct UnbindQuery {
    pub dir_id: String,
}

/// Validates that a raw path string is:
/// 1. An absolute path
/// 2. Exists and is a directory (via canonicalization)
/// 3. Within the user's home directory
///
/// Returns the canonicalized PathBuf on success.
pub fn validate_bind_path(
    raw: &str,
    home: &std::path::Path,
) -> Result<std::path::PathBuf, (StatusCode, Json<serde_json::Value>)> {
    let path = std::path::Path::new(raw);

    if !path.is_absolute() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "path must be absolute"})),
        ));
    }

    // dunce::canonicalize == std::fs::canonicalize 在 Unix 上完全等价,
    // 在 Windows 上自动剥 \\?\ UNC 前缀, 让后续 starts_with(home) 正常工作.
    // 不剥 UNC 会导致 Windows 用户连 home 目录本身都加不进 vault (canonical
    // 是 \\?\C:\Users\xxx, home 参数是 C:\Users\xxx, starts_with 失败).
    let canonical = dunce::canonicalize(path).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "directory not found or inaccessible"})),
        )
    })?;

    if !canonical.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "path is not a directory"})),
        ));
    }

    if !canonical.starts_with(home) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "path must be within the user home directory",
                "home": home.display().to_string(),
            })),
        ));
    }

    Ok(canonical)
}

pub async fn bind_directory(
    State(state): State<SharedState>,
    Json(body): Json<BindRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let ingest_options = crate::local_scheduler::ingest_options_from_state(&state, None);
    let ingest_options = if body.background {
        ingest_options.with_background_ingest_ocr()
    } else {
        ingest_options
    };
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db().map_err(|e| {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    let home = dirs::home_dir().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "internal error"})),
        )
    })?;
    let canonical = validate_bind_path(&body.path, &home)?;

    // 使用规范化后的路径字符串
    let canonical_str = canonical.display().to_string();

    let file_type_strs: Vec<&str> = body.file_types.iter().map(|s| s.as_str()).collect();
    let dir_id = vault
        .store()
        .bind_directory_with_domain(
            &canonical_str,
            body.recursive,
            &file_type_strs,
            &body.corpus_domain,
        )
        .map_err(|e| {
            // 错误信息脱敏: Rust/SQLite 原文不能直接给用户看
            let msg = e.to_string();
            let user_msg = if msg.contains("FOREIGN KEY") || msg.contains("constraint failed") {
                "添加目录失败：数据状态异常，请尝试在「设置 → 数据」中先解绑同名目录后重试"
                    .to_string()
            } else if msg.contains("UNIQUE") {
                "该目录已绑定，无需重复添加".to_string()
            } else if msg.contains("locked") || msg.contains("Locked") {
                "本地数据已锁定，请先解锁".to_string()
            } else {
                "添加目录失败，请稍后重试".to_string()
            };
            tracing::error!(target: "access", "bind_directory failed for {canonical_str}: {msg}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": user_msg})),
            )
        })?;

    if body.background {
        drop(vault);
        spawn_background_bind_scan(
            state.clone(),
            dek,
            dir_id.clone(),
            canonical.clone(),
            body.recursive,
            body.file_types.clone(),
            ingest_options,
        );
        return Ok(Json(serde_json::json!({
            "status": "accepted",
            "background": true,
            "dir_id": dir_id,
            "scan": {
                "status": "queued",
            }
        })));
    }

    // Scan directory synchronously for compatibility with API/e2e callers that
    // need immediate scan counts in the response.
    let scan_result = scanner::scan_directory_with_options(
        vault.store(),
        &dek,
        &dir_id,
        &canonical,
        body.recursive,
        &body.file_types,
        &ingest_options,
    )
    .map_err(|e| scan_error_response(&canonical_str, e))?;

    // 释放 bind/scan 阶段持有的 vault，再以规范锁序 fulltext → vault 重取做 FTS
    // rebuild。绝不在持 vault 时取 fulltext（那会反转 fulltext → vectors → vault
    // 规范序，与 search/chat 热点路径冲突 = ABBA 死锁）。dek 是 owned Key32，drop
    // vault 后仍可用。
    drop(vault);
    // #83 P0: 分页 FTS 增量刷新，每页单独加释放 vault lock。
    // 锁序维持 fulltext → vault（正确；ft_guard 外层，vault 内层），
    // 且持锁期限从"全量"缩为每页 500 条。
    rebuild_fulltext_from_vault(&state, &dek);

    Ok(Json(scan_result_payload(&dir_id, &scan_result)))
}

pub async fn rescan_directory(
    State(state): State<SharedState>,
    Json(body): Json<RescanRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let ingest_options = crate::local_scheduler::ingest_options_from_state(&state, None);
    let ingest_options = if body.background {
        ingest_options.with_background_ingest_ocr()
    } else {
        ingest_options
    };

    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let dek = vault.dek_db().map_err(|e| {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;
    let dir = vault
        .store()
        .list_bound_directories()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?
        .into_iter()
        .find(|dir| dir.id == body.dir_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "bound directory not found"})),
            )
        })?;
    if dir.path.starts_with("webdav:")
        || dir.path.starts_with("git:")
        || dir.path.starts_with("email:")
        || dir.path.starts_with("rss:")
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "rescan is only supported for local directories"})),
        ));
    }
    let canonical = PathBuf::from(&dir.path);
    if !canonical.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "bound directory not found or inaccessible"})),
        ));
    }
    let file_types = dir.file_type_list();

    if body.background {
        let dir_id = dir.id.clone();
        drop(vault);
        spawn_background_bind_scan(
            state.clone(),
            dek,
            dir_id.clone(),
            canonical,
            dir.recursive,
            file_types,
            ingest_options,
        );
        return Ok(Json(serde_json::json!({
            "status": "accepted",
            "background": true,
            "dir_id": dir_id,
            "scan": {
                "status": "queued",
            }
        })));
    }

    let scan_result = scanner::scan_directory_with_options(
        vault.store(),
        &dek,
        &dir.id,
        &canonical,
        dir.recursive,
        &file_types,
        &ingest_options,
    )
    .map_err(|e| scan_error_response(&dir.path, e))?;
    let dir_id = dir.id.clone();
    drop(vault);
    rebuild_fulltext_from_vault(&state, &dek);

    Ok(Json(scan_result_payload(&dir_id, &scan_result)))
}

fn scan_result_payload(dir_id: &str, scan_result: &scanner::ScanResult) -> serde_json::Value {
    serde_json::json!({
        "status": "ok",
        "dir_id": dir_id,
        "scan": {
            "total": scan_result.total_files,
            "new": scan_result.new_files,
            "updated": scan_result.updated_files,
            "skipped": scan_result.skipped_files,
            "deleted": scan_result.deleted_files,
            "degraded": scan_result.degraded_files,
        }
    })
}

fn scan_error_response(
    canonical_str: &str,
    e: attune_core::error::VaultError,
) -> (StatusCode, Json<serde_json::Value>) {
    let msg = e.to_string();
    tracing::error!(target: "access", "scan_directory failed for {canonical_str}: {msg}");
    let user_msg = if msg.contains("Permission denied") {
        "无法读取该目录：请检查访问权限".to_string()
    } else {
        "扫描目录失败，请稍后重试".to_string()
    };
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": user_msg})),
    )
}

fn spawn_background_bind_scan(
    state: SharedState,
    dek: Key32,
    dir_id: String,
    canonical: PathBuf,
    recursive: bool,
    file_types: Vec<String>,
    ingest_options: IngestOptions,
) {
    let task_id = format!("bind-scan-{dir_id}");
    let path = canonical.display().to_string();
    record_background_scan_status(
        &state,
        BackgroundScanTaskStatus {
            task_id: task_id.clone(),
            dir_id: dir_id.clone(),
            path: path.clone(),
            status: "running".to_string(),
            progress: 0.05,
            message: format!("正在后台扫描 {path}"),
            total: None,
            new: None,
            updated: None,
            skipped: None,
            deleted: None,
            degraded: None,
            errors: None,
            elapsed_ms: None,
        },
    );
    send_background_scan_progress(
        &state,
        &task_id,
        "running",
        0.05,
        &format!("正在后台扫描 {}", canonical.display()),
    );
    tokio::task::spawn_blocking(move || {
        let started = std::time::Instant::now();
        match run_background_bind_scan(
            &state,
            &dek,
            &dir_id,
            &canonical,
            recursive,
            &file_types,
            &ingest_options,
        ) {
            Ok(scan) => {
                let elapsed_ms = started.elapsed().as_millis();
                record_background_scan_status(
                    &state,
                    BackgroundScanTaskStatus {
                        task_id: task_id.clone(),
                        dir_id: dir_id.clone(),
                        path: canonical.display().to_string(),
                        status: "done".to_string(),
                        progress: 1.0,
                        message: format!(
                            "后台索引完成：{} 个文件，{} 新增，{} 更新，{} 跳过，{} 删除",
                            scan.total_files,
                            scan.new_files,
                            scan.updated_files,
                            scan.skipped_files,
                            scan.deleted_files
                        ),
                        total: Some(scan.total_files),
                        new: Some(scan.new_files),
                        updated: Some(scan.updated_files),
                        skipped: Some(scan.skipped_files),
                        deleted: Some(scan.deleted_files),
                        degraded: Some(scan.degraded_files),
                        errors: Some(scan.errors),
                        elapsed_ms: Some(elapsed_ms),
                    },
                );
                tracing::info!(
                    target: "access",
                    "background bind scan completed dir_id={dir_id} path={} total={} new={} updated={} skipped={} deleted={} degraded={} errors={} elapsed_ms={elapsed_ms}",
                    canonical.display(),
                    scan.total_files,
                    scan.new_files,
                    scan.updated_files,
                    scan.skipped_files,
                    scan.deleted_files,
                    scan.degraded_files,
                    scan.errors,
                );
                send_background_scan_progress(
                    &state,
                    &task_id,
                    "done",
                    1.0,
                    &format!(
                        "后台索引完成：{} 个文件，{} 新增，{} 更新，{} 跳过，{} 删除",
                        scan.total_files,
                        scan.new_files,
                        scan.updated_files,
                        scan.skipped_files,
                        scan.deleted_files
                    ),
                );
            }
            Err(e) => {
                record_background_scan_status(
                    &state,
                    BackgroundScanTaskStatus {
                        task_id: task_id.clone(),
                        dir_id: dir_id.clone(),
                        path: canonical.display().to_string(),
                        status: "failed".to_string(),
                        progress: 1.0,
                        message: format!("后台扫描失败：{e}"),
                        total: None,
                        new: None,
                        updated: None,
                    skipped: None,
                    deleted: None,
                    degraded: None,
                    errors: None,
                        elapsed_ms: Some(started.elapsed().as_millis()),
                    },
                );
                tracing::error!(
                    target: "access",
                    "background bind scan failed dir_id={dir_id} path={}: {e}",
                    canonical.display(),
                );
                send_background_scan_progress(
                    &state,
                    &task_id,
                    "failed",
                    1.0,
                    &format!("后台扫描失败：{e}"),
                );
            }
        }
    });
}

fn record_background_scan_status(state: &SharedState, status: BackgroundScanTaskStatus) {
    let mut tasks = state
        .background_scan_tasks
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if tasks.len() > 128 {
        tasks.retain(|_, task| task.status == "running");
    }
    tasks.insert(status.task_id.clone(), status);
}

fn run_background_bind_scan(
    state: &SharedState,
    dek: &Key32,
    dir_id: &str,
    canonical: &Path,
    recursive: bool,
    file_types: &[String],
    ingest_options: &IngestOptions,
) -> Result<scanner::ScanResult, String> {
    let db_path = attune_core::platform::db_path();
    let store = Store::open(&db_path).map_err(|e| format!("open store: {e}"))?;
    let scan = scanner::scan_directory_with_options(
        &store,
        dek,
        dir_id,
        canonical,
        recursive,
        file_types,
        ingest_options,
    )
    .map_err(|e| e.to_string())?;
    rebuild_fulltext_from_store(state, &store, dek);
    Ok(scan)
}

fn rebuild_fulltext_from_vault(state: &SharedState, dek: &Key32) {
    let ft_guard = state.fulltext.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ft) = ft_guard.as_ref() {
        const FTS_PAGE: usize = 500;
        let mut fts_offset = 0usize;
        loop {
            let page_items: Vec<(String, String, String, String)> = {
                let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
                match vault.store().list_item_ids_paged(fts_offset, FTS_PAGE) {
                    Ok(ids) => {
                        let mut buf = Vec::with_capacity(ids.len());
                        for id in &ids {
                            if let Ok(Some(item)) = vault.store().get_item(dek, id) {
                                buf.push((item.id, item.title, item.content, item.source_type));
                            }
                        }
                        buf
                    }
                    Err(_) => break,
                }
            }; // vault lock released here
            let n = page_items.len();
            for (id, title, content, source_type) in &page_items {
                let _ = ft.add_document(id, title, content, source_type);
            }
            fts_offset += FTS_PAGE;
            if n < FTS_PAGE {
                break;
            }
        }
    }
}

fn rebuild_fulltext_from_store(state: &SharedState, store: &Store, dek: &Key32) {
    let ft_guard = state.fulltext.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ft) = ft_guard.as_ref() {
        const FTS_PAGE: usize = 500;
        let mut fts_offset = 0usize;
        loop {
            let ids = match store.list_item_ids_paged(fts_offset, FTS_PAGE) {
                Ok(ids) if !ids.is_empty() => ids,
                _ => break,
            };
            let mut page_items = Vec::with_capacity(ids.len());
            for id in &ids {
                if let Ok(Some(item)) = store.get_item(dek, id) {
                    page_items.push((item.id, item.title, item.content, item.source_type));
                }
            }
            let n = page_items.len();
            for (id, title, content, source_type) in &page_items {
                let _ = ft.add_document(id, title, content, source_type);
            }
            fts_offset += FTS_PAGE;
            if n < FTS_PAGE {
                break;
            }
        }
    }
}

fn send_background_scan_progress(
    state: &SharedState,
    task_id: &str,
    status: &str,
    progress: f32,
    message: &str,
) {
    let _ = state.recommendation_tx.send(serde_json::json!({
        "type": "progress",
        "task_id": task_id,
        "status": status,
        "progress": progress,
        "message": message,
    }));
}

pub async fn unbind_directory(
    State(state): State<SharedState>,
    Query(params): Query<UnbindQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|e| {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    vault
        .store()
        .unbind_directory(&params.dir_id)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?;

    Ok(Json(serde_json::json!({"status": "ok"})))
}

pub async fn index_status(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
    let _ = vault.dek_db().map_err(|e| {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    let dirs = vault.store().list_bound_directories().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;
    let pending = vault.store().pending_embedding_count().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;
    let background_scans: Vec<_> = state
        .background_scan_tasks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .cloned()
        .collect();

    Ok(Json(serde_json::json!({
        "directories": dirs,
        "pending_embeddings": pending,
        "background_scans": background_scans,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_result_payload_includes_deleted_count_for_nas_web() {
        let payload = scan_result_payload(
            "dir-1",
            &scanner::ScanResult {
                total_files: 3,
                new_files: 1,
                updated_files: 0,
                skipped_files: 1,
                deleted_files: 1,
                degraded_files: 0,
                errors: 0,
            },
        );

        assert_eq!(payload["dir_id"], "dir-1");
        assert_eq!(payload["scan"]["deleted"], 1);
    }
}
