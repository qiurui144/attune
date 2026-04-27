# W3 Batch B Design (G1 + G2 + G5 + F3)

**Date**: 2026-04-27
**Roadmap**: 12-week strategy v4, Phase 1 W3 F-P0c
**Depends on**: W2 batch 1 + W3 batch A (commit `28bd691`)

[English](2026-04-27-w3-batch-b-design.md) · [简体中文](2026-04-27-w3-batch-b-design.zh.md)

---

## 1. Why this batch

attune 当前 Chrome 扩展只捕获 ChatGPT/Claude/Gemini AI 对话（窄信号）。用户决策（memory `project_browser_as_knowledge_source`）：扩展为"通用浏览状态知识源"，把停留 / 滚动 / 复制 / 回访等高质量"在意"信号喂回 SkillEvolver + Profile。

W3 batch B = G 系列后端 + Chrome 扩展上报 + 隐私控制 + F3 测试（W2 batch 1 reviewer 留的 followup）。

| Item | Type | Estimate |
|------|------|----------|
| **G1** Browse signals capture | new feature P0 | medium-large |
| **G2** Auto-bookmark trigger | new feature P0 | small (algorithm only) |
| **G5** Privacy control panel | new feature P0 | medium (popup UI) |
| **F3** J5 secondary retrieval E2E | followup | small |

## 2. Privacy default decision

per `feedback_resource_governance` + `project_browser_as_knowledge_source`：
**默认完全 opt-out**（用户在 popup 显式 enable per-domain）。强制黑名单（银行/医疗/政府登录页/密码管理器/incognito）覆盖任何手动 enable。角色化预设（Developer/Researcher/Lawyer）留 W7-8 与 K3 一起做。

## 3. G1 Backend

### Schema

```
browse_signals(
    id PK AUTOINCREMENT,
    url_enc BLOB (DEK 加密 url),
    title_enc BLOB (DEK 加密 title),
    domain_hash TEXT (SHA-256 of domain, 明文用于聚合 + per-domain 删除),
    dwell_ms INT, scroll_pct INT, copy_count INT, visit_count INT,
    created_at_secs INT
)
INDEX (domain_hash, created_at_secs DESC)
INDEX (created_at_secs DESC)
```

URL/title 加密因为是隐私；engagement 数值明文便于聚合查询。

### Store API

- `record_browse_signal(dek, BrowseSignalInput, now) -> Result<i64>` (id)
- `list_recent_browse_signals(dek, limit) -> Result<Vec<BrowseSignalRow>>`
- `browse_signals_count() -> Result<usize>`
- `clear_browse_signals_for_domain(domain_hash) -> Result<usize>`
- `clear_all_browse_signals() -> Result<usize>`

### G2 high engagement scoring

```rust
pub fn is_high_engagement(s: &BrowseSignalInput) -> bool {
    s.dwell_ms >= 3 * 60 * 1000  // ≥3 分钟
        && s.scroll_pct >= 50     // ≥50%
        && s.copy_count >= 1      // ≥1 次复制
}
```

When triggered, route layer **counts** `high_engagement` (returned in response) but **does not** insert a placeholder item — placeholder without page content is "name only" knowledge that pollutes search. Real page content extraction (Readability-style) is deferred to G3 W5-6, at which point auto-bookmark will create item with extracted body. Per reviewer N4 — spec aligned to actual implementation.

### Routes (attune-server)

- `POST /api/v1/browse_signals` — batch 接收（max 50 signals/req）
- `GET /api/v1/browse_signals?limit=20` — 诊断查询
- `DELETE /api/v1/browse_signals` — 全清
- `DELETE /api/v1/browse_signals?domain_hash=xxx` — per-domain

## 4. Chrome Extension

### Manifest changes

加 `host_permissions: ["<all_urls>"]` + `permissions: ["webNavigation"]`。

`<all_urls>` 触发 Chrome 安装时显式权限提示 — 这是 G5 隐私 default opt-out 的硬保证（用户必须主动开启 per-domain）。

### Content script `extension/src/content/browse_capture.js`

职责：捕获 dwell / scroll / copy / visit 信号。

防护：
1. 检查 `chrome.storage.local.browseWhitelist` 含 `location.hostname` 才捕获（默认 opt-out）
2. HARD_BLACKLIST 域名永远不捕获（bank/medical/gov/login/password manager）
3. `chrome.tabs.incognito` 永远不捕获
4. 不抓 form/password 字段

聚合点：visibilitychange (页面切走) + beforeunload (关页面) → 发 message 到 background。

### Background `extension/src/background/browse_signals.js`

聚合队列 + 30 秒周期 flush + 失败重试 + IndexedDB 持久化未发送队列。

### Popup G5 `extension/src/popup/Privacy.jsx`

- per-domain whitelist 增删
- 全局 Pause toggle（写 `chrome.storage.local.browsePaused`，content script 检查）
- "清除所有已捕获" 按钮 (调 DELETE /api/v1/browse_signals)
- 显示已捕获信号数 + "所有数据仅存本机不上传"提示

## 5. F3 J5 Secondary Retrieval E2E Test

`tests/rag_w3_batch_b_integration.rs`：

1. 真 Store + tempfile, seeded 5 chunks 跨多 items
2. MockLlmProvider push_response 两次:
   - 第一次模糊回答 + `【置信度: 1/5】`
   - 第二次清晰回答 + `【置信度: 5/5】`
3. 构造 ChatEngine 完整依赖（store / fulltext / vectors / embedding / reranker / web_search None）
4. `engine.chat(query, &[], dek)` 跑
5. 断言 `response.confidence == 5` + `response.secondary_retrieval_used == true`

技术挑战：`ChatEngine::new` 接受 6 个 `Arc<Mutex<...>>` 参数，需要 mock embeddings + fulltext index。可用 `embed::NoopProvider` + `FulltextIndex::open_memory()`。

## 6. Out of Scope (defer)

- ❌ G3 页面内容抽取入库（Readability.js style）— W5-6
- ❌ G4 跨 session topic cluster signal — W7-8
- ❌ G5 角色预设白名单 — W7-8
- ❌ Auto-bookmark 真正抓 page content — G3 一并
- ❌ HARD_BLACKLIST 完整化 — W4

## 7. Acknowledgments

per `ACKNOWLEDGMENTS.md` policy:
- **G1 capture pattern**: linkwarden + ArchiveBox 结合 attune cost contract（client-side filter 优先）
- **G5 privacy default opt-out**: Standard Notes + Bitwarden 模式（用户必须显式启用）
- **HARD_BLACKLIST**: 行业常识，无单一来源
- **F3 mocking**: 已有 attune `MockLlmProvider`

## 8. Acceptance Checklist

- [ ] G1 schema migrate clean on existing vault
- [ ] G1 store CRUD tests pass
- [ ] G2 is_high_engagement boundary tests
- [ ] G1 routes POST/GET/DELETE 单元测试
- [ ] G5 popup component shape (Preact JSX)
- [ ] F3 test passes
- [ ] R1 review pass (single round for batch B)
- [ ] ACKNOWLEDGMENTS update
- [ ] Bilingual docs
- [ ] Commit + push
