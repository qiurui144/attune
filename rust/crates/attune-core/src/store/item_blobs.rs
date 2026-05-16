//! item_blobs — 原始证据文件留存（批次1-A1, 2026-05-15）。
//!
//! `items.content` 只存 OCR / 解析后的文本；律师必须能核对**原始图像 / 扫描件**
//! 才能判断 OCR 转录是否准确（变体 A 的「查看证据原文」）。此模块把上传时的
//! 原始字节按 AES-GCM 加密存入 `item_blobs` 表，并提供取回。
//!
//! 所有方法属于 `impl Store`（inherent impl 跨文件分裂，rustc 自动合并）。

use rusqlite::{params, OptionalExtension};

use crate::crypto::{self, Key32};
use crate::error::Result;
use crate::store::Store;

/// 取回的原始文件（已解密）。
#[derive(Debug, Clone)]
pub struct ItemBlob {
    pub filename: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

impl Store {
    /// 留存一份原始上传文件（AES-GCM 加密）。
    /// 同 `item_id` 重复调用覆盖（`INSERT OR REPLACE`）。
    pub fn insert_item_blob(
        &self,
        dek: &Key32,
        item_id: &str,
        filename: &str,
        mime: &str,
        data: &[u8],
    ) -> Result<()> {
        let encrypted = crypto::encrypt(dek, data)?;
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT OR REPLACE INTO item_blobs (item_id, filename, mime, blob, byte_len, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![item_id, filename, mime, encrypted, data.len() as i64, now],
        )?;
        Ok(())
    }

    /// 取回原始文件（解密）。
    /// `None` = 该 item 无留存原件（纯文本笔记 / A1 之前入库的老 item）。
    pub fn get_item_blob(&self, dek: &Key32, item_id: &str) -> Result<Option<ItemBlob>> {
        // JOIN items 并过滤 is_deleted —— 防御纵深：即便某删除路径漏删了 blob，
        // 已软删除 item 的原件也不会被取回。
        let row = self
            .conn
            .query_row(
                "SELECT b.filename, b.mime, b.blob FROM item_blobs b \
                 JOIN items i ON i.id = b.item_id \
                 WHERE b.item_id = ?1 AND i.is_deleted = 0",
                params![item_id],
                |r| {
                    let filename: String = r.get(0)?;
                    let mime: String = r.get(1)?;
                    let blob: Vec<u8> = r.get(2)?;
                    Ok((filename, mime, blob))
                },
            )
            .optional()?;
        match row {
            Some((filename, mime, encrypted)) => {
                let bytes = crypto::decrypt(dek, &encrypted)?;
                Ok(Some(ItemBlob { filename, mime, bytes }))
            }
            None => Ok(None),
        }
    }

    /// 探测该 item 是否留存了原件（不解密、不读 blob，仅看是否存在）。
    pub fn has_item_blob(&self, item_id: &str) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM item_blobs WHERE item_id = ?1",
            params![item_id],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }
}

#[cfg(test)]
mod tests {
    use crate::crypto;
    use crate::store::Store;

    #[test]
    fn insert_then_get_roundtrip() {
        let store = Store::open_memory().expect("open memory");
        let dek = crypto::Key32::generate();
        let item_id = store
            .insert_item(&dek, "借条", "OCR 文本", None, "file", None, None)
            .expect("insert item");
        let original: &[u8] = b"\x89PNG\r\n\x1a\n original evidence bytes";
        store
            .insert_item_blob(&dek, &item_id, "借条.png", "image/png", original)
            .expect("insert blob");

        assert!(store.has_item_blob(&item_id).expect("has"));
        let got = store.get_item_blob(&dek, &item_id).expect("get").expect("some");
        assert_eq!(got.filename, "借条.png");
        assert_eq!(got.mime, "image/png");
        assert_eq!(got.bytes, original.to_vec());
    }

    #[test]
    fn get_missing_returns_none() {
        let store = Store::open_memory().expect("open memory");
        let dek = crypto::Key32::generate();
        assert!(store
            .get_item_blob(&dek, "no-such-item")
            .expect("get")
            .is_none());
        assert!(!store.has_item_blob("no-such-item").expect("has"));
    }

    #[test]
    fn reinsert_overwrites() {
        let store = Store::open_memory().expect("open memory");
        let dek = crypto::Key32::generate();
        let item_id = store
            .insert_item(&dek, "t", "c", None, "file", None, None)
            .expect("insert item");
        store
            .insert_item_blob(&dek, &item_id, "v1.jpg", "image/jpeg", b"first")
            .expect("blob v1");
        store
            .insert_item_blob(&dek, &item_id, "v2.jpg", "image/jpeg", b"second")
            .expect("blob v2");
        let got = store.get_item_blob(&dek, &item_id).expect("get").expect("some");
        assert_eq!(got.bytes, b"second".to_vec());
    }
}
