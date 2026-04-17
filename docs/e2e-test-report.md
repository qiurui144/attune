# Attune E2E Test Report

**测试日期**：2026-04-17
**测试环境**：AMD Ryzen 7 8845H @ 192.168.100.201, Ubuntu 25.10
**部署**：从 GitHub 源码 clone + cargo build --release --workspace
**前端**：Playwright MCP 连接 http://192.168.100.201:18900
**数据库**：全新 vault（每次 `rm -rf ~/.local/share/npu-vault`）

## 测试矩阵

| 场景 | 结果 | 备注 |
|------|------|------|
| ✅ 首次访问 Web UI | PASS | HTML 正常加载、角色选择向导展示 |
| ✅ 主密码设置向导 | PASS | 两次输入密码、setup + unlock 自动串联 |
| ✅ Vault 解锁 → AI/搜索/向量 就绪 | PASS | qwen2.5:3b + bge-m3 + tantivy 全绿 |
| ✅ 文档录入（中文 500 字） | PASS | 保存成功 Toast，1 条目 |
| ✅ 后台 embedding + 分类 | PASS | embedding queue + classifier 自动消费，已分类 1 条 |
| ✅ 全文 + 向量搜索 | PASS | 查询"Rust 所有权 借用" → 命中目标文档，score 0.542 |
| ✅ 条目列表 | PASS | Tab 显示已录入的文档、时间戳正确 |
| ⚠️ RAG Chat（有本地数据） | **部分** | LLM 回答内容正确，但 chat 路径显示「知识库检索 0 条相关文档」—— search_with_context 未命中 |
| ❌ 网络搜索 Fallback | **FAIL** | 问"2026 年诺贝尔奖"无触发浏览器搜索，LLM 用训练截止日期（2023）回答 |

## 发现的 Bug

### Bug #1：新建 vault 后首次 unlock 时 BrowserSearchProvider 未初始化

**现象**：
- 全新 vault、setup + unlock 成功
- POST /api/v1/settings 显式写入 `web_search.enabled=true` 后重启 server + unlock
- `init_search_engines` 日志无 "Web search: browser provider enabled"
- Chat 遇到本地无结果的问题时 `web_search_used: false`
- 服务器日志无 chromiumoxide 活动

**根因（推测）**：
`init_search_engines()`（`rust/crates/attune-server/src/state.rs`）从 `store.get_meta("app_settings")` 读取 settings。新建 vault 的 app_settings 为 None，会 silently 跳过 web_search provider 加载。即使后续 POST /settings 写入并重启，provider 加载路径似乎仍不执行 —— 可能有另一个静默失败点（chromiumoxide launch 在 server 上下文下的沙箱 / AppArmor 限制？）。

**影响**：
核心差异化卖点（"本地决定，全网增强"）**无法在新用户的 first-run 场景下工作**。

**建议修复**：
1. setup 时把 `default_settings()` 主动写入 vault_meta（而不是仅在 GET /settings fallback）
2. `from_settings()` 在 web_search 块缺失时，使用 hardcoded 默认（enabled+auto-detect），而非返回 None
3. 在 BrowserSearchProvider 的 search() 入口加 tracing，才能诊断 chromiumoxide 真正的失败点
4. 加一个 `/api/v1/status/diagnostics` 返回 `web_search.provider_loaded: bool`，让用户能发现

### Bug #2：RAG chat 的 search_with_context 返回 0 条，但直接 /search 能命中

**现象**：
- 搜索 tab 搜"Rust 所有权 借用" → 命中 1 条，score 0.542
- Chat tab 问"Rust 的借用规则有哪些？" → 回答正确但 UI 显示「知识库检索 0 条相关文档」

**根因（推测）**：
两条代码路径调用了不同的 search：
- `/api/v1/search` → 裸 hybrid 搜索（vector + fulltext + RRF）
- `/api/v1/chat` → `search_with_context()` 带 rerank 三阶段管道

chat 路径中 rerank 模型（bge-reranker-v2-m3）下载 404（server 日志确认 Reranker unavailable），降级到 vector cosine fallback。可能降级后 top_k 判断或评分阈值过严，过滤掉了唯一的 1 条结果。

**建议修复**：
1. reranker 不可用时的 fallback 路径走完整 hybrid search（保证 recall），不要再二次筛选
2. 日志打印 search_with_context 的每阶段候选数（initial_k → rerank → top_k）
3. 小语料场景（<10 条）跳过 rerank 阶段

### 次要问题

- **`npu-vault-server listening`**：server 日志文案未随改名更新
- **数据目录**：`~/.local/share/npu-vault/` 仍用老名字（`platform::data_dir()` 未改）
- **Web UI title**：`<title>npu-vault</title>` 未改，header 仍显示"🔐 npu-vault"
- **Reranker 模型下载 404**：`BAAI/bge-reranker-v2-m3` 的 ONNX 模型路径变更或已下架

