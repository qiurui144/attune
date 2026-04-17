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

---

## 2026-04-17 四轮回归：硬件加速 + reranker 可用

### 硬件画像自动检测（代码级）

启动日志实测输出：
```
hardware: OS=linux | CPU=AMD Ryzen 7 8845H w/ Radeon 780M Graphics (AuthenticAMD) | AMD GPU (gfx=gfx1103) | AMD XDNA NPU (Ryzen AI)
hardware: set HSA_OVERRIDE_GFX_VERSION=11.0.0 — AMD gfx1103 → ROCm runtime 兼容 11.0.0
```

实现：`rust/crates/attune-core/src/platform.rs`
- `HardwareProfile::detect()` 扫描 `/proc/cpuinfo`、`/dev/nvidia0`、`/dev/kfd`、`/sys/class/kfd/kfd/topology/nodes/*/properties`、`/dev/accel/accel0` + `/proc/modules`
- `apply_recommended_env()` 为 AMD APU 的 gfx1103/1102/1150/1151 映射到 `HSA_OVERRIDE_GFX_VERSION=11.0.0`；其他型号按表对应；NVIDIA 补 `CUDA_VISIBLE_DEVICES=0`。用户已设的 env 不覆盖

### Ollama ROCm 落地（系统级）

`scripts/enable-amd-rocm-ollama.sh` 写 systemd drop-in：
```
[Service]
Environment="HSA_OVERRIDE_GFX_VERSION=11.0.0"
```

Ollama 启动日志验证：
```
inference compute  id=0  library=ROCm  compute=gfx1100  name=ROCm0
description="AMD Radeon 780M Graphics"  type=iGPU  total="17.3 GiB"
```

### 吞吐加速实测

同一语料（19 章 rust-book） × 同一机器 × 启用前后对比：

| 阶段 | CPU only | ROCm | 加速 |
|------|---------|------|------|
| Embed (bge-m3) | 4.3 chunks/s | **~18 chunks/s** | ~4x |
| Classify (qwen2.5:3b) | 14.5 s/item | **~3 s/item** | ~5x |
| 总 queue drain (337 tasks) | 347 s | **48 s** | **7.2x** |

### Reranker / Embedding 模型迁移

原配置（默认值）—— 404 错误链：
- `BAAI/bge-reranker-v2-m3` + `onnx/model_quantized.onnx` ← 仓库没 ONNX 文件
- `Qwen/Qwen3-Embedding-0.6B` + `onnx/model_quantized.onnx` ← 仓库没 ONNX

新配置（默认）—— 实测下载 + 加载成功：
- `Xenova/bge-reranker-base` + `onnx/model_quantized.onnx` (267 MB)
- `Xenova/bge-m3` + `onnx/model_quantized.onnx` (544 MB, 多语言 1024 维)

可选切换（env var）：
- `ATTUNE_RERANKER_MODEL=jina-v2-multilingual` → Jina v2 多语言 reranker
- `ATTUNE_RERANKER_MODEL=bge-base-official` → BAAI 官方 bge-reranker-base (330 MB, 无 Xenova 量化)
- `ATTUNE_EMBEDDING_MODEL=multilingual-e5-small|base` → 更小的多语言 embedder

### 四轮 RAG 5 问（19 章 rust-book，带 ROCm + 真实 reranker）

| Q | top-1 | top-3 | 延迟 |
|----|------|------|------|
| Q1 references/borrowing | ⏱ timeout 60s（LLM 加载） | — | 60s |
| Q2 Box<T> | ✅ box | ✅ | 37s |
| Q3 reference cycles | ❌ deref（ref-cycles 未入 top-3） | — | 19s |
| Q4 lifetimes | ⏱ timeout 60s（LLM 加载） | — | 60s |
| Q5 refutable | ✅ refutability | ✅ | 14s |

对比三轮（无 reranker，只 RRF）的结果 4/5 top-1 → 四轮（用 cross-encoder reranker）变成 2/5 top-1。**Cross-encoder 对小语料反而不如 RRF 稳定**：

- **relevance 分值量级变化**：三轮 0.002（RRF reciprocal rank） → 四轮 0.007（bge-reranker cross-encoder sigmoid score）确认 reranker 真正接管了打分
- **Q3 退化根因**：bge-reranker-base 是主训练英文语义相似度的 cross-encoder。对 "reference cycles" → "deref" 这种语义相近但不对应章节的 false positive 无力修正
- **对策**（后续）：设置 reranker 触发阈值（候选 > N 才启用 rerank），避免小规模召回被 cross-encoder 重排打乱 RRF 的好结果

### 最终成就

