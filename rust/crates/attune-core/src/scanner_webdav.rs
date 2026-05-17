//! WebDAV 采集源。
//!
//! 用 reqwest_dav 列目录 + 下载，包成 WebDavConnector: SourceConnector。
//! 旧实现漏抄的 Level-2 embedding 与 enqueue_classify 缺陷，一旦走统一
//! ingest_document pipeline 就自动消失。
//! 增量去重用 ETag（不用 last_modified 字符串 —— 不同 server 时区/格式不一致）。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::crypto::Key32;
use crate::error::{Result, VaultError};
use crate::ingest::{
    ingest_document, ingest_document_replacing, DocumentSink, RawDocument, SourceConnector,
    SourceKind,
};
use crate::store::Store;

/// WebDAV 采集目录配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebDavConfig {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    /// PROPFIND depth：0=仅此资源，1=直接子项，2=两层（server 禁止 >2）。
    pub depth: u32,
}

/// scan_remote 返回给 route 层的统计摘要（JSON 响应字段保持不变）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RemoteScanResult {
    pub total_files: usize,
    pub new_files: usize,
    pub updated_files: usize,
    pub skipped_files: usize,
    pub errors: Vec<String>,
}

/// WebDAV 单文件下载大小上限（与本地 upload 一致）。
const MAX_REMOTE_FILE_BYTES: u64 = 20 * 1024 * 1024;

/// 远端受支持的扩展名（与 parser 支持集对齐的子集 — 二进制媒体不远程拉取）。
const SUPPORTED_REMOTE_EXTS: &[&str] = &[
    "md", "txt", "py", "js", "ts", "rs", "go", "java", "pdf", "docx", "html", "htm", "csv",
    "rtf", "pptx", "xlsx",
];

/// 判断文件名扩展名是否属于受支持的远端采集类型。
pub fn is_supported_remote_ext(filename: &str) -> bool {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    SUPPORTED_REMOTE_EXTS.contains(&ext.as_str())
}

/// 把 PROPFIND 返回的 href（可能是相对路径）解析成绝对 URL。
/// 已是绝对 URL 则原样返回；否则取 config.url 的 scheme://host 拼接。
pub fn resolve_href(config: &WebDavConfig, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    let mut parts = config.url.splitn(2, "://");
    let scheme = parts.next().unwrap_or("https");
    let rest = parts.next().unwrap_or_default();
    let host = rest.split('/').next().unwrap_or("");
    format!("{scheme}://{host}{href}")
}

/// 防御性 host 一致性校验：拒绝 server 返回的跨域 href（异常服务器可能在 PROPFIND
/// 响应里植入外部 host）。注意：实际下载走 reqwest_dav::Client::get(href)，
/// 它内部永远把 href 拼到 config.url 的 host 上，不受 abs 的 host 影响，
/// 所以本函数不是 SSRF 防护 —— 只是对 RawDocument.uri 写入值做一致性过滤。
fn validate_same_host(config: &WebDavConfig, abs_url: &str) -> Result<()> {
    let config_host = config
        .url
        .splitn(2, "://")
        .nth(1)
        .and_then(|s| s.split('/').next())
        .unwrap_or("");
    let fetch_host = abs_url
        .splitn(2, "://")
        .nth(1)
        .and_then(|s| s.split('/').next())
        .unwrap_or("");
    if fetch_host != config_host {
        return Err(VaultError::LlmUnavailable(format!(
            "href host '{fetch_host}' does not match config host '{config_host}'"
        )));
    }
    Ok(())
}

/// list 结果中的一项受支持远端文件（目录和不支持扩展名已过滤）。
#[derive(Debug, Clone)]
pub struct RemoteEntry {
    /// 服务器返回的 href（PROPFIND 原始值，可能是相对路径）。
    pub href: String,
    /// ETag 或 last_modified rfc3339 作为增量标记。
    pub etag: String,
    pub size: u64,
}

/// WebDAV 采集源。
pub struct WebDavConnector {
    config: WebDavConfig,
}

impl WebDavConnector {
    pub fn new(config: WebDavConfig) -> Self {
        Self { config }
    }

    /// 构造带鉴权的 reqwest_dav 客户端。
    fn build_client(&self) -> Result<reqwest_dav::Client> {
        let mut builder =
            reqwest_dav::ClientBuilder::new().set_host(self.config.url.clone());
        if let (Some(user), Some(pass)) = (&self.config.username, &self.config.password) {
            builder =
                builder.set_auth(reqwest_dav::Auth::Basic(user.clone(), pass.clone()));
        }
        builder
            .build()
            .map_err(|e| VaultError::LlmUnavailable(format!("webdav client build: {e}")))
    }

