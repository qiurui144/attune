//! memory/migration — reindex 单步:换 embedding 模型后,把一条记忆的向量
//! 从旧 model 重算为当前 model(解密 summary → re-embed → REPLACE)。
//!
//! WHY 原地 REPLACE 而非新旧并存:`put_memory_vector` 是 INSERT OR REPLACE 单行,
//! spec v2 据此设计 —— 迁移逐条把旧维度键向量替换为当前键,无并存歧义,
//! assembler 的 dim-guard 自动跳过尚未迁移的旧向量。
//!
//! 见 `docs/superpowers/plans/2026-06-15-memory-continuity-and-portability.md` Task 3。

use crate::crypto::Key32;
use crate::embed::EmbeddingProvider;
use crate::error::Result;
use crate::store::Store;

/// 重算一条记忆的向量:解密 summary → re-embed → put(REPLACE 为当前 model)。
/// 返回 1=已重算, 0=记忆不存在 / summary 为空 / embedding 返回空。
pub fn reindex_one(
    store: &Store,
    dek: &Key32,
    emb: &dyn EmbeddingProvider,
    memory_id: &str,
) -> Result<usize> {
    let summary = match store.get_memory_summary_plaintext(memory_id, dek)? {
        Some(s) if !s.is_empty() => s,
        _ => return Ok(0),
    };
    let (vecs, _usage) = emb.embed(&[summary.as_str()])?;
    let v = vecs.into_iter().next().unwrap_or_default();
    if v.is_empty() {
        return Ok(0);
    }
    store.put_memory_vector(memory_id, &v, &emb.model_name(), chrono::Utc::now().timestamp())?;
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::MockEmbeddingProvider;

    #[test]
    fn reindex_one_replaces_vector_with_current_model() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        // FK: memory_vectors.memory_id → memories(id) —— 必须先建真记忆拿到真实 UUID。
        store
            .insert_memory(&dek, "episodic", 1, 2, &["h1".into()], "summary text", "old-model", 1)
            .unwrap();
        let id = store.list_recent_memories(&dek, 1).unwrap()[0].id.clone();
        // 旧 model 向量(3 维)入库
        store.put_memory_vector(&id, &[0.0; 3], "old-model", 1).unwrap();

        // 当前 provider 是 4 维 mock → model_name == "embed-dim4"
        let emb = MockEmbeddingProvider::new(4);
        let n = reindex_one(&store, &dek, &emb, &id).unwrap();
        assert_eq!(n, 1);

        let v = store.get_memory_vector(&id).unwrap().unwrap();
        assert_eq!(v.model, "embed-dim4"); // 已替换为当前模型(维度键)
        assert_eq!(v.dim, 4); // REPLACE 生效:3 维 → 4 维
    }

    #[test]
    fn reindex_one_returns_zero_for_missing_memory() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let emb = MockEmbeddingProvider::new(4);
        assert_eq!(reindex_one(&store, &dek, &emb, "no-such-id").unwrap(), 0);
    }
}
