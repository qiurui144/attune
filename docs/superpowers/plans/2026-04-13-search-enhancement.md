# Search Enhancement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 Rust 商用线搜索引擎添加 Reranker 精排、LRU 缓存和 3 个缺失的 HTTP 端点
**Architecture:** 在 vault-core 搜索层添加二次排序和缓存，在 vault-server 路由层补充端点
**Tech Stack:** Rust, tantivy, usearch, lru crate, axum 0.8

---

## 背景与现状

### 代码现状（实施前必读）

| 文件 | 关键内容 |
|------|---------|
| `npu-vault/crates/vault-core/src/search.rs` | `rrf_fuse()` + `allocate_budget()`，无 reranker，无缓存 |
| `npu-vault/crates/vault-core/src/vectors.rs` | `VectorIndex` 结构，有 `search()` 但**无** `get_vector()` 方法 |
| `npu-vault/crates/vault-core/src/store.rs` | SQLite schema：`items`、`embed_queue`，无 `feedback` 表 |
| `npu-vault/crates/vault-server/src/state.rs` | `AppState`：`Mutex<Vault>` + `Mutex<Option<VectorIndex>>` 等，无缓存字段 |
| `npu-vault/crates/vault-server/src/routes/items.rs` | `list_items`、`get_item`、`update_item`、`delete_item`，无 stale/stats 端点 |
| `npu-vault/crates/vault-server/src/routes/search.rs` | `search()` + `search_relevant()`，RRF 融合后直接返回，无 reranker |
| `npu-vault/crates/vault-server/src/main.rs` | 路由注册中心，新端点需在此添加 `.route()` |
| `npu-vault/crates/vault-core/Cargo.toml` | 无 `lru` 依赖 |
| `npu-vault/crates/vault-server/Cargo.toml` | 无 `lru` 依赖 |

### 实施顺序

- **Task 1** — `VectorIndex::get_vector()`（F2 基础）
- **Task 2** — `search::rerank()`（F2 核心）
- **Task 3** — 在 `search_relevant` 路由中集成 reranker
- **Task 4** — LRU 缓存（F3）：`vault-core` 添加缓存类型 + `AppState` 集成
- **Task 5** — `store::list_stale_items()` + `GET /api/v1/items/stale`（F6-1）
- **Task 6** — `store::get_item_stats()` + `GET /api/v1/items/{id}/stats`（F6-2）
- **Task 7** — `feedback` 表 schema + `store::insert_feedback()` + `POST /api/v1/feedback`（F6-3）

---

## Task 1：为 VectorIndex 添加 `get_vector()` 方法

**文件：** `npu-vault/crates/vault-core/src/vectors.rs`

### 背景

`VectorIndex` 内部维护 `meta: HashMap<u64, VectorMeta>` 和 `usearch::Index`。usearch `Index` 支持 `get()` 方法返回 `Vec<f32>`。需要通过 `item_id` 查询所有对应 key，然后取各 chunk 向量的均值作为 item 级向量。

### 步骤

- [ ] 在 `vectors.rs` 的 `impl VectorIndex` 末尾（`is_empty()` 之后，`save()` 之前）添加：

```rust
/// 按 item_id 取出所有 chunk 向量，返回均值向量（用于 reranking）
///
/// 若该 item 不存在任何向量或 usearch get() 失败则返回 None。
pub fn get_vector(&self, item_id: &str) -> Option<Vec<f32>> {
    // 找出所有属于该 item 的 key
    let keys: Vec<u64> = self.meta.iter()
        .filter(|(_, m)| m.item_id == item_id)
        .map(|(k, _)| *k)
        .collect();

    if keys.is_empty() {
        return None;
    }

    let mut sum = vec![0.0f32; self.dims];
    let mut count = 0usize;

    for key in &keys {
        // usearch Index::get returns Option<Vec<f32>>
        if let Ok(vec) = self.index.get(*key, self.dims) {
            if vec.len() == self.dims {
                for (s, v) in sum.iter_mut().zip(vec.iter()) {
                    *s += v;
                }
                count += 1;
            }
        }
    }

    if count == 0 {
        return None;
    }

    let inv = 1.0 / count as f32;
    Some(sum.into_iter().map(|v| v * inv).collect())
}
```

> **注意：** usearch 2.x 的 `Index::get(key, dims) -> Result<Vec<f32>, _>` 需要传入维度数。若编译报错，查阅 `usearch = "2"` 的 API 文档确认签名，必要时改用 `self.index.get(key)` 并调整。

- [ ] 在 `vectors.rs` 的 `#[cfg(test)]` 区块末尾添加单元测试：