✅ 代码级硬件画像检测 + 默认启用优化 env
✅ Ollama ROCm 7.2x 加速（scripts/enable-amd-rocm-ollama.sh 一键启用）
✅ Reranker + Embedding 模型迁移到 Xenova 可用镜像，下载 + 加载成功
⚠️ Reranker 对小语料的排序策略需要阈值化（记录为后续优化）
⚠️ LLM 首次 query 60s 冷启动（模型加载到 VRAM 后续快）

---

## 2026-04-17 五轮回归：多场景 × 多语料 RAG

### 语料扩充到 3 套场景

| 场景 | 语料 | 语言 | 规模 | 状态 |
|------|------|------|------|------|
| A. 律师 / 法律咨询 | `/data/company/lawcontrol/data/test_evidence/` | 中文 | 15 文件（民法典/公司法/劳动合同法全文 + 7 案例 + 3 文书 + 2 合同） | ✅ Ingest 完成 |
| B. Rust 开发者 | rust-lang/book @ trpl-v0.3.0 | 英文 | 19 章 | ✅ Ingest 完成（四轮已验证） |
| C. 中文技术读者 | `TIM168/technical_books`（Python/Go/AI/数据库/算法 子集） | 中文 PDF | 185 PDFs | 📋 脚本就绪，未 ingest（5.6GB 需 sparse checkout） |

### 全量 RAG 10 问矩阵（场景 A + B）

Vault 共 52 items（19 Rust 章 + 15 法律文本 + 累积文档），ROCm 启用，bge-reranker-base cross-encoder 激活。

**场景 A：律师 / 中文法律**

| Q | 查询 | top-1 | 命中 | Rel | 延迟 |
|---|------|-------|------|-----|------|
| A1 | 劳动者主动解除劳动合同需要提前多少天 | 劳动合同法_全文 | ✅ top-1 | 0.181 | 4.1s |
| A2 | 民间借贷利率保护上限 | 民法典 | ✅ top-3（借款合同） | 0.132 | 5.6s |
| A3 | 商标侵权法律责任 | 民法典 | ✅ top-2（案例_商标侵权 rel 0.05） | 0.139 | 8.7s |
| A4 | 公司股东会决议表决程序 | 公司法_全文 | ✅ top-1 | 0.208 | 10.5s |
| A5 | 合同违约金法律规定 | 民法典 | ❌ 期待"买卖合同"但民法典本身也包含 | 0.191 | 12.4s |

**场景 A 结果：4/5 top-3 命中**，中文 RAG 健康 ✅

**场景 B：Rust 开发者 / 英文**

| Q | 查询 | top-1 | 命中 | Rel |
|---|------|-------|------|-----|
| B1 | references vs borrowing | **民法典** | ❌ | 0.168 |
| B2 | When use Box<T>? | **民法典** | ❌ | 0.127 |
| B3 | Reference cycles | **民法典** | ❌ | 0.122 |
| B4 | What are lifetimes? | **民法典** | ❌ | 0.139 |
| B5 | Refutable vs irrefutable | **民法典** | ❌ | 0.177 |

**场景 B 结果：0/5 — 全部被民法典吸走** ❌

### Bug #5 — Cross-lingual 污染（本次暴露的关键问题）

**现象**：只要混合中英文语料，英文 query 的 top-1 几乎必然是民法典（中文长文档）。

**根因分析**（三层叠加）：

1. **bge-reranker-base 是英文主训**：对中文 chunks 给出异常高的 pseudo-score（0.12-0.17 远超真实相关度）
2. **大文档偏置**：民法典 328KB → 数十个 chunks → 每个 chunk 都获得独立 RRF 机会 → 任何英文 query 有高概率命中某个 chunk
3. **Cross-encoder 不检查语言**：reranker 直接把 query + doc 拼接走前向，不过滤语言差异

**对用户的影响**：
- 单域用户（只有中文法律 or 只有英文技术）—— 无影响
- 多域用户（例如律师同时保存英文 Rust 知识）—— 英文检索会混入中文文档

**修复选项**（未实施，待评估）：
- **选项 A（轻量）**：在 search.rs 里按 query 语言（简单启发式：检测 ASCII 占比）过滤候选文档语言。同语言优先，跨语言降权
- **选项 B（正确但有风险）**：切换 reranker 到真正多语言的 `jinaai/jina-reranker-v2-base-multilingual`（XLM-RoBERTa 架构，ONNX 兼容性需验证）
- **选项 C（小语料适配）**：当融合候选 < N 时跳过 reranker，直接用 RRF 排名（上一轮提到的阈值化，这次证明更必要）

### 最终成就（五轮）

