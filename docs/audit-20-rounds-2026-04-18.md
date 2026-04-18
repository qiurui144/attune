# Attune 20 轮全项目审计

执行时间：2026-04-18 · 覆盖代码风险 / 功能缺口 / 打包分发 / 会员机制

## Round 1: 打包 & 构建产物

**发现**：
- 🔴 **CRITICAL** — `.github/workflows/` 4 个 yaml（release/build-linux/build-windows/ci）**全部只打 Python + Chrome 扩展**，完全**不构建 Rust 产物**（`rust/target/release/attune-server` 没被任何 workflow 打包）。`release.yml` 只发 `npu-webhook-linux/` + `npu-webhook-windows/` + `extension.zip`。Rust 商用线**没有发布流水线**。
- 🔴 **CRITICAL** — `packaging/linux/AppRun` 和 `packaging/windows/installer.nsi` 只引用 Python 后端，无 Attune Rust 二进制的安装路径。
- 🟡 Rust 二进制 **cargo build --release** 需手动；无交叉编译 CI 矩阵（README.md L571 承诺 Linux x86_64 / aarch64 / Windows 全支持，实际 CI 零验证）

**影响**：用户 git clone → 只能通过 `cargo build` 自行构建；无预编译分发；产品无法交付 B 端客户。

## Round 2: 会员 / 许可 / 自动分发机制

**发现**：
- 🔴 **CRITICAL** — 零许可证/订阅/会员代码。rust 子树内 `license` 字眼只出现在 Cargo.toml 的 OSS 许可字段。**无任何激活、会员校验、版本 tier 区分**的代码。
- 🔴 **CRITICAL** — README L25 承诺"只付两样钱：软件本身 + LLM token"。这一承诺**没有付费入口实现**（无支付集成、无设备绑定、无激活码校验、无灰度升级）。
- 🔴 **CRITICAL** — "自动分发"机制完全缺失：
  - 无自动更新检查（看 GitHub Releases？检查二进制 hash？）
  - 无增量升级（delta update）
  - 无 revocation 机制（如果用户许可过期，代码无路径触发降级）
- 🟡 README 产品定位"私有 AI 知识伙伴"是**付费 B 端/Prosumer 定位**，但当前代码完全等同于 OSS 工具，**产品化层缺失**

**影响**：无法作为付费产品交付；发布后客户自由 fork；没有防火墙阻止盗用。