    /// 异步列出远端目录，过滤出受支持文件。
    pub async fn list(&self) -> Result<Vec<RemoteEntry>> {
        let client = self.build_client()?;
        let depth = match self.config.depth {
            0 => reqwest_dav::Depth::Number(0),
            1 => reqwest_dav::Depth::Number(1),
            _ => reqwest_dav::Depth::Infinity,
        };
        let listed = client
            .list("", depth)
            .await
            .map_err(|e| VaultError::LlmUnavailable(format!("webdav list: {e}")))?;

        let mut out = Vec::new();
        for entry in listed {
            if let reqwest_dav::list_cmd::ListEntity::File(f) = entry {
                let href = f.href.clone();
                let filename = href.rsplit('/').next().unwrap_or(&href);
                if !is_supported_remote_ext(filename) {
                    continue;
                }
                if f.content_length as u64 > MAX_REMOTE_FILE_BYTES {
                    log::warn!(
                        "webdav: skip oversized {filename} ({} bytes)",
                        f.content_length
                    );
                    continue;
                }
                // ETag 缺失时退回 last_modified rfc3339（仍优于无标记）。
                // 即便 fallback 值误判"已变"触发重入库，ingest_document 内部的
                // content_hash 短路会将结果判为 Duplicate，不会重复写入。
                let etag = f
                    .tag
                    .clone()
                    .unwrap_or_else(|| f.last_modified.to_rfc3339());
                out.push(RemoteEntry {
                    href,
                    etag,
                    size: f.content_length as u64,
                });
            }
        }
        Ok(out)
    }

    /// 异步下载单个文件字节，path 为 href（相对路径，reqwest_dav 内部拼 host）。
    pub async fn fetch(&self, href: &str) -> Result<Vec<u8>> {
        let client = self.build_client()?;
        // reqwest_dav::Client::get() 带鉴权，path 相对于 host。
        let resp = client
            .get(href)
            .await
            .map_err(|e| VaultError::LlmUnavailable(format!("webdav get {href}: {e}")))?;
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| VaultError::LlmUnavailable(format!("webdav body {href}: {e}")))?;
        // content_length 可能缺失（reqwest_dav 对缺失值返回 0 通过 list() 过滤）
        // 或被服务器谎报，下载后二次校验防止任意大文件整体读入内存。
        if bytes.len() as u64 > MAX_REMOTE_FILE_BYTES {
            return Err(VaultError::LlmUnavailable(format!(
                "webdav file too large: {} bytes (max {MAX_REMOTE_FILE_BYTES})",
                bytes.len()
            )));
        }
        Ok(bytes.to_vec())
    }

    /// 同步驱动 list + fetch，逐个把结果交给 sink。
    ///
    /// `SourceConnector::fetch_documents` 是同步契约 —— 这里用单线程 tokio
    /// runtime 桥接内部 async I/O；调用方（route / scheduler）在
    /// `spawn_blocking` 里调本方法，不阻塞外层 async runtime。
    fn drive_blocking(&self, sink: &mut DocumentSink<'_>) -> Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| VaultError::LlmUnavailable(format!("webdav runtime: {e}")))?;
        runtime.block_on(async {
            let entries = self.list().await?;
            for entry in entries {
                let abs = resolve_href(&self.config, &entry.href);
                let filename = abs.rsplit('/').next().unwrap_or(&abs).to_string();
                // 过滤跨域 href（防御 server 返回异常 PROPFIND 数据）。
                if let Err(e) = validate_same_host(&self.config, &abs) {
                    log::warn!("webdav: skip {filename} — SSRF check failed: {e}");
                    continue;
                }
                match self.fetch(&entry.href).await {
                    Ok(bytes) => {
                        let mut metadata = HashMap::new();
                        metadata.insert("etag".into(), entry.etag.clone());
                        sink(RawDocument {
                            uri: abs.clone(),
                            title: String::new(),
                            content: bytes,
                            mime_hint: None,
                            source_kind: SourceKind::WebDav,
                            // source_ref 用 href（不含 origin）—— 同一 server 内稳定唯一键。
                            source_ref: entry.href.clone(),
                            // ETag 作 modified_marker，驱动增量去重。
                            modified_marker: Some(entry.etag),
                            // WebDAV 源无来源域 / 用户标签；corpus_domain 由 route 层
                            // 从 webdav_remotes 表读出后回填（见 Task 10 / Task 11）。
                            domain: None,
                            tags: None,
                            corpus_domain: None,
                            metadata,
                        });
                    }
                    Err(e) => {
                        // 单文件下载失败不致命：记日志、继续下一个。
                        log::warn!("webdav: fetch {filename} failed: {e}");
                    }
                }
            }
            Ok::<(), VaultError>(())
        })
    }
}

impl SourceConnector for WebDavConnector {
    fn source_kind(&self) -> SourceKind {
        SourceKind::WebDav
    }