```rust
#[test]
fn get_vector_returns_mean() {
    let mut idx = VectorIndex::new(4).unwrap();
    // 同一 item 两个 chunk
    idx.add(&[1.0, 0.0, 0.0, 0.0], VectorMeta {
        item_id: "a".into(), chunk_idx: 0, level: 2, section_idx: 0
    }).unwrap();
    idx.add(&[0.0, 1.0, 0.0, 0.0], VectorMeta {
        item_id: "a".into(), chunk_idx: 1, level: 2, section_idx: 0
    }).unwrap();
    // 另一个 item
    idx.add(&[0.0, 0.0, 1.0, 0.0], VectorMeta {
        item_id: "b".into(), chunk_idx: 0, level: 2, section_idx: 0
    }).unwrap();

    let v = idx.get_vector("a").unwrap();
    assert_eq!(v.len(), 4);
    // 均值应为 [0.5, 0.5, 0.0, 0.0]
    assert!((v[0] - 0.5).abs() < 1e-5, "expected 0.5 got {}", v[0]);
    assert!((v[1] - 0.5).abs() < 1e-5, "expected 0.5 got {}", v[1]);

    // 不存在的 item 返回 None
    assert!(idx.get_vector("nonexistent").is_none());
}

#[test]
fn get_vector_missing_item_returns_none() {
    let idx = VectorIndex::new(4).unwrap();
    assert!(idx.get_vector("ghost").is_none());
}
```

- [ ] 运行：`cd /data/company/project/npu-webhook/npu-vault && cargo test -p vault-core vectors -- --nocapture`，确认新测试通过。

**Commit：** `feat(vault-core): add VectorIndex::get_vector() for reranking`

---

## Task 2：在 `search.rs` 中添加 `rerank()` 函数

**文件：** `npu-vault/crates/vault-core/src/search.rs`

### 设计

- 输入：`query_vec: &[f32]`（已由调用方嵌入），`results: &mut Vec<SearchResult>`，`vector_index: &VectorIndex`
- 对每个 `SearchResult`，调用 `vector_index.get_vector(&result.item_id)` 取该 item 的向量
- 计算余弦相似度：`dot(q, v) / (norm(q) * norm(v))`，若任一范数为 0 则 rerank_score = 0.0
- 加权融合：`final_score = 0.7 * rerank_score + 0.3 * result.score`
- 原地按 `final_score` 降序重排 `results`

### 步骤

- [ ] 在 `search.rs` 顶部 `use` 之后，`RRF_K` 常量之前，添加两个 reranker 常量：

```rust
pub const RERANK_VECTOR_WEIGHT: f32 = 0.7;
pub const RERANK_RRF_WEIGHT: f32 = 0.3;
pub const RERANK_TOP_K_THRESHOLD: usize = 20;  // top_k <= 此值时启用 reranker
```

- [ ] 在 `allocate_budget()` 之后（测试模块之前）添加 `rerank()` 函数。注意需要在文件头部添加对 `VectorIndex` 的引用，使用 `crate::vectors::VectorIndex`：

```rust
use crate::vectors::VectorIndex;

/// 计算两个向量的余弦相似度，任一范数为 0 时返回 0.0
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "cosine_similarity: dimension mismatch");
    if a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a < 1e-8 || norm_b < 1e-8 {
        return 0.0;
    }
    (dot / (norm_a * norm_b)).clamp(-1.0, 1.0)
}

/// 对 RRF 一阶结果进行余弦相似度二次排序。
///
/// 当 query 向量可用且结果集不超过 `RERANK_TOP_K_THRESHOLD` 时调用。
/// 原地修改 `results` 的 `score` 字段并重新排序。
///
/// 若 item 在向量索引中无对应向量（尚未完成 embedding），则保留 rrf_score 原值。
pub fn rerank(
    query_vec: &[f32],
    results: &mut Vec<SearchResult>,
    vector_index: &VectorIndex,
) {
    for result in results.iter_mut() {
        let rrf_score = result.score;
        let rerank_score = vector_index
            .get_vector(&result.item_id)
            .map(|item_vec| cosine_similarity(query_vec, &item_vec))
            .unwrap_or(0.0);
        result.score = RERANK_VECTOR_WEIGHT * rerank_score + RERANK_RRF_WEIGHT * rrf_score;
    }
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
}
```

> **注意：** `use crate::vectors::VectorIndex;` 需要放在文件顶部 `use` 区块中，与现有 `use std::collections::HashMap;` 并列。

- [ ] 在 `search.rs` 的 `#[cfg(test)]` 区块中添加单元测试：

