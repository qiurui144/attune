# W3 Batch B 设计稿（G1 + G2 + G5 + F3）

**日期**：2026-04-27
**对应路线图**：12-week 战略 v4 Phase 1 W3 F-P0c
**依赖**：W2 batch 1 + W3 batch A (commit `28bd691`)

[English](2026-04-27-w3-batch-b-design.md) · [简体中文](2026-04-27-w3-batch-b-design.zh.md)

---

## 1. 为什么这一批

attune 当前 Chrome 扩展只捕获 ChatGPT/Claude/Gemini AI 对话（窄信号）。用户决策：扩展为"通用浏览状态知识源"，把停留 / 滚动 / 复制 / 回访等高质量"在意"信号喂回 SkillEvolver + Profile。

W3 batch B = G 系列后端 + Chrome 扩展上报 + 隐私控制 + F3 测试。

| 项 | 类型 |
|----|------|
| **G1** 浏览信号捕获 | 新功能 P0 |
| **G2** Auto-bookmark 触发 | 新功能 P0（仅算法）|
| **G5** 隐私控制面板 | 新功能 P0（popup UI）|
| **F3** J5 二次检索 E2E 测试 | followup |

## 2. 隐私默认决策

**默认完全 opt-out**（用户在 popup 显式 enable per-domain）。强制黑名单（银行/医疗/政府登录页/密码管理器/incognito）覆盖任何手动 enable。角色化预设留 W7-8。

## 3. G1 后端

### Schema

`browse_signals(id PK, url_enc / title_enc DEK 加密, domain_hash SHA-256 明文, dwell_ms / scroll_pct / copy_count / visit_count, created_at_secs)` + 索引。

### Store API

`record_browse_signal` / `list_recent_browse_signals` / `browse_signals_count` / `clear_browse_signals_for_domain` / `clear_all_browse_signals`。

### G2 高 engagement 评分

`dwell ≥3 分钟 + scroll ≥50% + copy ≥1` → is_high_engagement → route 层创建 placeholder item（W5-6 G3 时填真内容）。

### Routes

- `POST /api/v1/browse_signals` 接收 batch
- `GET /api/v1/browse_signals?limit=N` 诊断
- `DELETE /api/v1/browse_signals[?domain_hash=xxx]`

## 4. Chrome 扩展

### Manifest 改动

加 `host_permissions: <all_urls>` + `permissions: webNavigation`。`<all_urls>` 触发 Chrome 安装时显式权限提示 — 这是 G5 default opt-out 的硬保证。

### Content script

捕获 dwell / scroll / copy / visit。防护：whitelist 检查 + HARD_BLACKLIST + incognito 跳过 + 不抓 form/password。聚合点：visibilitychange + beforeunload。

### Background

队列 + 30 秒周期 flush + 失败重试 + IndexedDB 持久化未发送队列。

### Popup G5 Privacy

per-domain whitelist 增删 + 全局 Pause toggle + 清除按钮 + 已捕获计数 + "所有数据仅存本机"提示。

## 5. F3 测试

`tests/rag_w3_batch_b_integration.rs`：MockLlm 第一次 `【置信度: 1/5】` 第二次 `【置信度: 5/5】` → 验证 ChatEngine.chat() 触发二次检索。

## 6. 不做（明示）

- ❌ G3 / G4 / G5 角色预设 / Auto-bookmark 真正抓内容 / HARD_BLACKLIST 完整化

## 7. 致谢

- G1: linkwarden + ArchiveBox + attune cost contract
- G5: Standard Notes + Bitwarden 默认 opt-out 模式
- F3: 复用 attune MockLlmProvider

## 8. 验收

- [ ] G1 schema + store CRUD + routes 全绿
- [ ] G2 boundary 测试
- [ ] G5 popup 组件结构
- [ ] F3 通过
- [ ] R1 review + ACKNOWLEDGMENTS + 双语文档 + commit push