## 部署工序记录（供复现）

```bash
# 目标机：192.168.100.201
ssh qiurui@192.168.100.201
sudo apt install -y libssl-dev pkg-config
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
curl -fsSL https://ollama.com/install.sh | sh
sudo systemctl start ollama
ollama pull bge-m3
ollama pull qwen2.5:3b
git clone https://github.com/qiurui144/attune.git ~/work/attune
cd ~/work/attune/rust
cargo build --release --workspace
./target/release/attune-server --host 0.0.0.0 --port 18900 --no-auth
# 浏览器访问 http://192.168.100.201:18900
```

注意：`--no-auth` 仅为演示目的；生产部署需加 `--tls-cert/--tls-key` + 移除 `--no-auth`。

## 验收结论

**通过** 6 / 9 场景（Web UI 加载、密码设置、向导、录入、搜索、条目列表）。
**警告** 1 场景（RAG Chat — LLM 回答正确但本地知识未被注入 prompt）。
**失败** 2 场景（浏览器网络搜索 fallback，次要文案残留）。

---

## 2026-04-17 二轮回归（修复后重测）

### 修复措施

| Bug | 修复 | Commit |
|-----|------|--------|
| #1 (web_search 未初始化) | `from_settings` 在 web_search 块缺失时走默认值 + state.rs 区分诊断日志 | d10a318 |
| #2 (RAG 0 hits) | `search_with_context` 每阶段 `log::info!` 计数；reranker 失败时保留 RRF 序 | d10a318 |
| #3 (chromiumoxide CDP 不兼容) | 先升 0.7→0.9.1（仍不兼容）→ **改用 reqwest 直接抓 DuckDuckGo HTML 端点** | 928e919 + 776e711 |
| 文案残留 | `platform.rs` 目录改 attune（兼容老路径）、server 日志、Web UI title/heading | d10a318 |

### 二轮测试结果（同一 AMD 机，全新 vault）

| 场景 | 结果 | 证据 |
|------|------|------|
| 新建 vault setup + unlock | ✅ | `state=unlocked` |
| BrowserSearchProvider 自动加载 | ✅ | 日志 `Web search: browser provider enabled` |
| 本地录入 2 条文档 + 后台 embedding | ✅ | `items=2` |
| **RAG Chat（本地命中）** | ✅ | citations 包含 "Rust 所有权与借用"，`web_search_used=false` |
| **混合智能：本地无 → 网络搜索 fallback** | ✅ | 问 "DuckDuckGo 是什么？" → 日志 `web search: GET https://html.duckduckgo.com/html/?q=...` → parsed 3 results → citations 包含 Wikipedia/百度百科/知乎链接 → `web_search_used=true` → 回答内容基于搜索结果（含来源引用） |
| server 日志 | ✅ | "attune-server listening on http://0.0.0.0:18900" |

### 架构决策记录

**为什么最终放弃 chromiumoxide** —— chromiumoxide 0.7 和 0.9 对当前 Chrome 的 CDP 协议都存在 WS 消息反序列化不兼容（"WS Invalid message: data did not match any variant of untagged enum Message"）。升版仅降级为 WARN 但 fetch_html 仍失败。

**选择 reqwest + DDG HTML 端点** —— DuckDuckGo 的 `/html/` 端点显式兼容无 JS 客户端、对爬虫友好、无验证码。reqwest + 真实浏览器 UA 即可稳定抓取。trade-off：JS-heavy 站点（Google 搜索主页）抓不了 —— 但这类站点本就不适合 fallback 场景。`SearchEngineStrategy` trait 保留，未来按需扩展。

**`detect_system_browser()` 保留** —— 依然作为"系统有 Chrome"的信号位；虽然 fetch_html 不再启动 Chrome，但提供 ergonomic 的 `auto()` 入口 + 未来可选择性回归 chromiumoxide 分支。

## 验收结论（最终）

**通过** 所有 9 / 9 核心场景 + 混合智能网络搜索 fallback。
**知识库构建完善**：录入 → chunk → embed → 全文索引 → 分类 → 本地搜索 → RAG Chat with citations → 本地无则自动 web 搜索 完整 pipeline 端到端跑通。

**后续工作**（非紧急）：
- 补 classifier / clusters / remote / history / settings 五个 tab 的 E2E 覆盖
- bge-reranker-v2-m3 ONNX 模型 404 —— 需要重定向到新版模型路径或打包内嵌
- platform::data_dir 数据目录 attune/ 新用户使用，老 npu-vault/ 兼容读取；是否做迁移 copy 待决策