```rust
#[test]
fn cosine_similarity_basic() {
    // 相同方向
    assert!((cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-5);
    // 正交
    assert!((cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]) - 0.0).abs() < 1e-5);
    // 零向量
    assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
}

#[test]
fn rerank_orders_by_cosine() {
    use crate::vectors::{VectorIndex, VectorMeta};

    let mut idx = VectorIndex::new(2).unwrap();
    // item "close" 与 query [1,0] 完全对齐
    idx.add(&[1.0, 0.0], VectorMeta { item_id: "close".into(), chunk_idx: 0, level: 2, section_idx: 0 }).unwrap();
    // item "far" 与 query [1,0] 正交
    idx.add(&[0.0, 1.0], VectorMeta { item_id: "far".into(), chunk_idx: 0, level: 2, section_idx: 0 }).unwrap();

    let mut results = vec![
        SearchResult { item_id: "far".into(),   score: 0.9, title: "Far".into(),   content: "c".into(), source_type: "note".into(), inject_content: None },
        SearchResult { item_id: "close".into(), score: 0.5, title: "Close".into(), content: "c".into(), source_type: "note".into(), inject_content: None },
    ];

    // far 的 RRF 分数更高，但 reranker 应将 close 排到前面
    rerank(&[1.0, 0.0], &mut results, &idx);
    assert_eq!(results[0].item_id, "close", "Reranker should elevate closer vector");
}

#[test]
fn rerank_fallback_when_no_vector() {
    use crate::vectors::VectorIndex;

    let idx = VectorIndex::new(2).unwrap();  // empty
    let mut results = vec![
        SearchResult { item_id: "a".into(), score: 0.8, title: "A".into(), content: "c".into(), source_type: "note".into(), inject_content: None },
        SearchResult { item_id: "b".into(), score: 0.3, title: "B".into(), content: "c".into(), source_type: "note".into(), inject_content: None },
    ];
    // 无向量时 rerank_score=0，final_score = 0.3 * rrf_score
    rerank(&[1.0, 0.0], &mut results, &idx);
    // a 的最终分数仍高于 b（因 rrf_score 权重 0.3 保留）
    assert!(results[0].score >= results[1].score);
}
```

- [ ] 运行：`cd /data/company/project/npu-webhook/npu-vault && cargo test -p vault-core search -- --nocapture`

**Commit：** `feat(vault-core): add rerank() function with cosine similarity re-scoring`

---

## Task 3：在搜索路由中集成 Reranker

**文件：** `npu-vault/crates/vault-server/src/routes/search.rs`

### 设计

- 修改 `embed_query()` 函数，使其同时返回 `Vec<(String, f32)>`（向量搜索结果）和 `Option<Vec<f32>>`（query 向量本身，供 reranker 使用）
- 或者：新建 `embed_query_with_vec()` 返回 `(Vec<(String, f32)>, Option<Vec<f32>>)`
- 在 `search()` 和 `search_relevant()` 中，当 `top_k <= RERANK_TOP_K_THRESHOLD` 时，调用 `rerank()`

### 步骤

- [ ] 修改 `search.rs` 顶部 import，添加 reranker 导入：

```rust
use vault_core::search::{
    allocate_budget, rerank, rrf_fuse, SearchResult, INJECTION_BUDGET,
    RERANK_TOP_K_THRESHOLD,
};
```

- [ ] 将现有 `embed_query()` 函数替换为返回 query 向量的新版本：

```rust
/// Embed query text and run vector search.
/// Returns (vector_search_results, query_embedding).
/// query_embedding is Some when embedding succeeded, None otherwise.
async fn embed_query(
    state: &SharedState,
    query: &str,
) -> (Vec<(String, f32)>, Option<Vec<f32>>) {
    let emb_opt = state.embedding.lock().ok().and_then(|g| g.clone());
    let vec_opt_exists = state.vectors.lock().ok().map(|g| g.is_some()).unwrap_or(false);

    let (emb, _) = match (emb_opt, vec_opt_exists) {
        (Some(emb), true) => (emb, ()),
        _ => return (vec![], None),
    };

    let query_owned = query.to_string();
    let state_clone = state.clone();

    let result = tokio::task::spawn_blocking(move || {
        let embeddings = match emb.embed(&[&query_owned]) {
            Ok(e) if !e.is_empty() => e,
            _ => return (vec![], None),
        };
        let query_vec = embeddings[0].clone();
        let vec_guard = match state_clone.vectors.lock() {
            Ok(g) => g,
            Err(_) => return (vec![], None),
        };
        let search_results = match vec_guard.as_ref() {
            Some(vecs) => vecs
                .search(&query_vec, 10)
                .unwrap_or_default()
                .into_iter()
                .map(|(meta, score)| (meta.item_id, score))
                .collect(),
            None => vec![],
        };
        (search_results, Some(query_vec))
    })
    .await;

    result.unwrap_or((vec![], None))
}
```

- [ ] 修改 `search()` handler，在 RRF 融合后加入 reranker 调用：

在 `let fused = rrf_fuse(...)` 之后，`// Fetch and decrypt items` 之前，插入：

```rust
    // Fetch and decrypt items
    let vault = state.vault.lock().map_err(|_| err_500("vault lock poisoned"))?;
    let mut results: Vec<SearchResult> = Vec::new();
    for (item_id, score) in &fused {
        if let Ok(Some(item)) = vault.store().get_item(&dek, item_id) {
            results.push(SearchResult {
                item_id: item.id,
                score: *score,
                title: item.title,
                content: item.content,
                source_type: item.source_type,
                inject_content: None,
            });
        }
    }

    // Rerank when top_k is small enough and query vector is available
    if params.top_k <= RERANK_TOP_K_THRESHOLD {
        if let Some(qvec) = query_vec {
            let vec_guard = state.vectors.lock().map_err(|_| err_500("vectors lock poisoned"))?;
            if let Some(vecs) = vec_guard.as_ref() {
                rerank(&qvec, &mut results, vecs);
            }
        }
    }
```

  完整的 `search()` handler 改写如下（展示完整函数，替换现有实现）：

