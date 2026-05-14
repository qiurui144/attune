# Attune Changelog

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)。

## [0.6.3] — 2026-05-14

### Fixed

- **(release-blocker) LLM 热重载** — Wizard / Settings 修改 LLM endpoint 后, state.llm 仍 None, chat 返 503 必须重启。抽出 `build_llm_from_settings` 自由函数 + 加 `AppState::reload_llm()` + `settings.rs` PATCH 在 `body.get("llm")` 时触发热切。实测 AMD 笔电 wizard 配 hiapi.online → 第一次 chat 立即返 cloud 响应 (commit d388282)
- **Plugins API 数据源合并** — `GET /api/v1/plugins` 只读 taxonomy.plugins, 用户从 plugins/ 目录安装的 attune-pro vertical (law-pro/patent-pro/presales-pro/tech-pro) 完全不可见; 合并 plugin_registry 数据源 + HashSet 去重, marketplace UI 4 vertical 正确展示 (commit 508b49c)
- **PII Redact 服务端绕过** — routes/chat.rs 自己拼 messages 直调 `llm.chat_with_history`, 完全绕过 attune-core::ChatEngine 的 redact_batch + restore 路径; 加 `Redactor::default()` 全路径拦截 + outbound_audit 日志, 实测 PHONE/EMAIL/CARD 真触发
- **Reset-vault 残留** — `forgot-password-reset` 未清 bound_dirs/indexed_files, 重绑文件夹 FK constraint failed; `wipe_all_user_data` 加 WAL checkpoint + post-assert, `bind_directory_with_domain` 改 UPDATE-or-INSERT, 错误消息脱敏

### Changed

- **About 页面信息密度** — 增加 "会员 / 本地服务 / 存储位置 / 帮助与反馈" 5 节, ServiceStatus 用 ✓ / ⚠ 指示
- **Settings 锁定 UI** — `cloud_llm` 锁定时禁用字段 + 顶部 warning box (不再分散在多个 panel)
- **Wizard 信息密度** — Step 2 Device Secret/会员账号收进 details, Step 3 Ollama/K3 卡片收进 "其他选项" toggle, Step 4 删除多余 Pill, Step 5 加 ? Tooltip
- **按钮色系增强** — Primary/danger 加 baseline boxShadow + hover 加深; 新增 `color-accent-active/on`, `color-surface-muted/strong`, `color-on-surface-muted` tokens
- **i18n locale 持久化** — `localStorage.attune.locale` 跨 session 持久化, hoist 进 component 订阅 reactive source (修 module-level snapshot stale 问题)

### Verified

- AMD 笔电 (Ryzen 7 8845H) deb-only 部署路径全闭环: 重置 vault → wizard 5 步 → 解锁 → 4 vertical 装载 (9 plugin manifest, "loaded 9 plugins" log) → chat 接通 hiapi gpt-4o-mini (附 web search 3 引用) → 暗色模式切换 → About 5 节
- attune-server release 测试套件全通过 (10 + 多 integration 测试 0 failed)
- 浏览器自动化 Playwright Chrome (channel) 通过 SSH tunnel 18900 完成 E2E

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
