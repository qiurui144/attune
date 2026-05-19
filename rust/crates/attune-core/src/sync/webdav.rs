//! WebDAV 定时备份 capability。
//!
//! v0.7 scaffold：定义 `BackupProvider` trait + `MockWebDavProvider` 实现 + 单测。
//! v0.8 真 WebDAV：用 `reqwest::Client` + PROPFIND/PUT/GET，
//! 支持 Nextcloud / Dropbox WebDAV / 大多数 NAS。
//!
//! 备份内容（v0.8 流水线）：vault 完整 sqlite + tantivy index + usearch hnsw
//! 打包成单个 tar.gz/zip，文件名 `attune-vault-<unix_ts>.tar.gz`，
//! 上传到 `remote_path` 下。`interval_sec` 由 scheduler 触发。
use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// WebDAV 服务器配置。
///
/// `interval_sec` 仅用于上层 scheduler 调度，provider 本身不读。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebDavConfig {
    /// WebDAV 端点 URL，例如 `https://nas.example.com/remote.php/dav/files/alice/`
    pub url: String,
    /// HTTP Basic Auth 用户名
    pub username: String,
    /// HTTP Basic Auth 密码 / App password
    pub password: String,
    /// 上传目标的子路径（相对 `url`），例如 `/attune-backups`
    pub remote_path: String,
    /// 自动备份周期（秒）。scheduler 据此触发 upload；0 表示禁用自动。
    pub interval_sec: u64,
}

/// 远端备份条目元信息。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupEntry {
    /// 远端文件名（不含路径）
    pub name: String,
    /// 字节大小
    pub size: u64,
    /// 远端 mtime unix 秒
    pub modified_unix: i64,
}

/// 备份 capability。
///
/// 抽象成 trait 以便：
/// - v0.7 用 Mock 跑单测 / Web UI 联调
/// - v0.8 实现 `RealWebDavProvider`
/// - 将来可加 `S3Provider` / `GdriveProvider` 共用同一接口
pub trait BackupProvider: Send + Sync {
    /// 上传 `local_path` 到远端，命名为 `remote_name`（位于 `remote_path` 下）。
    fn upload(
        &self,
        local_path: &Path,
        remote_name: &str,
    ) -> impl Future<Output = Result<()>> + Send;

    /// 列出 `remote_path` 下所有备份条目（按 mtime 倒序由调用方排）。
    fn list_backups(&self) -> impl Future<Output = Result<Vec<BackupEntry>>> + Send;

    /// 把 `remote_name` 下载到 `local_path`。
    fn download(
        &self,
        remote_name: &str,
        local_path: &Path,
    ) -> impl Future<Output = Result<()>> + Send;
}

/// 测试 mock，模拟"远端"用内存 Vec 存元数据 + 本地 fs 复制内容。
///
/// `upload(local_path, name)` 复制 local 文件到内部记录的虚拟 path，
/// `download` 反向操作。`list_backups` 返记录列表。
pub struct MockWebDavProvider {
    inner: Mutex<MockState>,
}

struct MockState {
    /// 已"上传"的备份：name → (size, modified_unix, 源文件内容快照)
    entries: Vec<(BackupEntry, Vec<u8>)>,
}

impl MockWebDavProvider {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(MockState {
                entries: Vec::new(),
            }),
        }
    }

    /// 预置一个已存在的 backup（测试 fixture）。
    pub fn with_preloaded(name: &str, data: Vec<u8>, modified_unix: i64) -> Self {
        let entry = BackupEntry {
            name: name.to_string(),
            size: data.len() as u64,
            modified_unix,
        };
        Self {
            inner: Mutex::new(MockState {
                entries: vec![(entry, data)],
            }),
        }
    }
}