```rust
pub async fn search(
    State(state): State<SharedState>,
    Query(params): Query<SearchQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let dek = {
        let vault = state.vault.lock().map_err(|_| err_500("vault lock poisoned"))?;
        vault.dek_db().map_err(|e| {
            (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
        })?
    };

    let ft_results = {
        let ft_guard = state.fulltext.lock().map_err(|_| err_500("fulltext lock poisoned"))?;
        match ft_guard.as_ref() {
            Some(ft) => ft.search(&params.q, params.top_k).unwrap_or_default(),
            None => vec![],
        }
    };

    let (vec_results, query_vec) = embed_query(&state, &params.q).await;
    let fused = rrf_fuse(&vec_results, &ft_results, 0.6, 0.4, params.top_k);

    let vault = state.vault.lock().map_err(|_| err_500("vault lock poisoned"))?;
    let mut results: Vec<SearchResult> = Vec::new();
    for (item_id, score) in &fused {
        if let Ok(Some(item)) = vault.store().get_item(&dek, item_id) {
            results.push(SearchResult {
                item_id: item.id,
                score: *score,
                title: item.title,
                content: item.content,
                source_type: item.source_type,
                inject_content: None,
            });
        }
    }

    if params.top_k <= RERANK_TOP_K_THRESHOLD {
        if let Some(qvec) = query_vec {
            let vec_guard = state.vectors.lock().map_err(|_| err_500("vectors lock poisoned"))?;
            if let Some(vecs) = vec_guard.as_ref() {
                rerank(&qvec, &mut results, vecs);
            }
        }
    }

    Ok(Json(serde_json::json!({
        "query": params.q,
        "results": results,
        "total": results.len()
    })))
}
```

- [ ] 类似地，修改 `search_relevant()` handler，在 `allocate_budget()` 之前加入 reranker（相同模式）：

```rust
    if top_k <= RERANK_TOP_K_THRESHOLD {
        if let Some(qvec) = query_vec {
            let vec_guard = state.vectors.lock().map_err(|_| err_500("vectors lock poisoned"))?;
            if let Some(vecs) = vec_guard.as_ref() {
                rerank(&qvec, &mut results, vecs);
            }
        }
    }

    allocate_budget(&mut results, budget);
```

  注意：`embed_query` 的调用需要从 `let vec_results = embed_query(...)` 改为 `let (vec_results, query_vec) = embed_query(...)`.

- [ ] 运行：`cd /data/company/project/npu-webhook/npu-vault && cargo build -p vault-server 2>&1 | head -40`

**Commit：** `feat(vault-server): integrate reranker into search and search_relevant routes`

---

## Task 4：搜索结果 LRU 缓存

**文件：**
- `npu-vault/crates/vault-core/Cargo.toml`（添加依赖）
- `npu-vault/crates/vault-server/Cargo.toml`（可选：仅 vault-server 需要也可在此添加）
- `npu-vault/crates/vault-server/src/state.rs`（添加缓存字段）
- `npu-vault/crates/vault-server/src/routes/search.rs`（读写缓存）
- `npu-vault/crates/vault-server/src/routes/ingest.rs`（ingest 时清空缓存）

### 步骤

#### 4.1 添加 `lru` 依赖

- [ ] 在 `npu-vault/crates/vault-server/Cargo.toml` 的 `[dependencies]` 末尾添加：

```toml
lru = "0.12"
```

> **版本确认：** `lru` 当前稳定版为 `0.12.x`（2024年）。若需要精确版本，可用 `cargo search lru` 或查阅 crates.io。

#### 4.2 在 `state.rs` 添加缓存字段

- [ ] 在 `state.rs` 顶部添加 import：

```rust
use lru::LruCache;
use std::num::NonZeroUsize;
use std::time::Instant;
```

- [ ] 添加缓存包装结构（在 `use` 块之后，`pub type SharedState` 之前）：

```rust
const SEARCH_CACHE_CAPACITY: usize = 256;
const SEARCH_CACHE_TTL_SECS: u64 = 30;

/// LRU 搜索缓存条目，携带时间戳用于 TTL 验证
pub struct CachedSearch {
    pub results: Vec<vault_core::search::SearchResult>,
    pub created_at: Instant,
}

impl CachedSearch {
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed().as_secs() >= SEARCH_CACHE_TTL_SECS
    }
}
```

- [ ] 在 `AppState` 结构体末尾（`require_auth` 之后）添加字段：

```rust
    pub search_cache: Mutex<LruCache<u64, CachedSearch>>,
```

- [ ] 在 `AppState::new()` 中初始化：

```rust
    search_cache: Mutex::new(LruCache::new(
        NonZeroUsize::new(SEARCH_CACHE_CAPACITY).unwrap()
    )),
```

- [ ] 在 `clear_search_engines()` 中清空缓存：

```rust
    self.search_cache.lock().unwrap().clear();
```