**建议路径**（产品层决策需先做）：
1. 决定商业模式（开源 + 付费云端 / pro tier 本地二进制 / SaaS 托管？）
2. 若选 pro-tier：需加激活码（HMAC sig + expiry + hardware-bind）、license renewal 心跳
3. 自动更新建议用 [tauri updater pattern](https://tauri.app/v1/guides/distribution/updater/) 或自建 `/api/v1/update/check` 端点
4. 增量 delta 暂无必要（rust 二进制 28MB，全量下载可接受）

## Round 3: 自动更新机制

**发现**：
- 🔴 **CRITICAL** — 零自动更新代码。没有 `/api/v1/update/check`、没有 version 比对、没有二进制下载 / 验签。
- 🟡 `attune_core::version()` 只返回 Cargo 包版本字符串，调用方无处使用
- 🟡 Vault DB 没有 schema `user_version` PRAGMA，未来改表 schema 后，旧 DB 静默跑新代码会碰到 missing column 错误

## Round 4: 数据迁移 (export/import)

**发现**：
- ✅ `/api/v1/profile/export` 存在，返回 VaultProfile { tags, cluster_snapshot, histograms }
- 🟡 **IMPORTANT** — 导出**不含批注** (annotations)！批注已经成为 Batch A 的核心资产（"用户的思考痕迹"），但 `profile.rs::export` 未包含。迁移到新设备时所有批注丢失。
- 🟡 同样不含 `chunk_summaries` 摘要缓存（丢失后首次 chat 全部 miss 重新付费）
- ✅ Device Secret 导出/导入路由存在 (`/vault/device-secret/export/import`)

## Round 5: 备份 / 灾难恢复

**发现**：
- 🔴 **CRITICAL** — **没有 backup 命令**。用户唯一的备份路径是手动复制 `~/.local/share/attune/vault.db`（README L285 建议）
- 🔴 SQLite 用 WAL 模式；复制 `.db` 时若不 checkpoint，WAL 中的写入丢失。用户不会知道要用 `sqlite3 .db "VACUUM INTO 'backup.db'"` 做一致快照
- 🟡 tantivy 索引 + usearch 向量文件**未加入 profile export**；备份时必须单独处理（需要重新 embedding 才能搜索）

## Round 6: 日志 / 可观测性

**发现**：
- ✅ 53 个 tracing::info!/warn!/error! 调用，覆盖主要路由
- 🟡 **IMPORTANT** — 无结构化日志（全是字符串拼接 tracing）；排查生产问题得 grep
- 🟡 无 audit log：谁解锁 vault、谁删 item、LLM 调用分钟计费 — 全不可追溯
- 🟡 无 metrics 端点：无 /metrics（prometheus 格式）、无 request count / latency 分布
- 🟡 Ollama HTTP 失败场景日志模糊：`LlmUnavailable("chat request: {e}")` 不区分网络、超时、API key 错误

## Round 7: Schema 迁移 / 版本管理

**发现**：
- 🔴 **CRITICAL** — `store.rs` 只有 `migrate_task_type` 一次迁移（针对 embed_queue task_type 字段）。**无 PRAGMA user_version 基础设施**，新加表都走 `CREATE TABLE IF NOT EXISTS` 静默跳过。
- 🔴 已有 fresh install 正常；但**若用户旧版 DB（无 annotations / chunk_summaries 表）升级新二进制**，`IF NOT EXISTS` 会创建新表 —— 但如果**未来改表结构**（如给 annotations 加 `reviewed_at` 字段），旧 DB 不会 ALTER，静默运行时 SELECT 不存在的字段会报错。
- 🟡 建议：引入 `migrations/001_init.sql` + `002_add_annotations.sql` + PRAGMA user_version 跳表执行，形成真正的迁移链

## Round 8: 用户可见错误信息友好度

**发现**：
- 统计：168 处 `"error": ...` + 110 处 `e.to_string()` 直接丢给客户端
- 🟡 **IMPORTANT** — 大量错误是 **Rust Debug 格式** 直接 `.to_string()`：
  - `"parse chat response: serde_json::Error(...)"` — 中国用户看不懂
  - `"sqlx error: column X does not exist"` — 泄露 schema 细节
  - `"reqwest::Error { inner: ... }"` — 泄露内部库
- 🔴 未区分「用户可修复」vs「需要联系开发者」的错误。生产产品应有 error code + 友好中文 message + 可选英文技术 detail

## Round 9: 网络搜索稳健性

**发现**：
- ✅ `web_search_browser.rs` 有 `min_interval` rate-limit（阻塞线程 sleep）
- 🟡 **IMPORTANT** — 只有 DuckDuckGo 一个引擎；DDG 限流/封禁时**无 fallback**。README L18 承诺"自动走网络补全"但实际单点依赖
- 🟡 系统浏览器路径自动检测 —— 若用户未装 Chrome/Edge，整个 web_search 静默 degrade（无 UI 提示用户装浏览器）

## Round 10: OCR 稳健性

**发现**：
- ✅ Tesseract CLI 两阶段检测（which + --list-langs）
- 🟡 **IMPORTANT** — `Command::new(tesseract).output()` **无超时** —— 大 PDF 可能卡死半小时，调用方收不到取消信号
- 🟡 `pdftoppm -r 300 -png` 硬编码 DPI，大 PDF（300 页）会生成几 GB 临时 PNG → 磁盘爆
- 🟡 临时文件在 `/tmp/tempfile*` 由 `tempfile::TempDir` 管理，但 OCR 失败时的中间 PNG 未清理

## Round 11: LLM provider 切换稳健

**发现**：
- ✅ OpenAI 兼容 endpoint 支持多后端（OpenAI / LM Studio / vLLM / Ollama v1）
- 🔴 **CRITICAL** — `/api/v1/settings` GET 返回结果中 **`llm.api_key` 明文包含**！任何能访问 `/api/v1/settings` 的请求（包括 Chrome 扩展、未来第三方 tool）都可以拿到 API Key。应该 redact 为 `"sk-***...last4"` 或直接过滤。
- 🟡 **IMPORTANT** — `api_key` 存在 vault_meta 里（SQLite BLOB）但没有单独加密 —— DB 文件被偷 + 用户密码被破解 → API key 泄露。应该单独 AES-GCM 加密（类似 content 字段）
- 🟡 provider 切换后未清理缓存的 `chunk_summaries`（模型不同，摘要质量可能差）

## Round 12: 批注 UX 细节

**发现**：
- ✅ 创建、列表、删除流全
- 🔴 **IMPORTANT** — `makeAnnCard` **只有删除按钮**，无编辑入口。用户要修改批注的内容/标签/颜色，只能删除后重建 —— 丢失 created_at 时序
- 🔴 **IMPORTANT** — AI 批注缺 **"接受 / 忽略 / 保留+备注"** 三按钮（memory spec 要求）
- 🟡 批注重叠处理：当前策略是 `offset_start < pos` 则跳过；没有"合并 / 提示冲突"UI
- 🟡 无 undo 机制：批注删了就没了（AI 分析 5 条批注后用户反悔，得一条条重删）

## Round 13: 搜索质量 / 引用追溯

**发现**：
- ✅ Vector + BM25 + RRF 融合 + reranker（已验证）
- 🟡 **IMPORTANT** — 无查询级别**去重**：同一 item 的多个 chunk 被 top_k 都包含时，可能返回 3-5 个相同 item 的不同段。用户 UI 看到"引用 [MySQL 文档, MySQL 文档, MySQL 文档]" 感觉重复
- 🟡 citations UI 上只显示 title（`引用：oa`），不显示具体 chunk offset；点击不能跳转到阅读视图对应位置
- 🟡 无"相似度"颜色化提示：用户看到 score=1.43（批注 boost 过）感到奇怪

## Round 14: Chat 会话管理

**发现**：
- ✅ 90 行 chat_sessions.rs，CRUD 全（list/get/delete）
- 🟡 `LAST_CHAT_STATS` 跨 session 污染（已在 Batch B.2 review 提及）
- 🟡 无会话导出（user 想把某次对话存档为 markdown — 没路径）
- 🟡 无"继续这个会话"入口：UI 的历史 tab 只能看旧会话，无法从旧会话继续

## Round 15: Settings UI 验证

**发现**：
- 🔴 **IMPORTANT** — Settings 后端 0 处 `valid`/`check` 调用。白名单只是过滤 key 名，**不校验 value**：
  - `context_strategy: "doesn't_exist"` → 写入 vault_meta，读出时 `ContextStrategy::parse` fallback 到 economical（还好）
  - `web_search.min_interval_ms: -1` 或 `"not a number"` → 写入后 provider 读取时 `.as_u64()` 返 None，fallback 到默认（还好）
  - `llm.endpoint: "javascript:alert(1)"` → 存进去！未来若此字段被渲染到前端就 XSS
- 🟡 无设置回滚路径（改错了就得手删 vault_meta row）

## Round 16: 跨设备一致性

**发现**：
- 🔴 **CRITICAL** — 零同步/合并机制。2 台设备 setup 同一密码后**无任何合并协议**：
  - Device A 和 B 分别导出 profile → 导入对方会覆盖/冲突
  - 没有 last_write_wins / CRDT / 增量 sync
- 🟡 README 隐晦宣传"换设备带走"但实际是**手动迁移**（导出 vault.db + device.key + 密码三件套）
- 🟡 没有多设备激活限制 —— 如果加许可证机制时，需设计 device registry

## Round 17: Chrome 扩展集成

**发现**：
- 扩展 76 行 api.js，13 个方法
- ✅ baseUrl 配置灵活，同源 / 远程 NAS 都能接
- 🔴 **IMPORTANT** — 扩展**完全不用**本批新增的：annotations / AI 批注 / 上下文压缩 / token chip 相关端点。扩展功能和 Web UI 严重不对等（扩展还停留在 Batch 2 之前）
- 🟡 扩展无本批成本契约的 UI（注入时不显示预估 token），产品体验割裂

## Round 18: 前端构建 / 单文件

**发现**：
- ✅ `include_str!` 把 2400 行 / 97KB HTML 编译进二进制，单文件分发
- 🟡 **IMPORTANT** — 无**前端压缩/打包**：生产 HTML 97KB，压缩后可到 ~30KB。影响首次加载（本地 localhost 无感，远程 NAS 移动端可感）
- 🟡 无 source map / 开发热更新 —— 改 HTML 要整个 `cargo build`（10+ 秒）
- 🟡 没有 vendor 分离（所有 CSS/JS 内联）—— 不利于 CDN 缓存未来

## Round 19: CI/CD 流程

**发现**：
- 🔴 **CRITICAL** — `ci.yml` 只跑 Python 的 ruff + mypy + pytest，**Rust 端零 CI**：
  - 无 `cargo test --workspace`（299 个测试在 CI 不跑）
  - 无 `cargo clippy`
  - 无 `cargo audit`（依赖漏洞扫描）
  - 无交叉编译矩阵验证
- 🔴 **CRITICAL** — release.yml 只打 Python 产物 + 扩展，**Rust 二进制不发布**（已在 Round 1 提过）
- 🟡 没有 release 流水线的 changelog 自动生成（只依赖 `generate_release_notes: true`）

## Round 20: 遥测 / 隐私合规

**发现**：
- ✅ 零遥测/分析/追踪代码（符合"本地优先、隐私保护"定位）
- 🟡 **IMPORTANT** — 但产品化需要**匿名 crash 上报**（Sentry / 自建 crash collector）才能发现生产 bug
- 🟡 无隐私声明路径：用户问"网络搜索会不会把我的查询上传？" 无文档路径
- 🟡 无合规开关：EU/CN 用户可能需要"禁用所有网络请求"选项（完全离线模式）

---

## 🔥 关键汇总：必须处理的 CRITICAL 问题（9 项）

1. **R1** Rust 产物无 CI/Release —— 项目**无法交付**
2. **R2** 无许可证 / 会员 / 自动分发机制 —— **商业模型缺失**
3. **R2** 无自动更新 —— 用户升级要手动下载
4. **R5** 无 backup 命令 —— 用户复制 WAL 模式 .db 会丢数据
5. **R7** 无 PRAGMA user_version 迁移 —— 未来改 schema 会炸老用户
6. **R11** `/api/v1/settings` 泄露明文 `api_key` —— **安全问题**
7. **R15** Settings endpoint/api_key 字段不校验，可存 `javascript:` URL
8. **R16** 跨设备无同步/合并 —— README "换设备带走" 言过其实
9. **R19** CI 不跑 Rust 测试 —— 299 个测试**零 CI 保护**

## 🟡 IMPORTANT（20 项）

- R4 profile/export 不含 annotations + chunk_summaries
- R6 无结构化日志 / metrics / audit log
- R8 错误信息太技术化，非中文友好
- R9 web_search 单点 DDG 无 fallback
- R10 OCR 无超时、大 PDF 磁盘风险
- R11 api_key 未单独加密（vault lock 泄露风险）
- R12 批注无编辑 / AI 接受-忽略 / undo
- R13 chat 引用不跳转 / 不去重
- R14 会话无导出 / 无"继续"入口
- R15 endpoint/api_key 无 URL 校验
- R16 扩展功能与 Web UI 不对等（缺批注/token chip）
- R17 扩展不用本批新增端点
- R18 前端未压缩，移动端首加载慢
- R19 无 clippy / audit / 交叉编译 CI
- R20 无 crash 上报 / 隐私开关

