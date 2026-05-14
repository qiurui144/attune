//! Async-safe filesystem helpers — 用 tokio::task::spawn_blocking 包装 std::fs.
//!
//! 背景: Axum / Tokio async runtime 的 worker thread 数量有限 (默认 = CPU 核数).
//! 在 async handler 内直接调 std::fs::read / fs::write 会阻塞 worker, 导致其他
//! 并发请求积压. 真正可移植的修法是把 fs 操作放到 spawn_blocking 池 (默认 512
//! 线程, 独立于 runtime worker).
//!
//! 何时用本模块:
//! - async fn 内需要读写文件 (路由 / wizard / config save / model load)
//! - 不知道当前是不是 async 上下文 (库代码) — 安全起见统一走 spawn_blocking
//!
//! 何时不用 (直接 std::fs 即可):
//! - sync 上下文 (CLI / 启动初始化 / 测试 setup)
//! - long-running worker thread (queue worker / scanner) — 已在 std::thread::spawn 里, 不会阻塞 tokio
//!
//! D3 ARCH review 发现 92 处 std::fs in src/, 多数已经在合理 sync 上下文; 但
//! future 新代码默认应走 async_fs, 防止 future async handler 误调用 sync fs.

use std::path::{Path, PathBuf};
use tokio::task::spawn_blocking;

/// 异步读文件为 `Vec<u8>`. 错误用 std::io::Error 直接透出.
pub async fn read<P: AsRef<Path>>(path: P) -> std::io::Result<Vec<u8>> {
    let path = path.as_ref().to_path_buf();
    spawn_blocking(move || std::fs::read(path))
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("spawn_blocking: {e}")))?
}

/// 异步读文件为 `String`. 自动处理 BOM + UTF-8 校验.
pub async fn read_to_string<P: AsRef<Path>>(path: P) -> std::io::Result<String> {
    let path = path.as_ref().to_path_buf();
    spawn_blocking(move || std::fs::read_to_string(path))
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("spawn_blocking: {e}")))?
}

/// 异步写文件 (覆盖). 调用方负责保证 path 父目录存在.
pub async fn write<P: AsRef<Path>>(path: P, content: Vec<u8>) -> std::io::Result<()> {
    let path = path.as_ref().to_path_buf();
    spawn_blocking(move || std::fs::write(path, content))
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("spawn_blocking: {e}")))?
}

/// 异步创建目录 (含 mkdir -p 语义).
pub async fn create_dir_all<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
    let path = path.as_ref().to_path_buf();
    spawn_blocking(move || std::fs::create_dir_all(path))
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("spawn_blocking: {e}")))?
}

/// 异步删除文件 (不存在不报错, 类似 rm -f).
pub async fn remove_file_if_exists<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
    let path: PathBuf = path.as_ref().to_path_buf();
    spawn_blocking(move || match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    })
    .await
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("spawn_blocking: {e}")))?
}

/// 异步检查 path 是否存在 (canonicalize 友好, 不解析 symlink).
pub async fn try_exists<P: AsRef<Path>>(path: P) -> std::io::Result<bool> {
    let path = path.as_ref().to_path_buf();
    spawn_blocking(move || path.try_exists())
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("spawn_blocking: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn read_write_round_trip() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("a.txt");
        write(&p, b"hello attune".to_vec()).await.unwrap();
        let s = read_to_string(&p).await.unwrap();
        assert_eq!(s, "hello attune");
    }

    #[tokio::test]
    async fn create_dir_all_recursive() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("a/b/c");
        create_dir_all(&nested).await.unwrap();
        assert!(nested.exists());
    }

    #[tokio::test]
    async fn remove_file_if_exists_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("nonexistent.txt");
        // 第一次调用: 文件不存在, 不报错
        remove_file_if_exists(&p).await.unwrap();
        // 创建后再删
        write(&p, b"x".to_vec()).await.unwrap();
        assert!(p.exists());
        remove_file_if_exists(&p).await.unwrap();
        assert!(!p.exists());
        // 再删一次, 仍不报错
        remove_file_if_exists(&p).await.unwrap();
    }

    #[tokio::test]
    async fn try_exists_returns_correct_bool() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("x.txt");
        assert!(!try_exists(&p).await.unwrap());
        write(&p, b"y".to_vec()).await.unwrap();
        assert!(try_exists(&p).await.unwrap());
    }
}