#### 4.3 缓存 key 计算函数

- [ ] 在 `routes/search.rs` 顶部添加 hash 辅助函数：

```rust
/// djb2 hash，用作缓存 key（query 字符串 → u64）
fn hash_query(query: &str) -> u64 {
    let mut hash: u64 = 5381;
    for b in query.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(b as u64);
    }
    hash
}
```

#### 4.4 在 `search()` handler 中读写缓存

- [ ] 在 `search()` 函数开头（`let dek = ...` 之前）读取缓存：

```rust
    // --- cache read ---
    let cache_key = hash_query(&params.q);
    {
        let mut cache = state.search_cache.lock().map_err(|_| err_500("cache lock poisoned"))?;
        if let Some(entry) = cache.get(&cache_key) {
            if !entry.is_expired() {
                return Ok(Json(serde_json::json!({
                    "query": params.q,
                    "results": entry.results,
                    "total": entry.results.len(),
                    "cached": true
                })));
            }
        }
    }
    // --- cache miss: proceed with full search ---
```

- [ ] 在函数末尾 `Ok(Json(...))` 之前写入缓存：

```rust
    // --- cache write ---
    {
        let mut cache = state.search_cache.lock().map_err(|_| err_500("cache lock poisoned"))?;
        cache.put(cache_key, crate::state::CachedSearch {
            results: results.clone(),
            created_at: std::time::Instant::now(),
        });
    }
```

- [ ] `search_relevant()` 不缓存（注入场景需要实时性），保持不变。

#### 4.5 ingest 时清空缓存

- [ ] 在 `routes/ingest.rs` 中，`insert_item()` 成功后添加清空缓存：

```rust
    // 清空搜索缓存（新条目入库后缓存失效）
    {
        let mut cache = state.search_cache.lock().unwrap();
        cache.clear();
    }
```

> 简单粗暴地全量清空，避免 source_type 过滤等复杂失效逻辑。

- [ ] 运行：`cd /data/company/project/npu-webhook/npu-vault && cargo build -p vault-server 2>&1 | head -60`

#### 4.6 添加单元测试（vault-core 层不需要缓存测试；集成测试在 vault-server 层）

由于缓存逻辑在 `state.rs` 和路由层，单元测试较复杂。添加简单的 hash 函数测试：

- [ ] 在 `routes/search.rs` 末尾添加：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_query_deterministic() {
        assert_eq!(hash_query("hello"), hash_query("hello"));
        assert_ne!(hash_query("hello"), hash_query("world"));
    }

    #[test]
    fn hash_query_empty() {
        // empty string should not panic
        let _ = hash_query("");
    }
}
```

**Commit：** `feat(vault-server): add LRU search cache (256 entries, 30s TTL)`

---

## Task 5：`list_stale_items()` + `GET /api/v1/items/stale`

**文件：**
- `npu-vault/crates/vault-core/src/store.rs`
- `npu-vault/crates/vault-server/src/routes/items.rs`
- `npu-vault/crates/vault-server/src/main.rs`

### 背景

`items` 表的 `updated_at` 字段记录最后修改时间（ISO 8601 字符串）。"超过 30 天未访问" 按 `updated_at` 字段判断（知识库无独立 `last_viewed_at`，以 `updated_at` 代替）。

### 步骤

#### 5.1 添加 `ItemSummary` 结构和 `list_stale_items()` 方法

- [ ] 在 `store.rs` 中，找到 `ItemRow` 或 `ItemSummary` 结构体定义处。如果不存在 `ItemSummary`，在 `list_items()` 方法附近添加：

```rust
/// 条目摘要（不含加密内容，用于列表端点）
#[derive(Debug, serde::Serialize)]
pub struct ItemSummary {
    pub id: String,
    pub title: String,
    pub source_type: String,
    pub updated_at: String,
    pub created_at: String,
}
```

- [ ] 在 `store.rs` 中找到 `list_items()` 方法，在其后添加：

```rust
/// 列出超过 `days` 天未更新的条目（最多 `limit` 条）
pub fn list_stale_items(&self, days: i64, limit: i64) -> Result<Vec<ItemSummary>> {
    let cutoff = chrono::Utc::now()
        .checked_sub_signed(chrono::Duration::days(days))
        .unwrap_or_else(chrono::Utc::now)
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string();

    let mut stmt = self.conn.prepare(
        "SELECT id, title, source_type, updated_at, created_at
         FROM items
         WHERE is_deleted = 0 AND updated_at < ?1
         ORDER BY updated_at ASC
         LIMIT ?2"
    )?;
    let rows = stmt.query_map(params![cutoff, limit], |row| {
        Ok(ItemSummary {
            id: row.get(0)?,
            title: row.get(1)?,
            source_type: row.get(2)?,
            updated_at: row.get(3)?,
            created_at: row.get(4)?,
        })
    })?;
    let mut items = Vec::new();
    for row in rows {
        items.push(row?);
    }
    Ok(items)
}
```

> **注意：** `store.rs` 使用 `use chrono` 吗？查看现有 `insert_item()` 中时间戳的生成方式。若使用 `chrono::Utc::now().to_rfc3339()`，则 `chrono` 已在 `vault-core` 依赖中，直接使用即可。若未 `use chrono`，在文件顶部添加 `use chrono::{Duration, Utc};`。

#### 5.2 添加路由 handler

- [ ] 在 `routes/items.rs` 末尾添加：

```rust
#[derive(serde::Deserialize)]
pub struct StaleQuery {
    #[serde(default = "default_stale_days")]
    pub days: i64,
    #[serde(default = "default_stale_limit")]
    pub limit: i64,
}

