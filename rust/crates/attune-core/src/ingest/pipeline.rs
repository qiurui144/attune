//! 统一入库 pipeline。实质实现见 Task 2。

/// 入库结果。实质定义见 Task 2。
pub enum IngestOutcome {
    /// 新文档，已入库。
    Inserted { item_id: String },
    /// 内容未变（`modified_marker` 命中），跳过。
    Skipped,
    /// 文档已存在但内容已更新，已重新入库。
    Updated { item_id: String },
}

/// 将一份 `RawDocument` 走完 parse → 判重 → insert → breadcrumbs →
/// embed(L1+L2) → classify 五步并返回结果。实质实现见 Task 2。
pub fn ingest_document(
    _doc: super::RawDocument,
    _store: &crate::store::Store,
    _dek: &crate::crypto::Key32,
) -> crate::error::Result<IngestOutcome> {
    unimplemented!("ingest_document: 实质实现在 Task 2")
}