    fn fetch_documents(&self, sink: &mut DocumentSink<'_>) -> Result<()> {
        self.drive_blocking(sink)
    }
}

/// POST /api/v1/index/bind-remote 的薄壳入口（保持向后兼容）。
///
/// 用 WebDavConnector 驱动采集，每份文档走统一 ingest_document pipeline，
/// ETag dedup 逻辑内嵌在 scan_remote 自身（比对 indexed_files.file_hash）。
pub fn scan_remote(
    config: &WebDavConfig,
    store: &Store,
    dek: &Key32,
    dir_id: &str,
) -> Result<RemoteScanResult> {
    let connector = WebDavConnector::new(config.clone());

    // RefCell 让 sink closure 与 result 共享可变访问，同时满足借用检查器。
    let result = std::cell::RefCell::new(RemoteScanResult::default());

    let mut sink: DocumentSink<'_> = Box::new(|raw: RawDocument| {
        let source_ref = raw.source_ref.clone();
        let etag = raw.modified_marker.clone().unwrap_or_default();
        let filename = source_ref.rsplit('/').next().unwrap_or(&source_ref).to_string();

        result.borrow_mut().total_files += 1;

        // 增量判断：用 ETag 比对 indexed_files.file_hash。
        let existing = store.get_indexed_file(&source_ref).ok().flatten();
        if let Some(ref ex) = existing {
            if ex.file_hash == etag {
                result.borrow_mut().skipped_files += 1;
                return;
            }
        }

        // 内容已变（或首次）：有旧 item 先删旧 + enqueue purge。
        let old_item_id: Option<String> = existing.as_ref().and_then(|ex| {
            ex.item_id.as_ref().map(|id| {
                let _ = store.delete_item(id);
                if let Err(e) = store.enqueue_reindex(id, "purge") {
                    log::warn!(
                        "webdav scan: enqueue_reindex(purge) failed for {id}: {e} — orphan 风险"
                    );
                }
                if let Err(e) = store.record_signal_event("doc_update", id, None) {
                    log::debug!(
                        "webdav scan: record_signal_event failed for {id}: {e} (non-fatal)"
                    );
                }
                id.clone()
            })
        });

        // 走统一 ingest_document pipeline（含 L2 embedding + classify）。
        let outcome = if let Some(ref old_id) = old_item_id {
            ingest_document_replacing(store, dek, &raw, old_id)
        } else {
            ingest_document(store, dek, &raw)
        };

        match outcome {
            Ok(crate::ingest::IngestOutcome::Inserted { item_id, .. }) => {
                // 记录 indexed_files，下次增量用 ETag 短路。
                let _ = store.upsert_indexed_file(dir_id, &source_ref, &etag, &item_id);
                let mut r = result.borrow_mut();
                if old_item_id.is_some() {
                    r.updated_files += 1;
                } else {
                    r.new_files += 1;
                }
            }
            Ok(crate::ingest::IngestOutcome::Updated { item_id, .. }) => {
                let _ = store.upsert_indexed_file(dir_id, &source_ref, &etag, &item_id);
                result.borrow_mut().updated_files += 1;
            }
            Ok(crate::ingest::IngestOutcome::Duplicate { .. }) => {
                result.borrow_mut().skipped_files += 1;
            }
            Ok(crate::ingest::IngestOutcome::Skipped { .. }) => {
                result.borrow_mut().skipped_files += 1;
            }
            Err(e) => {
                result.borrow_mut().errors.push(format!("{filename}: ingest {e}"));
            }
        }
    });

    connector.fetch_documents(&mut sink)?;
    // sink 在此 drop，RefCell 独占引用释放，into_inner 安全。
    drop(sink);
    Ok(result.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_href_absolute_passthrough() {
        let cfg = WebDavConfig {
            url: "https://dav.example.com/remote.php/dav/files/u/".into(),
            username: None,
            password: None,
            depth: 1,
        };
        let abs = "https://dav.example.com/remote.php/dav/files/u/notes.md";
        assert_eq!(resolve_href(&cfg, abs), abs);
    }

    #[test]
    fn resolve_href_relative_joins_origin() {
        let cfg = WebDavConfig {
            url: "https://dav.example.com/remote.php/dav/files/u/".into(),
            username: None,
            password: None,
            depth: 1,
        };
        assert_eq!(
            resolve_href(&cfg, "/remote.php/dav/files/u/notes.md"),
            "https://dav.example.com/remote.php/dav/files/u/notes.md"
        );
    }

    #[test]
    fn supported_ext_filters_binaries() {
        assert!(is_supported_remote_ext("notes.md"));
        assert!(is_supported_remote_ext("report.pdf"));
        assert!(!is_supported_remote_ext("movie.mp4"));
        assert!(!is_supported_remote_ext("archive.zip"));
    }
}