fn default_stale_days() -> i64 { 30 }
fn default_stale_limit() -> i64 { 50 }

pub async fn list_stale_items(
    State(state): State<SharedState>,
    Query(params): Query<StaleQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap();
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    let items = vault.store().list_stale_items(params.days, params.limit).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    Ok(Json(serde_json::json!({"items": items, "count": items.len(), "days": params.days})))
}
```

#### 5.3 注册路由

- [ ] 在 `main.rs` 中，`.route("/api/v1/items", get(routes::items::list_items))` 之后添加：

```rust
.route("/api/v1/items/stale", get(routes::items::list_stale_items))
```

> **顺序注意：** `/api/v1/items/stale` 必须在 `/api/v1/items/{id}` 之前注册，否则 axum 0.8 会将 `stale` 匹配为 `id`。在当前 `main.rs` 中，`/api/v1/items/{id}` 在第 66 行，需将 stale 路由插在它之前。

#### 5.4 添加单元测试

- [ ] 在 `store.rs` 的 `#[cfg(test)]` 区块中（或相关 test 文件）添加：

```rust
#[test]
fn list_stale_items_basic() {
    use chrono::{Duration, Utc};

    let mut store = Store::open_memory().unwrap();
    let dek = vault_core::crypto::Key32::generate();

    // 插入一个"新"条目（today）
    let id_new = store.insert_item(&dek, "New", "content", None, "note", None, None).unwrap();

    // 手动更新一个条目的 updated_at 为 40 天前（绕过 insert_item 使用 SQL）
    let old_ts = (Utc::now() - Duration::days(40)).format("%Y-%m-%dT%H:%M:%S").to_string();
    store.conn.execute(
        "UPDATE items SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![old_ts, id_new],
    ).unwrap();

    let stale = store.list_stale_items(30, 50).unwrap();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].id, id_new);
}

#[test]
fn list_stale_items_empty() {
    let store = Store::open_memory().unwrap();
    let stale = store.list_stale_items(30, 50).unwrap();
    assert!(stale.is_empty());
}
```

> **注意：** `Store` 的 `conn` 字段当前是私有的（`conn: Connection`）。测试中需要访问它，可以：(a) 将 `conn` 暴露为 `pub(crate)`，或 (b) 给 `Store` 添加一个仅测试用的 `execute_raw()` 辅助方法，或 (c) 把测试写成集成测试并通过公开 API 构造数据。推荐方案 (c)：先 ingest 后手动修改，通过在 `store.rs` 添加 `#[cfg(test)] pub fn set_updated_at(&self, id: &str, ts: &str) -> Result<()>` 测试辅助方法。

- [ ] 运行：`cd /data/company/project/npu-webhook/npu-vault && cargo test -p vault-core store -- --nocapture`

**Commit：** `feat(vault-core/vault-server): add list_stale_items() + GET /api/v1/items/stale`

---

## Task 6：`get_item_stats()` + `GET /api/v1/items/{id}/stats`

**文件：**
- `npu-vault/crates/vault-core/src/store.rs`
- `npu-vault/crates/vault-server/src/routes/items.rs`
- `npu-vault/crates/vault-server/src/main.rs`

### 步骤

#### 6.1 添加 `ItemStats` 结构和 `get_item_stats()` 方法

- [ ] 在 `store.rs` 中，`ItemSummary` 之后添加：

```rust
/// 单条目统计信息
#[derive(Debug, serde::Serialize)]
pub struct ItemStats {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub chunk_count: i64,           // embed_queue 中该 item 的总任务数
    pub embedding_pending: i64,     // 状态为 pending 的任务数
    pub embedding_done: i64,        // 状态为 done 的任务数
}
```

- [ ] 在 `list_stale_items()` 之后添加：