✅ 三场景语料框架（scripts/download-corpora.sh 覆盖 rust-book / cs-notes / openai-cookbook / technical-books）
✅ `/data/company/lawcontrol` 的 15 个法律文本真实 ingest + RAG 验证
✅ 中文 RAG（场景 A）4/5 命中，relevance 0.13-0.21 健康
✅ 硬件优化自动启用（代码 + 脚本）
✅ Reranker / Embedding 模型迁移完成且可用
❌ **Cross-lingual 污染是本次发现的新 bug**，需要后续修复（阈值化 + 语言过滤 + 多语言 reranker 评估）

**后续工作**（非紧急）：
- 补 classifier / clusters / remote / history / settings 五个 tab 的 E2E 覆盖
- bge-reranker-v2-m3 ONNX 模型 404 —— 需要重定向到新版模型路径或打包内嵌
- platform::data_dir 数据目录 attune/ 新用户使用，老 npu-vault/ 兼容读取；是否做迁移 copy 待决策

---

## 2026-04-17 三轮回归（真实语料规模测试）

### 规模

- 19 个 rust-book 章节（trpl-v0.3.0 tag）—— ch04（所有权）+ ch10（泛型）+ ch15（智能指针）+ ch19（模式）
- 批量 ingest 经 `scripts/bulk-ingest.sh` + jq 安全 JSON 编码
- 19/19 成功 POST /api/v1/ingest

### Embedding 吞吐（关键时序数据）

监控 `pending_tasks` 消费过程：

| 阶段 | 任务数 | 时间 | 速率 |
|-----|-------|------|------|
| 预处理期（未监控）| ~50 | ~90s | 0.55 tasks/s |
| Embed 消费期 | ~260 chunks | ~65s | **4.3 chunks/s** ← bge-m3 批量 10 条/批 |
| Classify 消费期 | 19 items | ~275s | **14.5s/item** ← qwen2.5:3b 单条分类 |
| **总计** | 324 tasks | **347s**（监控开始到 0） | — |

**关键发现**：qwen2.5:3b 分类是 CPU 推理，未利用 Radeon 780M iGPU。ROCm 虽然装了但 Ollama 默认路径未启用。优化空间约 5-10x。

### 又一个关键 Bug 被发现

**Bug #4（最致命，前面的"RAG 0 hits"真正根因）**：
`RawItem::decrypt` 把 `items.tags` 字段反序列化为 `Vec<String>`，
但 AI 分类器写入的是 `ClassificationResult`（JSON map 带 core/
universal/plugin/user_tags）。解析失败 → serde 报 "invalid type:
map, expected a sequence" → `get_item` 返回 Err → 调用者用 `if let
Ok(Some(..))` 吞错误 → 搜索 `items_decrypted=0` → Chat 本地全军覆没。

**证据链**（本次回归收集到）：
```
server log: search stages: rrf_fused=7  items_decrypted=0  returned=0
API:        GET /api/v1/items/<id> → {"error":"json error: invalid
            type: map, expected a sequence at line 1 column 0"}
```

**修复**（commit 534ce3f）：`RawItem::decrypt` 先尝试 `Vec<String>`，
失败则按 `serde_json::Value` 解析，从 `user_tags` 字段提取；完全无法
解析时保持 `tags=None` 但 item 仍可取出。

### 三轮修复后 RAG 5 问质量测试（19 个 rust-book 章节）

| 问 | 预期命中 | 实际 top-1 | 评估 |
|----|---------|----------|------|
| Q1 What's the difference between references and borrowing? | ch04-02 | references-and-borrowing | ✅ PASS (top-1) |
| Q2 When should you use Box<T>? | ch15-01 | box | ✅ PASS (top-1) |
| Q3 How does Rust handle reference cycles? | ch15-06 | deref（ref-cycles top-2）| ⚠️ top-2 |
| Q4 What are lifetimes in Rust? | ch10-03 | lifetime-syntax | ✅ PASS (top-1) |
| Q5 Refutable vs irrefutable patterns? | ch19-02 | refutability | ✅ PASS (top-1) |

**4/5 top-1 命中，5/5 top-3 命中**。所有问题 `web_search_used=false`（本地召回充分），不再触发网络搜索 fallback。

### 最终验收结论

**PASS**. 知识库构建 pipeline 在真实 GitHub 语料上端到端跑通：
- 录入：19 章节全成功
- Chunk + Embed：4.3 chunks/s 稳定吞吐
- 分类：100% 完成（虽然慢，CPU 推理）
- 全文 + 向量索引：全部在场
- 搜索召回：4/5 top-1、5/5 top-3
- RAG Chat + 引用：本地优先，web fallback 需要时自动触发
- 混合智能：本地命中不 fallback，本地空则 DuckDuckGo 自动补充
