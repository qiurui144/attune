# Attune Changelog

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)。

## [Unreleased]

计划中（未发版）：

### Done in Unreleased
- ✅ LICENSE (Apache-2.0) + NOTICE（开源 / 商业分界明确）
- ✅ Rust 跨平台 Release 流水线（5 平台：Linux x86_64/aarch64、macOS Intel/Silicon、Windows x86_64）
- ✅ CHANGELOG 格式固化
- ✅ Rust CI 加入 cargo test workspace + clippy
- ✅ `/api/v1/settings` api_key 返回 redact（安全修复）
- ✅ URL scheme 白名单（http/https）+ browser_path 拒绝 `-` 前缀
- ✅ Skill evolver 三阶段锁释放（修 vault 锁 15s+ 阻塞问题）
- ✅ profile/export 加 annotations（v1→v2，向前兼容）

### Planned
- 插件签名校验骨架（ed25519）— 为商业插件 registry 铺路
- 激活码离线校验（HMAC-SHA256(plan, expiry, device_fp)）— 为 Pro/Pro+ 订阅铺路
- 律师 vertical 落地（参考 lawcontrol 的 plugin / RPA / Intent Router 设计模式，独立实现，不调其 API；详见 `CLAUDE.md` 「独立应用边界」段落与 `docs/superpowers/specs/2026-04-25-industry-attune-design.md`）

## [0.5.x] — 2026-04-18

### Added

**深度阅读 + 批注 + 上下文压缩**（6 个 batch，299→359 tests）

- Batch 1 — Settings 重构 · 硬件感知默认摘要模型 · 扫描版 PDF OCR 兜底
- Batch 2 — 顶栏 + 模态 Settings + 模型 chip（ChatGPT 风格）
- Batch A.1 — 用户批注 CRUD（5 标签 × 4 色）+ Reader 模态
- Batch A.2 — AI 批注（⚠️ 风险 / 🕰 过时 / ⭐ 要点 / 🤔 疑点 四角度）
- Batch B.1 — 上下文压缩流水线（摘要缓存 + 三阶段锁释放）+ Token Chip
- Batch B.2 — 批注加权 RAG + Token Chip 点击展开

### Security

- `/api/v1/settings` GET 响应 redact `api_key` 明文，改返 `api_key_set: bool`
- `update_settings` 对 `llm` 字段深度合并 —— 客户端不发 api_key 时保留原值
- URL scheme 白名单：`llm.endpoint` 只接受 `http://` / `https://`，拒绝 `javascript:` / `file:`
- `web_search.browser_path` 拒绝 `-` 开头（防 argv injection）

### Changed

- Skill Evolver 改三阶段锁释放模式，vault Mutex 在 LLM 调用期间不再被持有（解决 15s+ 阻塞所有路由的并发问题）
- `/api/v1/ingest` 响应补齐 `chunks_queued` 字段（与 `/upload` 对齐）

### Fixed

- PDF 加密扫描件 OCR 兜底路径（pdf_extract 报错时也走 tesseract）
- `allocate_budget` 截断导致的 cache 永不命中（hash 源改用全量 content）
- Spawn_blocking panic 时不再静默丢弃所有 knowledge（fallback 到 raw）
- 精确 label 白名单匹配，修复 `"非过时"` / `"非重点"` 被误判 Drop/Boost 的 footgun

### Docs

- README / DEVELOP / RELEASE 同步更新（10 phase · 57 断言 Playwright 回归全过 + 20 轮全项目审计原始记录见 git history）

### License

- 项目许可变更：MIT → **Apache-2.0**（增加专利授权条款保护贡献者）
- 新增 NOTICE 明确**开源核心**（Apache-2.0）与**商业插件 / 服务**（proprietary）分界

## 早期版本

详见 `rust/RELEASE.md` 历史条目。