```rust
/// 获取单条目统计信息（不解密内容，从 embed_queue 统计 embedding 状态）
pub fn get_item_stats(&self, id: &str) -> Result<Option<ItemStats>> {
    // 先确认 item 存在
    let exists: i64 = self.conn.query_row(
        "SELECT COUNT(*) FROM items WHERE id = ?1 AND is_deleted = 0",
        params![id],
        |row| row.get(0),
    )?;
    if exists == 0 {
        return Ok(None);
    }

    let (created_at, updated_at): (String, String) = self.conn.query_row(
        "SELECT created_at, updated_at FROM items WHERE id = ?1",
        params![id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    let chunk_count: i64 = self.conn.query_row(
        "SELECT COUNT(*) FROM embed_queue WHERE item_id = ?1",
        params![id],
        |row| row.get(0),
    )?;

    let embedding_pending: i64 = self.conn.query_row(
        "SELECT COUNT(*) FROM embed_queue WHERE item_id = ?1 AND status = 'pending'",
        params![id],
        |row| row.get(0),
    )?;

    let embedding_done: i64 = self.conn.query_row(
        "SELECT COUNT(*) FROM embed_queue WHERE item_id = ?1 AND status = 'done'",
        params![id],
        |row| row.get(0),
    )?;

    Ok(Some(ItemStats {
        id: id.to_string(),
        created_at,
        updated_at,
        chunk_count,
        embedding_pending,
        embedding_done,
    }))
}
```

> **注意：** 此方法不需要 `dek`，因为不解密内容——只读元数据和统计数字。

#### 6.2 添加路由 handler

- [ ] 在 `routes/items.rs` 中，`list_stale_items` 之后添加：

```rust
pub async fn get_item_stats(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap();
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;
    match vault.store().get_item_stats(&id) {
        Ok(Some(stats)) => Ok(Json(serde_json::json!(stats))),
        Ok(None) => Err((StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "not found"})))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))),
    }
}
```

#### 6.3 注册路由

- [ ] 在 `main.rs` 中，`.route("/api/v1/items/{id}", ...)` 那行修改为链式添加 stats，或在其后新增：

```rust
.route("/api/v1/items/{id}/stats", get(routes::items::get_item_stats))
```

> **顺序：** 放在 `.route("/api/v1/items/{id}", get(...).delete(...).patch(...)...)` 之后即可，axum 会优先匹配更具体的 `/items/{id}/stats`。

#### 6.4 添加单元测试

- [ ] 在 `store.rs` 测试区块添加：

```rust
#[test]
fn get_item_stats_basic() {
    let store = Store::open_memory().unwrap();
    let dek = vault_core::crypto::Key32::generate();

    let id = store.insert_item(&dek, "Test", "content", None, "note", None, None).unwrap();
    // insert_item 会自动 enqueue，先确认 stats 可查
    let stats = store.get_item_stats(&id).unwrap().unwrap();
    assert_eq!(stats.id, id);
    assert!(stats.chunk_count >= 0);
    assert_eq!(stats.embedding_pending + stats.embedding_done, stats.chunk_count);
}

#[test]
fn get_item_stats_missing() {
    let store = Store::open_memory().unwrap();
    let result = store.get_item_stats("nonexistent-id").unwrap();
    assert!(result.is_none());
}
```

- [ ] 运行：`cd /data/company/project/npu-webhook/npu-vault && cargo test -p vault-core -- item_stats --nocapture`

**Commit：** `feat(vault-core/vault-server): add get_item_stats() + GET /api/v1/items/{id}/stats`

---

## Task 7：`feedback` 表 + `insert_feedback()` + `POST /api/v1/feedback`

**文件：**
- `npu-vault/crates/vault-core/src/store.rs`（schema 迁移 + CRUD）
- `npu-vault/crates/vault-server/src/routes/feedback.rs`（新建文件）
- `npu-vault/crates/vault-server/src/routes/mod.rs`（暴露模块）
- `npu-vault/crates/vault-server/src/main.rs`（注册路由）

### 步骤

#### 7.1 添加 `feedback` 表 Schema

- [ ] 在 `store.rs` 的 `SCHEMA_SQL` 字符串末尾（最后一个 `CREATE INDEX` 之后）追加：

```sql
CREATE TABLE IF NOT EXISTS feedback (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id      TEXT NOT NULL,
    feedback_type TEXT NOT NULL CHECK(feedback_type IN ('relevant','irrelevant','correction')),
    query        TEXT,
    created_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_feedback_item ON feedback(item_id);
CREATE INDEX IF NOT EXISTS idx_feedback_created ON feedback(created_at);
```

> **注意：** 直接在 `SCHEMA_SQL` 中添加 `CREATE TABLE IF NOT EXISTS feedback`，由于使用 `IF NOT EXISTS`，对已存在数据库是幂等的，不需要单独迁移函数。但若已有线上数据库（无 feedback 表），会在下次启动时自动创建。

#### 7.2 添加 `FeedbackEntry` 结构和 `insert_feedback()` 方法

- [ ] 在 `store.rs` 中 `ItemStats` 之后添加：

```rust
/// 用户反馈条目
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct FeedbackEntry {
    pub id: i64,
    pub item_id: String,
    pub feedback_type: String,
    pub query: Option<String>,
    pub created_at: String,
}
```

- [ ] 在 `get_item_stats()` 之后添加：

```rust
/// 插入用户反馈（feedback_type 须为 "relevant"/"irrelevant"/"correction"）
pub fn insert_feedback(
    &self,
    item_id: &str,
    feedback_type: &str,
    query: Option<&str>,
) -> Result<i64> {
    let valid_types = ["relevant", "irrelevant", "correction"];
    if !valid_types.contains(&feedback_type) {
        return Err(VaultError::Crypto(format!(
            "invalid feedback_type: {feedback_type}"
        )));
    }
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string();
    self.conn.execute(
        "INSERT INTO feedback (item_id, feedback_type, query, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![item_id, feedback_type, query, now],
    )?;
    Ok(self.conn.last_insert_rowid())
}
```