impl Default for MockWebDavProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl BackupProvider for MockWebDavProvider {
    async fn upload(&self, local_path: &Path, remote_name: &str) -> Result<()> {
        let data = std::fs::read(local_path)?;
        let size = data.len() as u64;
        // 用文件 mtime 或当前时间作为 modified_unix
        let modified_unix = std::fs::metadata(local_path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let entry = BackupEntry {
            name: remote_name.to_string(),
            size,
            modified_unix,
        };
        let mut state = self.inner.lock().expect("mock webdav mutex poisoned");
        // 同名覆盖
        state.entries.retain(|(e, _)| e.name != remote_name);
        state.entries.push((entry, data));
        Ok(())
    }

    async fn list_backups(&self) -> Result<Vec<BackupEntry>> {
        let state = self.inner.lock().expect("mock webdav mutex poisoned");
        Ok(state.entries.iter().map(|(e, _)| e.clone()).collect())
    }

    async fn download(&self, remote_name: &str, local_path: &Path) -> Result<()> {
        let state = self.inner.lock().expect("mock webdav mutex poisoned");
        let (_, data) = state
            .entries
            .iter()
            .find(|(e, _)| e.name == remote_name)
            .ok_or_else(|| {
                crate::error::VaultError::Classification(format!(
                    "mock webdav: backup '{remote_name}' not found"
                ))
            })?;
        if let Some(parent) = local_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(local_path, data)?;
        Ok(())
    }
}

// 提示给后续生产实现者：`PathBuf` 是 `download` 测试用的便利类型。
#[allow(dead_code)]
fn _hint_pathbuf_is_used(_p: PathBuf) {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn mock_upload_then_list_returns_entry() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("backup.tar.gz");
        std::fs::write(&src, b"hello backup payload").unwrap();

        let p = MockWebDavProvider::new();
        p.upload(&src, "attune-vault-1715000000.tar.gz")
            .await
            .unwrap();

        let entries = p.list_backups().await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "attune-vault-1715000000.tar.gz");
        assert_eq!(entries[0].size, b"hello backup payload".len() as u64);
    }

    #[tokio::test]
    async fn mock_upload_overwrites_same_name() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("a.bin");
        std::fs::write(&src, b"v1").unwrap();
        let p = MockWebDavProvider::new();
        p.upload(&src, "snap").await.unwrap();
        std::fs::write(&src, b"v2-longer").unwrap();
        p.upload(&src, "snap").await.unwrap();
        let entries = p.list_backups().await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].size, b"v2-longer".len() as u64);
    }

    #[tokio::test]
    async fn mock_roundtrip_upload_download_matches_bytes() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("orig.bin");
        let payload: &[u8] = b"\x00\x01\x02 attune vault snapshot \xff\xfe";
        std::fs::write(&src, payload).unwrap();

        let p = MockWebDavProvider::new();
        p.upload(&src, "snap-1.bin").await.unwrap();

        let restored = tmp.path().join("restored.bin");
        p.download("snap-1.bin", &restored).await.unwrap();
        let got = std::fs::read(&restored).unwrap();
        assert_eq!(got, payload);
    }

    #[tokio::test]
    async fn mock_download_unknown_errors() {
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("x.bin");
        let p = MockWebDavProvider::new();
        let err = p.download("does-not-exist", &dest).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn mock_preloaded_fixture_listable_and_downloadable() {
        let tmp = TempDir::new().unwrap();
        let p =
            MockWebDavProvider::with_preloaded("seed.tar.gz", b"seed-data".to_vec(), 1_715_000_000);
        let entries = p.list_backups().await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].modified_unix, 1_715_000_000);

        let dest = tmp.path().join("seed.tar.gz");
        p.download("seed.tar.gz", &dest).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"seed-data");
    }

    #[test]
    fn config_roundtrips_json() {
        let cfg = WebDavConfig {
            url: "https://nas/dav/".into(),
            username: "u".into(),
            password: "p".into(),
            remote_path: "/attune".into(),
            interval_sec: 3600,
        };
        let j = serde_json::to_string(&cfg).unwrap();
        let back: WebDavConfig = serde_json::from_str(&j).unwrap();
        assert_eq!(back.url, cfg.url);
        assert_eq!(back.interval_sec, 3600);
    }
}