#### 7.3 新建 `routes/feedback.rs`

- [ ] 创建新文件 `npu-vault/crates/vault-server/src/routes/feedback.rs`：

```rust
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::state::SharedState;

#[derive(Deserialize)]
pub struct FeedbackRequest {
    pub item_id: String,
    pub feedback_type: String,
    pub query: Option<String>,
}

pub async fn submit_feedback(
    State(state): State<SharedState>,
    Json(body): Json<FeedbackRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap();
    let _ = vault.dek_db().map_err(|e| {
        (StatusCode::FORBIDDEN, Json(serde_json::json!({"error": e.to_string()})))
    })?;

    let id = vault
        .store()
        .insert_feedback(&body.item_id, &body.feedback_type, body.query.as_deref())
        .map_err(|e| {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?;

    Ok(Json(serde_json::json!({"id": id, "status": "ok"})))
}
```

#### 7.4 暴露模块

- [ ] 在 `routes/mod.rs` 末尾添加：

```rust
pub mod feedback;
```

#### 7.5 注册路由

- [ ] 在 `main.rs` 中，`.route("/api/v1/ingest", ...)` 之后添加：

```rust
.route("/api/v1/feedback", post(routes::feedback::submit_feedback))
```

#### 7.6 添加单元测试

- [ ] 在 `store.rs` 测试区块添加：

```rust
#[test]
fn insert_feedback_valid() {
    let store = Store::open_memory().unwrap();
    let id = store.insert_feedback("item-1", "relevant", Some("my query")).unwrap();
    assert!(id > 0);
}

#[test]
fn insert_feedback_invalid_type() {
    let store = Store::open_memory().unwrap();
    let result = store.insert_feedback("item-1", "bad_type", None);
    assert!(result.is_err());
}

#[test]
fn insert_feedback_no_query() {
    let store = Store::open_memory().unwrap();
    let id = store.insert_feedback("item-1", "irrelevant", None).unwrap();
    assert!(id > 0);
}
```

- [ ] 运行：`cd /data/company/project/npu-webhook/npu-vault && cargo test -p vault-core -- feedback --nocapture`
- [ ] 运行：`cd /data/company/project/npu-webhook/npu-vault && cargo build -p vault-server 2>&1 | head -60`

**Commit：** `feat(vault-core/vault-server): add feedback table + insert_feedback() + POST /api/v1/feedback`

---

## Task 8：全量验证

- [ ] 运行全量测试：`cd /data/company/project/npu-webhook/npu-vault && cargo test 2>&1 | tail -30`
- [ ] 确认无编译警告（`cargo build --release -p vault-server 2>&1 | grep -E "warning|error"`）
- [ ] 检查 Python 原型线测试不受影响：`cd /data/company/project/npu-webhook && python -m pytest tests/ -x -q 2>&1 | tail -20`

---

## 自检清单

完成所有 Task 后，验证以下项目：

- [ ] **占位符扫描**：计划中无 `TODO`、`YOUR_CODE_HERE`、`...` 等占位符残留
- [ ] **`get_vector()` 存在**：Task 1 明确在 `vectors.rs` 中添加此方法
- [ ] **`lru` 版本号正确**：Task 4.1 使用 `lru = "0.12"`（crates.io 当前稳定版）
- [ ] **F6 三个端点的 Store 方法**：
  - `list_stale_items()` → Task 5.1
  - `get_item_stats()` → Task 6.1
  - `insert_feedback()` → Task 7.2
- [ ] **路由注册顺序正确**：`/api/v1/items/stale` 在 `/api/v1/items/{id}` 之前
- [ ] **feedback 表 CHECK 约束**：仅允许 `relevant`/`irrelevant`/`correction`
- [ ] **reranker 仅在 `top_k <= 20` 时启用**：Task 3 中有 `if params.top_k <= RERANK_TOP_K_THRESHOLD`
- [ ] **缓存 TTL = 30s，容量 = 256**：Task 4.2 中常量定义正确
- [ ] **ingest 清空缓存**：Task 4.5 中在 `ingest.rs` 插入 `cache.clear()`

---

## API 变更汇总

| 端点 | 方法 | 新/改 | 说明 |
|------|------|------|------|
| `GET /api/v1/search?q=&top_k=` | GET | 改 | 新增 reranker（top_k<=20），新增 LRU 缓存，响应新增 `"cached": true` 字段 |
| `GET /api/v1/items/stale?days=30&limit=50` | GET | 新 | 返回超过 N 天未更新的条目列表 |
| `GET /api/v1/items/{id}/stats` | GET | 新 | 返回条目统计（chunk_count, embedding_pending/done） |
| `POST /api/v1/feedback` | POST | 新 | 接收用户相关性反馈 |
