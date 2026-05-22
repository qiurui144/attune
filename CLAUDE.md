# attune

个人知识库 + 记忆增强系统。通过 Chrome 扩展在 AI 对话和日常浏览中自动捕获、检索、注入知识，利用 Ollama / NPU / iGPU 闲置算力处理 embedding。

## ⭐ v1.0 GA Roadmap（2026-05-20 用户拍板，5 天物理时间）

**目标**：**5/25 完成 v1.0 定版，5/26 cloud / wiki-web / official-web 全部上架**。

| 日期 | 版本 | 关键交付 |
|------|------|---------|
| **5/20** today | foundation | 文档清理（33+ 违规文件归并 / 删除）+ 双 CLAUDE.md commit + version/doc audit 脚本收口 |
| **5/21** | **v0.8** | 底层框架固化：**6 现有 law-pro agent 闭环 backfill 全完成**（per "Agent 验证铁律"节）；`agent_golden_gate` 扩展强制 6 类测试下限；cloud 配套基础设施定版 |
| **5/22-23** | **v0.9.0** | **现有规划 agents 接入**：4 个新 law-pro agent（traffic-accident / divorce / sale-contract / housing-rent），每个完整闭环；defamation 推到 v1.1 |
| **5/24** | **v0.9.1** | 修复 sprint + E2E + Playwright + wiki/官网内容填充 |
| **5/25** | **v1.0 GA** | develop → main `--no-ff`；tag `v1.0.0` + `desktop-v1.0.0`；attune-pro `v1.0.0` 配对；cloud `cloud-v2.2.0`（声明兼容 attune v1.0.x） |
| **5/26** | 上架 | cloud SaaS / wiki-web / official-web（均在 cloud 仓内）全部部署上线 |

**核心硬约束**：
- 每个新 agent 必须过 "Agent 验证铁律"（≥10 真实 golden + ≥3 prop + ≥5 boundary + ≥3 异常 + ≥1 E2E + 回归 fixture）
- `agent_golden_gate.rs` 在每个 PR 上跑 1.00 pass rate
- 5/25 GA 前每条 develop → main `--first-parent` 视角必须纯 `merge:` 前缀
- 5/26 上架日 cloud 三个组件（accounts / wiki-web / official-web）必须同时 healthcheck 绿

## 双产品线架构

本仓库包含两条并行的产品线，共享 Chrome 扩展协议（`/api/v1/*`）：

1. **Python 原型线** (`python/src/attune_python/`) — 实验/验证
   - FastAPI + ChromaDB + SQLite FTS5
   - 快速迭代新特性和算法
   - 73 tests，持续增长

2. **Rust 商用线** (`rust/`) — 生产/发布
   - Axum + rusqlite + tantivy + usearch + hdbscan
   - 加密模型：Argon2id + AES-256-GCM + Device Secret
   - 定位：**私有 AI 知识伙伴**（主动进化 + 对话式 + 混合智能，详见 `docs/superpowers/specs/2026-04-17-product-positioning-design.md`）
   - TLS NAS 模式 + 嵌入式 Web UI (8 标签页 + Settings 模态 + Reader 模态) + Chrome 扩展兼容
   - AI 自动分类 + HDBSCAN 聚类 + 编程/法律/专利/售前行业插件
   - 浏览器自动化网络搜索（chromiumoxide 驱动系统 Chrome，零 API 费用）
   - SkillClaw 风格后台自动技能进化（失败信号 → LLM 扩展词 → 静默生效）
   - 行为画像 + 画像导出/导入 + WebDAV 远程目录
   - 237+ tests（210 attune-core + 27 attune-server），独立 README/DEVELOP/RELEASE
   - 最新里程碑：v0.5.x 改名为 Attune + 浏览器搜索重构完成

**测试策略**：`docs/TESTING.md` 固化了产品级测试方案 — 六层测试金字塔、GitHub 真实知识仓库作为语料（rust-lang/book、CyC2018/CS-Notes 等版本固化）、golden set 质量回归、禁止随机测试数据。添加任何 feature 前先参考该文档的测试矩阵。

Python 验证后，择优特性迁移到 Rust 商用线。对应开发时根据任务选择目录：
- 涉及算法实验、ML 集成、快速原型 → 改 Python 端
- 涉及加密、性能、打包分发、生产部署 → 改 Rust 端

## 三产品矩阵 + 边界（与 attune-enterprise、attune-pro 的关系）

> v2 (2026-04-27)：从"独立应用、不依赖 attune-enterprise"修订为**三产品矩阵 + 配套关系**。
> 详见 `docs/oss-pro-strategy.md` v2 决策 2.5（双语）。
> 注：该产品原名 LawControl，自 2026-05-22 改名 Attune Enterprise。

**三产品矩阵**：

| 产品 | 用户群 | 形态 |
|------|--------|------|
| **attune (本仓 OSS)** | 个人通用用户 | 桌面/扩展，纯通用知识库（零行业绑定）|
| **attune-pro** | 个人行业用户 | Plugin pack 装载到 attune（律师 / 医生 / 学者 / 售前 / 工程师 / 专利代理）|
| **attune-enterprise** | 律所 B2B 小团队 | Django + Vue + 19 容器 SaaS（原 LawControl）|

**等式**：
- 个人通用用户 = `attune (OSS)`
- 个人行业用户 = `attune (OSS) + attune-pro/<vertical>-pro`
- 行业小团队 = `attune-enterprise`

**技术上独立**（硬约束保持）：
- **不调用 attune-enterprise 的任何 API / pluginhub / 服务**。attune 必须能在没有 attune-enterprise 部署的环境中完整工作
- **不复用 attune-enterprise 代码**（不同技术栈：attune-enterprise = Python + Django；attune = Rust）
- **数据完全隔离**：attune 的 vault / 批注 / chat / Project 永远在用户本地（或用户自己的 K3），不与任何外部产品同步

**战略上配套**（v2 新增）：
- 同一团队两个产品分工 — B2C 桌面 vs B2B 律所，不是独立竞品
- **可参考 attune-enterprise 设计模式**：plugin.yaml + prompt.md + JSON schema 分离、Intent Router、chat_trigger 路由、Project 卷宗心智、RPA 七类分法（rpa / crawler / search / skill / workflow / channel / industry，AI 边界严守 — 数据层禁用 AI、AI 层走 skill/workflow）— **实现完全独立**
- 公共"行业知识"层（law prompts / case schema）M3+ 商业化时可能放 git submodule (`legal-prompts-pack`) — 与任何单一产品仓分离

**后续互通**（同一律所同时用 attune-enterprise 和 attune）：通过**用户主动 export / import** 完成，不做后台自动桥接。

**OSS attune 边界规则（v0.6.0-rc.2 起）**：

per `docs/oss-pro-strategy.md` v2 §4.3 — 一个功能进 OSS attune 当且仅当它**对任何领域的个人通用用户都有价值**。
行业 (law / patent / sales / tech / medical / academic) **完全在 attune-pro**，不在 OSS。

v0.6.0-rc.2 边界瘦身已删除：
- `assets/plugins/{tech,law,presales,patent}.yaml` (4 个 builtin 行业 yaml)
- `entities.rs::EntityKind::CaseNo` + `extract_case_no` 中文法律案号正则
- `project_recommender.rs::CHAT_TRIGGER_KEYWORDS` 律师专属 const

→ 全部迁到 `attune-pro/plugins/<vertical>-pro/`

## Git push 权限（本仓库特例）

**全局规则禁止 git push，但本仓库（attune-core 开源主线）+ attune-pro 私有商业仓都允许 push**：

授权记录：
- 2026-04-26 attune 公开仓允许 push（开源主线）
- 2026-04-27 **attune-pro 私有仓也允许 push**（商业线接收 OSS 边界瘦身后，需要主动同步远端备份）

允许的 push 操作（attune + attune-pro 都适用）：
- ✅ 允许：`git push origin develop` / `git push origin main` / `git push origin <feature-branch>` / `git push origin <tag>`
- ✅ 允许：`git push` PR 用的 feature 分支

不允许的（任一仓都拒绝）：
- ❌ **不允许**：`git push --force` / `git push --force-with-lease` 到 main 或 develop（其他分支按需问用户）
- ❌ **不允许**：`git push --no-verify` 跳过 pre-commit hook
- ❌ **不允许**：push 任何 attune-enterprise 仓库（独立项目，未授权）

push 前一律 `git status` + `git log --oneline origin/<branch>..HEAD` 复核要推什么；推完报告 commit SHA + 远端 URL。


## Git 分支管理标准（GitFlow Lite）

attune + attune-pro 双仓共用，详细规则见 `DEVELOP.md`「分支模型」。**这是行为标准，不是建议**。

### 两条长期分支

| 分支 | 角色 | 写入方式 | 谁能直接 push |
|------|------|---------|--------------|
| **`main`** | 稳定发布线，对外用户克隆默认看到这条；每个 GA tag 在此打 | **只通过 `develop → main` merge**（`--no-ff`） | ❌ 不直接开发 |
| **`develop`** | 集成线，日常开发汇总；alpha/beta/rc tag 在此打 | feature/* squash merge 进入 | ✅ 小修可直接 commit + push |
| `feature/<name>` | 短期特性，sprint 内用；merge 后**立即删远端 + 本地** | 直接 commit + push + PR → squash merge develop | ✅ |

**铁律**：`main` 的 **first-parent 历史上永远不出现非 merge commit**。所有进 main 的代码必须先经 develop。检查命令必须加 `--first-parent`：

```bash
git log origin/main --first-parent --oneline | head -20
# 这里看到非 merge: 前缀的 commit 才是异常
```

注意：**不加 `--first-parent`** 的 `git log origin/main` 会列出所有可达 commit（含 develop 经 merge bubble 引入的非 merge commit），那些是 GitFlow `--no-ff` 的正常副作用，不是异常。判别异常必须用 first-parent 视角。

### develop → main 的两类合理时机

| 类型 | 触发场景 | 是否打 tag |
|------|---------|----------|
| **发布型 merge** | 准备发新 release（GA / patch / minor） | ✅ 必须，打 `vX.Y.Z` + `desktop-vX.Y.Z` |
| **治理对齐型 merge**（attune 仓允许） | 大量文档/小特性累积，对外 README 与 develop 漂移 | ❌ 不打 tag |

历史先例：`ee8035e` / `4b2c162` / `fdc3ac9` / `364445f` 都是治理对齐型 merge（不打 tag），是允许的模式。判断标准：**对外文档（README / oss-pro-strategy / 定位 spec）的 develop 与 main 漂移已经会让 GitHub 默认分支访客读到错误信息时，即应触发治理对齐 merge**。

### Tag 双轨制（v0.7+ 起明确）

| Tag 前缀 | 触发的 workflow | 产物 |
|---------|----------------|------|
| `vX.Y.Z[-alpha/beta/rc.N]` | `.github/workflows/rust-release.yml` | server/CLI 二进制 tarball（多平台） |
| `desktop-vX.Y.Z[-alpha/beta/rc.N]` | `.github/workflows/desktop-release.yml` | Tauri 桌面安装器（NSIS / MSI / .deb / AppImage） |

**两条线版本号必须保持一致**（如同时发 `v0.6.0` + `desktop-v0.6.0`），共用同一份 `RELEASE.md` changelog。

### Tag 打在哪条分支

- **正式版（`vX.Y.Z` 无后缀）**：**只在 `main`** 打。
- **预发版（`-alpha.N` / `-beta.N` / `-rc.N`）**：在 `develop` 打。
- 历史 tag 状态见 `git tag --sort=-creatordate`；最新 GA 是 `v0.6.0` + `desktop-v0.6.0`（2026-04-28）。

### Merge commit 形态（develop → main 必须 `--no-ff`）

正确：
```bash
git checkout main
git merge --no-ff develop -m "merge: develop → main (<原因>)"
git push origin main
```

**禁止 fast-forward merge**（默认 `git merge` 在历史线性时会 ff-only），会丢失 develop / main 边界。在 attune 仓 `git config merge.ff false` 已配置，但本规则仍是行为标准，避免 CLI override。

### "main 比 develop 多 N 个 commit" 不一定是异常

由 `--no-ff` 产生的 merge commit 本身只在 main 上存在，不在 develop 上。这意味着：
- `git rev-list --count origin/main` 通常 ≥ `develop`
- `git log origin/develop..origin/main --first-parent` 看到的应该**全部是 `merge:` 前缀** — 没有则异常（**必须加 `--first-parent`**，否则会列出 second-parent 上 develop 的非 merge commit，造成误报）
- 真正衡量"main 是否同步"的是 `git log origin/main..origin/develop` —— 这里有 commit 说明 develop 领先未 merge

### 我的执行纪律

任何 git 操作前后：

1. **push 前**：`git status` + `git log --oneline origin/<branch>..HEAD` 复核
2. **push 后**：报告 commit SHA + 远端 URL
3. **不在 main 直接 commit**：哪怕一行 typo 也走 develop → merge
4. **feature 分支用完即删**：squash merge 后 `git push origin --delete feature/<name>` + `git branch -d`
5. **打正式 tag 前必 merge develop → main**：tag 永远在 main 上，不在 develop
6. **tag 一旦 push 视为不可撤销**：除非用户明确同意 + 远端无 release 引用，否则不删 tag
7. **检测异常状态**：`git log origin/develop..origin/main --first-parent` 看到非 `merge:` commit 立刻报警（必须加 `--first-parent`，否则会误报 develop 的合并 commit）


## 技术栈（Python 原型线）

- 后端: FastAPI + Uvicorn, Python 3.11+
- 向量库: ChromaDB (嵌入式, cosine 相似度)
- 全文搜索: SQLite FTS5 + jieba 分词（LIKE 回退）
- Embedding: Ollama bge-m3 (默认) / ONNX Runtime (CPU/DirectML/ROCm) / OpenVINO (Intel NPU/iGPU)
- Chrome 扩展: Manifest V3 + Preact + Vite 多阶段构建
- 打包: PyInstaller + AppImage (Linux) / NSIS (Windows)

## 技术栈（Rust 商用线，rust/）

- 后端: Axum 0.8 + Tokio + rustls TLS
- 数据库: rusqlite + 字段级 AES-256-GCM 加密
- 全文搜索: tantivy 0.22 + tantivy-jieba（中文分词）
- 向量搜索: usearch HNSW + f16 量化
- 加密: argon2 + aes-gcm + zeroize 纯 Rust 密码学
- Web UI: 嵌入式单页 HTML + vanilla JS（`include_str!`）
- CLI: clap + rpassword
- AI 分类: Ollama chat (qwen2.5) + hdbscan 聚类 + 编程/法律插件
- 分发: Rust 主二进制 ~47 MB stripped / 59 MB unstripped（静态链接，含 TLS + 搜索引擎 + Web UI + 分类引擎；R32 实测 2026-05-01 x86_64-linux）；Win MSI / Linux deb 安装包 ~150-200 MB（捆绑 Ollama runtime + whisper.cpp + PP-OCRv5 mobile (ONNX) + poppler-utils + 必要底座模型；**LLM 不本地预装** — 笔电统一 cloud API（Gemini Free / attune Pro 会员 token / 用户 BYOK），K3 一体机镜像例外）

## 已实现模块（Phase 0-3）

### 后端
- `main.py` — lifespan 全链路初始化、路由注册、认证中间件
- `config.py` — YAML 配置 + Pydantic Settings，默认模型 bge-m3, device auto
- `core/embedding.py` — OllamaEmbedding (HTTP API) / ONNXEmbedding / OpenVINO (Phase 4)
- `core/search.py` — RRF 混合搜索引擎 + 两阶段层级检索 (search_relevant) + 动态注入预算
- `core/chunker.py` — 滑动窗口分块 + extract_sections() 语义章节切割
- `core/parser.py` — 文件解析 (MD/TXT/代码/PDF/DOCX) + parse_bytes() 内存解析
- `db/sqlite_db.py` — SQLite (schema/CRUD/FTS5/embedding 队列，含 level/section_idx)
- `db/chroma_db.py` — ChromaDB 封装
- `scheduler/queue.py` — Embedding 队列 Worker (后台线程，metadata 含 level/section_idx)
- `indexer/watcher.py` — watchdog 多目录监听
- `indexer/pipeline.py` — 解析→两层入队（章节 Level1 + 段落块 Level2）→存储→embedding 管道
- `platform/detector.py` — 芯片级硬件检测 + 驱动匹配 + 一键安装命令
- `tray.py` — 系统托盘入口（pystray + uvicorn daemon 线程）
- API: ingest / upload / search / items / index / status / settings / models / ws

### Chrome 扩展
- `content/detector.js` — 平台适配器 (ChatGPT/Claude/Gemini, extractMessage/isComplete/setInputContent)
- `content/capture.js` — MutationObserver 对话捕获 (djb2 去重, 2s debounce)
- `content/indicator.js` — 4 状态指示器 (disabled/processing/captured/offline)
  - 注：原 `content/injector.js`（前缀注入）于 cleanup-r15 删除，产品 2026-04-12 转向内置 Chat + RAG，不再向 AI 网站 DOM 注入
- `background/worker.js` — 消息路由 + 去重缓存 (session storage) + 30s 健康检查 + 会话感知加权
- `popup/Popup.jsx` — 连接状态 / 统计 / 注入开关
- `options/Options.jsx` — 后端地址 / 注入模式 / 排除域名 / 测试连接
- `sidepanel/` — 搜索 / 时间线 / 文件 (拖拽上传, uid 并发安全) / 状态
- `shared/messages.js` — 统一消息类型（含 FILE_UPLOADED）+ 通信辅助
- `shared/api.js` — 后端 API 封装 (动态 baseUrl, 含 uploadFile)

## 产品决策记录

- **Chat 流式输出**：attune Chat 不实现流式输出（SSE streaming）。等待 LLM 响应期间，Web UI 显示加载指示器（spinner）即可。原因：本地 0.6B-3B 模型响应快，云端 API 等待时有 loading 状态满足体验需求，实现复杂度不值得。
- **三产品矩阵：attune × attune-pro × attune-enterprise**（2026-04-27 v2，从"独立应用"演进而来；attune-enterprise 原名 LawControl，2026-05-22 改名）：attune (OSS 通用) + attune-pro (个人行业增强) + attune-enterprise (B2B 小团队)。技术上独立运行；战略上配套分工。可参考 attune-enterprise plugin / RPA / Intent Router 设计模式，但实现完全独立。详见 `docs/oss-pro-strategy.md` v2 决策 2.5 + 上文「三产品矩阵 + 边界」。
- **行业版第一刀切律师**（2026-04-25）：复用 attune-pro 已有 5 个 law-pro skill + 自研 RPA + Project/Case 卷宗。会员制 SaaS（个人版 / 专业版）+ 一体机（K3）双形态。
- **本地 AI 底座边界**（2026-04-25）：attune 不是"全本地 AI"，是"**降低 token + 数据安全**"。本地仅捆绑必要底座（Embedding / Rerank / ASR / OCR + Ollama runtime），**LLM 模型不捆绑**，LLM 走远端 token 默认；K3 一体机形态可选装本地 LLM。
- **平台优先级**（2026-04-25，2026-05-21 修正 K3 架构）：**Windows P0 → Linux x86_64 P1 → macOS 暂不做**。**K3 一体机 = riscv64 RVA23**（SpacemiT K3 X100 SoC，VLEN=256，**非 aarch64**），走 `/data/RV/rv-gcc/install-15.2/` 交叉编译 + 镜像化部署，不走 .deb workflow。Win MSI + Linux deb/AppImage 双轨，K3 单独镜像（per `/home/qiurui/.claude/CLAUDE.md` § RISC-V 验证与优化规范）。
- **ASR 引擎**（2026-04-25）：whisper.cpp binary + Rust subprocess（与 K3 推理服务一致路径），中文 WER 必须 < 20%（whisper-small Q8 实测满足）才能选默认模型；whisper-tiny WER 35-40% 不可用。

## 磁盘资源管理铁律（强制 — 2026-05-21 用户重申，盘满过踩坑）

**触发场景**:任何 cargo build/test 完成、worktree merge 完成、agent dispatch 完成的时刻,
**立即检查 + 清理**。盘已经被打到 / 100% + /data 99% 一次,根因是 target/ 累积 184G + 93G + 8 个未清 worktree 65G。

### 红黄绿线(df 检查)

| df `/data` available | 状态 | 行动 |
|---|---|---|
| > 200G | 🟢 正常 | 例行操作 |
| 50-200G | 🟡 黄线 | 完成的 worktree 立即清,陈旧 target 评估 |
| < 50G | 🔴 红线 | 全停手,清 target + 已 merge worktree + 大文件 audit;无空间时不允许新 dispatch agent |

任何大 sprint(多 agent 并发)之前 + 之后 都跑 `df -h /data` 看。

### target/ 清理

- **cargo build/test 完成后,若未来 1 小时内无再 build 需求 → 立即 `cargo clean`**
- 主 worktree target/ 可达 100+GB(184G attune / 93G attune-pro 实测)
- 各 isolated worktree 的 target/ 在 `git worktree remove` 时自动清
- 不要"留着备用 cache" — cargo incremental 重 build 也快,主 target 200G 不值得占盘

### worktree 清理

- **agent done + branch merged → 立即 `git worktree remove -f -f <path>` + `git worktree prune`**
- 不积累(每个 3-20GB,8 个 = 100+GB)
- 周期 `git worktree list` 检查遗留
- locked worktree 用 `-f -f`(双 force)解锁删除

### staging merge worktrees

- 我自建的 `/tmp/attune-pro-merge*/` 等 staging worktrees,merge commit 完立即 `git worktree remove`
- 不留累计

### 反模式

- ❌ "下次还要 build,留着 target"(实测 cargo clean 后重 build 才几分钟,vs 占 200GB 盘)
- ❌ "worktree 留着以后参考"(用 `git log` + `git show` 即可,worktree 不是 history)
- ❌ "merge 完不清 worktree"(上一个 sprint 累计 100GB 就这样来的)
- ❌ dispatch 新 agent 前不查 df(空间不够会让 agent 跑到一半 disk full crash)

## Agent 验证铁律（强制 — 2026-05-20 用户重申）

**核心定位**：**Agents 是 attune 生态最重要的附加功能，也是核心功能来源之一**。
任何 agent（无论 free / pro tier，无论 deterministic / LLM-judgement / lawyer-rule）
shipping 前必须完成**闭环验证**（test → fix → verify），不可仅满足于"有 tests"
或"clippy 干净"。

**铁律实施基准**：`attune-pro/plugins/law-pro/docs/agent-skill-training-methodology.md`
（866 行，2026-05-20 落地）+ `attune-pro/CLAUDE.md` 「Agent 验证铁律」节是规则全文。
本节是 OSS attune 端的执行清单 + 同等纪律承诺。

### 闭环 = 4 步全过，缺一即不合格

1. **覆盖测试**：每 agent ≥10 真实 golden case（YAML in `tests/golden/<prefix>-N.yaml`）
2. **真实测试发现 bug**：首跑全过 = 八成是 ground truth 由 agent 自己生成 / 测的不是
   invariant —— 必须深挖
3. **修复迭代**：bug → 写 reproducer fixture（独立计算 GT，不调 `agent.calculate()`）
   → 修 agent → 跑 fixture 过 → commit 把 reproducer + fix 一起入库
4. **验证锁定**：fixture 进 golden set；阈值 ratchet 只升不降；
   `agent_golden_gate.rs` 是日常 CI 硬门

### 6 类测试覆盖下限（强制）

| 类型 | 下限 | 工具 |
|------|------|------|
| Golden case | ≥10 真实 + 1 sentinel | YAML fixture |
| 属性测试 | ≥3 per agent | `proptest` |
| 边界 case | ≥5 `#[test]` | inline `#[cfg(test)]` |
| 异常 / 错误 | ≥3 case | YAML `expected_error` |
| 集成 E2E | ≥1 subprocess | `tests/<agent>_subprocess.rs` |
| 回归 fixture | 每修一个 bug 加 1 | golden set 永久 |

**未到下限禁止 PR merge**。

### Free vs Pro 同纪律

OSS attune **当前没有 domain agent**（chat / search / RAG 是 base capability 不是
agent）。**未来 OSS 加 agent 时同走此纪律**，agent_golden_gate.rs 等价 harness
应当从 attune-pro 复制到 attune-core。free / pro 共用同一套 reliability framework，
不存在"free 因为是 OSS 所以纪律松"的合理化空间。

### 反模式（违反即拒绝）

- ❌ "我加了一个 agent 还没写测试" → 同 PR 必须含测试
- ❌ "测试都过了没发现 bug" → 测试不够难，继续加 case
- ❌ "这是 free agent 不用走 framework" → 是 free 也要走
- ❌ "ground truth 用 LLM 生成" → agent 自检自己，gate 形同虚设
- ❌ 阈值下调绕过失败 → ratchet rule，只升不降
- ❌ "暂跳过这个失败 case" → 要么修 agent，要么修 GT（后者需 lawyer / domain expert 签字）

### PR 纪律

涉及 agent 的 PR commit msg 必须含 **"test-fix-verify"** + 引用本节 +
`agent_golden_gate.rs` 在 PR 上 1.00 pass rate（deterministic）或 F1 ≥0.85（LLM）。

## 成本感知与触发契约（Cost & Trigger Contract）

Attune 的每一次计算都要分清楚"谁在买单"，UI 里必须让用户一眼看到。这是贯穿整个产品的最高优先原则，与 1Password 式"私密"、混合智能式"本地优先"并列。

### 三层成本

| 层级 | 资源 | 触发策略 | 例子 |
|------|------|---------|------|
| 🆓 **零成本** | CPU，毫秒级 | 随便跑 | 文件解析 · 分词 · BM25/tantivy 检索 · OCR (PP-OCRv5 mobile via ORT) |
| ⚡ **本地算力** | GPU/NPU，秒级 | 建库阶段自动跑；顶栏有"暂停后台任务"开关 | embedding 生成 · 基础 classify (tag/cluster) · 一次性 150 字存档摘要 |
| 💰 **时间/金钱** | LLM（本地或云端），秒到分钟 | **必须用户显式触发**（敲回车/点按钮），**永不后台偷跑** | Chat 问答 · AI 批注 · 深度分析 · 云端 API 调用 |

### 核心规则

1. **建库阶段永远不升级到第三层**。文件进入（upload / 文件夹监听）只跑到"能被搜到 + 有 150 字存档摘要"为止。深度摘要、观点提取、批注建议都属于分析阶段。
2. **分析阶段永远等用户开口**。不做"AI 主动建议下一个问题"、"AI 猜你需要什么"这类产品行为 — 用户时间和 API 费用都太贵。
3. **UI 必须显示成本**：
   - Chat 发送按钮旁常驻 `~1.2K tok · $0.0004`（本地模型显示 `~本地 · 2s`）；点开展开所选上下文
   - 每个 AI 分析按钮标注**本地/云端 + 预估耗时/花费**
   - 顶栏后台任务队列**可见 + 可暂停**
4. **摘要缓存不可跳过**：每个 chunk 生成的摘要按 chunk_hash 入缓存；批注变更使"含批注视角摘要"作废，保留"原文摘要"那份。
5. **批注 source 是状态不是分类**：`user`（默认）/ `ai`（被 AI 处理后变）。用户再手动编辑则回到 `user`。所有批注可删。UI 上两种小圆点区分颜色，不做"发布/撤回"协作概念。

### 硬件感知的默认底座

启动时检测 RAM/GPU/NPU，推荐**本地底座模型**（embedding / rerank / ASR / OCR）。**LLM 默认走远端 token，不在本地预装**（M2 决策）。Settings 里展示"根据你的硬件推荐，可更换"：

| RAM | GPU/NPU | 默认 embedding | 默认 ASR | 默认 chat/摘要 |
|-----|---------|----------------|----------|----------------|
| ≥16 GB | 独显/NPU | `bge-m3` (Ollama) | whisper-large-v3-turbo Q5 (中文 WER 5-7%) | 远端 token |
| 16-32 GB | 核显/NPU | `bge-base` (ORT) | whisper-large-v3-turbo Q5 | 远端 token |
| 8-16 GB | 核显 | `bge-small` (ORT) | whisper-medium Q5 (480 MB, WER 10-12%) | 远端 token |
| <8 GB | 仅 CPU | `bge-small` (ORT) | whisper-medium Q5 | 远端 token |

**ASR 升级（2026-05-01 用户拍板）**：不轻易降级到 small-q8。large-v3-turbo 是 OpenAI 2024-10 sota（中文 WER 5-7% vs small 15-20%；turbo 比 large-v3 快 8x）。低 tier 笔电 medium-q5 兜底（480 MB / WER 10-12%，仍优于 small）。

**LLM 提供商策略（2026-05-01 用户拍板，澄清版）**：

核心原则：**云端为主，本地为辅；本地 LLM 当前研发成本过高，暂时不走主推**。云端解决不了的场景（隐私敏感 / 离线）才回头优化本地。

Wizard 推荐顺序：
1. **★ Attune Pro Membership Gateway**（登录即用）
   - Endpoint: `https://gateway.attune.ai/v1`
   - 标准 SaaS 会员：登录即追踪 token 用量配额
   - 由 attune-pro 仓 LLM gateway 实装（路由到 OpenAI / Anthropic / Gemini，对用户透明）
   - 主推方案 — 降低上手摩擦
2. **BYOK：用户已有付费会员的 API key**
   - ChatGPT Plus/Team → OpenAI API key
   - Claude Pro → Anthropic API key
   - Gemini Advanced → Gemini API key（Google AI Studio）
   - DeepSeek / Qwen / 其他 OpenAI 兼容
3. **本地 Ollama**（K3 一体机镜像 + 笔电 advanced 用户）
   - K3 form factor 镜像构建时预装 qwen2.5:1.5b/3b
   - 笔电用户 wizard 选择 Ollama 时手动 ollama pull
   - 当前**不主推** — 研发成本高（ROCm 配置 / 模型选型 / 推理优化）

**不走的路径**：
- ❌ 第三方 "free API tier"（Gemini Free / Groq Free 等）— 避免误导用户"免费"，且研发成本高
- ❌ MCP backbone（attune 作为 ChatGPT Desktop 的 context provider）— 至少 v0.7 不做
- ❌ 浏览器扩展给 web AI 注入 context — cleanup-r15 已删，产品方向改为内置 Chat

"用户的免费 AI 会员"指 → 浏览器内 ChatGPT.com / Claude.ai / Gemini Advanced **web 会话**，但 attune 不直接对接（不走 MCP / 不注入浏览器）；用户如果有 web 会员，对应付费 plan 通常自带 API quota → 走 BYOK 路径。

**K3 一体机形态**：底座由 K3 推理服务提供（参考 `docs/k3-ai-service/`）；LLM 可选装本地（qwen2.5:1.5b/3b 实测 K3 上可跑），但默认仍是远端 token。

### 前端范式

Settings UI 采用 ChatGPT/Gemini/Claude 共同范式：模态对话框（左 tab 栏 + 右内容面板），每个 tab ≤4 项，toggle/radio 为主。模型选择不埋在 Settings 里，放在**对话框头部 chip**，点开下拉换模型。锁定 Vault 按钮在**全局顶栏常驻**。删除所有"搜索引擎下拉（只一个选项）、RRF 权重、注入预算"等技术字段 — 普通用户不该看到。

## 开发规范

### Python 原型线
- Python 代码使用 ruff 格式化和 lint（line-length=120）
- 类型注解: 所有公开函数必须有类型注解
- 测试放 `tests/` 目录, 使用 pytest
- **扩展 E2E 测试使用 Playwright 真 Chrome (`channel="chrome"`), 禁止退化到 Chromium** (per CLAUDE.md MCP 限制, FIX-4 已落实)
- 调试代码放 `tmp/`, 使用后删除
- 使用 venv 管理 Python 依赖
- pip 使用清华源

### 通用
- API 路径前缀: `/api/v1/`
- 后端端口: 18900
- **API path 命名 kebab-case** (per OPT-5). 新 path 必须 kebab; 旧 snake path 保留 alias 1 release 周期后删

### Web UI 国际化（i18n）规范（强制 — 2026-05-15 确立，杜绝中英混杂）

**问题**：`attune-server/ui/src/` 下多个视图把界面文案**硬编码成中文字面量**，绕过 `t()`。
这些字符串永远显示中文；而 wizard / Sidebar / 组件走 `t()` 会随 locale 切换 —— 当
locale=en（英文浏览器 / 用户切英文）时就出现「英文外壳 + 中文视图」的中英混杂。

**根因**：i18n key 表（`i18n/zh.ts` + `i18n/en.ts`）本身齐全（key 集合一致），但 `.tsx`
里大量 `toast('error', '保存失败')` / `title="新建项目"` / `placeholder="如：..."` / JSX
文本节点 `<span>刷新</span>` 没有进 `t()`。i18n 引擎 `t()` 缺 key 时按 zh→key 兜底，
所以「漏写一个 locale」也会静默显示错语言。

**铁律（所有 UI 改动强制遵守）**：

1. **任何用户可见字符串必须走 `t()`**，零硬编码。易漏点全覆盖：
   - JSX 文本节点：`<span>刷新</span>` → `<span>{t('common.refresh')}</span>`
   - 属性：`title=` / `placeholder=` / `label=` / `description=` / `aria-label=`
   - `toast(type, msg)` 的 `msg` 参数
   - 按钮文案、表头 `<th>`、`EmptyState` 的 title/description、`error` 提示文案
   - 动态拼接用 `{param}` 插值（`t('x.created', {name})`），禁止 `'已创建：' + name`
2. **新增 key 必须同时写入 `zh.ts` 和 `en.ts`**，两文件 key 集合永远完全一致。
   只写一个 → 另一 locale 静默 fallback 显示错语言。
3. **注释不受限** —— `//` `/* */` 里的中文是开发注释、不是 UI，无需处理。
4. **language-neutral 值例外**：品牌名（Attune）、技术术语（AI/OCR/WebDAV/LPR）、URL、
   纯数字 —— zh/en 两边值可相同，但**仍必须建 key、走 `t()`**，不可硬编码。

**提交前自检（强制 grep 守卫，两条命令都必须无输出）**：
```bash
cd rust/crates/attune-server/ui/src
# (1) 硬编码中文 UI 字面量（toast / 属性值 / JSX 文本节点）
grep -rnP "(toast\([^)]*'[^']*[\x{4e00}-\x{9fff}]|(title|placeholder|label|description|aria-label)=\"[^\"]*[\x{4e00}-\x{9fff}]|>[^<{]*[\x{4e00}-\x{9fff}])" --include="*.tsx" . | grep -v "/i18n/"
# (2) zh / en key 集合必须一致
diff <(grep -oP "^\s+'[\w.]+'\s*:" i18n/zh.ts | tr -d " ':" | sort) \
     <(grep -oP "^\s+'[\w.]+'\s*:" i18n/en.ts | tr -d " ':" | sort)
```
有输出 = 引入了中英混杂或 key 不齐，必须修掉再提交。

**存量债务**：2026-05-15 审计约 100 处硬编码中文待迁移（ProjectsView / SkillsView /
MarketplaceView / SettingsView / Step3LLM / Step4Hardware 等）。**新代码严禁再增**；
存量按视图逐个迁移，迁完一个视图即在该视图内 grep 守卫归零。

### Rust 商用线约定 (v0.7 sprint 增量：记忆护城河)

**文档生命周期协调（v0.7 新规）**:
- 任何写 items.content 的 path（upload / update / scanner / webdav / ingest）**必须**通过 `attune-core::reindex` 模块走完整 pipeline，禁止直接调 `store.update_item` 后不 reindex
- 新加 update path 前先看 `routes/items.rs::update_item` 和 `routes/upload.rs::upload_file` 现有 5 步：（1）算 content_hash（2）短路判断（3）DB update（4）若 content_changed → reindex_item 或 enqueue_reindex（5）写 doc_* 信号到 skill_signals
- attune-core 后台 worker（scanner / scanner_webdav / 任何拿不到 server lock 的）**不要**自己调 vectors / fulltext API，**必须**通过 `store.enqueue_reindex(item_id, action)` 让 server worker 间接处理
- `vectors::delete_by_item_id` + `fulltext::delete_document` 现在通过 reindex 模块统一调，**不再单独直调**

**Lock ordering（防死锁）**:
- 顺序：`vault.lock()` → `vectors.lock()` → `fulltext.lock()` → `embedding.lock()`
- 反顺序持锁是死锁高风险路径；route handler 同时拿多锁前先 release vault guard，或保证顺序一致
- `start_reindex_worker` 内部已经按此顺序，新加 worker 沿用相同模式

**自学习信号约定**:
- 用 `Store::record_signal_event(kind, ref_id, query_opt)` 写新信号；kind 必须从已知集选（doc_create / doc_update / doc_delete / citation_hit / annotation_marker / search_miss）
- `record_skill_signal(query, knowledge_count, web_used)` 是老 API，仅用于 search_miss 场景（向后兼容）
- 失败时静默忽略（`let _ = ...`），永不阻塞主流程

**content_hash 短路条件**:
- update_item 内部已做（hash 相同 → 不重写 BLOB / 不返回 content_changed）
- upload.rs 入口做（hash 命中 → 返回 status=duplicate 跳过 insert）
- 老 vault content_hash='' 视为"未 backfill"，update_item 时 lazy 填回；'' 不参与 `find_item_by_content_hash` 命中

### Rust 商用线约定 (v0.6.3 sprint 确立)

**错误处理**:
- 新 routes 用 `attune_server::error::{AppError, AppResult}` + `?` 链, 统一返回 `{"error": msg, "code": kebab}` JSON shape
- 旧 routes `(StatusCode, Json)` tuple-style 渐进 migration, 不阻塞 build
- `AppError::From<VaultError>` 自动映射 `Locked → 401 / NotFound → 404 / Sealed → 503 / AlreadyInitialized → 409` 等; 客户端 `code` 字段稳定可针对处理

**Async-safe fs**:
- async handler 内禁止直接调 `std::fs::*` (会阻塞 tokio worker)
- 用 `attune_core::async_fs::{read, read_to_string, write, create_dir_all, try_exists, remove_file_if_exists}` (spawn_blocking 包装)
- sync 上下文 (CLI / 启动 init / long-running worker thread) 可继续用 `std::fs`

**State 访问**:
- AppState 19 个 ML provider / 索引字段, 新代码用 accessor 方法 `state.embedding()` / `state.llm()` / `state.set_embedding()` 等 (lock+clone Arc, µs 临界区)
- 不直接调 `state.embedding.lock()` — 后续 v0.7 字段类型会改 `ArcSwap`, accessor 不变, 直接 .lock() 调用会编译失败

**SQL prepare cache**:
- 写新 rusqlite 查询用 `conn.prepare_cached(static_sql)` (rusqlite stmt cache, 复用编译过的 stmt object)
- 动态 SQL (含变量拼接) 必须 `conn.prepare(&sql)` 不能 cache

**测试隔离**:
- 集成测试 (`crates/*/tests/*.rs`) 涉及 sysinfo / 系统负载 / 平台路径 / timing → 用 mock / `cfg(unix)` 隔离 / 显式 `MockMonitor`. CI 高负载 runner 上踩过坑 (governor_integration / index_path_test)
- timing-sensitive 测试 (`thread::sleep`) 用 retry-with-deadline 而非 fixed sleep

**Build profile**:
- workspace [profile.dev.package."*"] opt-level=1 — dev test 速度 5x
- workspace [profile.release] lto=thin codegen-units=1 strip=symbols — 二进制 -15%, perf +5%

## 项目结构

- `python/src/attune_python/` — Python 后端
- `extension/` — Chrome 扩展（Manifest V3 + Preact + Vite）
- `packaging/` — 打包配置（PyInstaller/AppImage/NSIS）
- `.github/workflows/` — CI/CD
- `tests/` — 测试代码 + conftest.py
- `docs/screenshots/<topic>/` — 文档/验证用截图（committed）
- `.playwright-mcp/` — Playwright MCP 临时工作目录（gitignored）
- `tmp/` — 临时调试，用完即删（gitignored）

## 截图存放规范（Playwright MCP / 手工截图）

**规则**：截图**禁止**直接写仓库根目录。`/.gitignore` 已锁 `/*.png` + `.playwright-mcp/`，
但 working tree 60+ 散落 png 仍是 cruft. 新截图按用途分目录：

| 用途 | 位置 | 是否 commit |
|------|------|-------------|
| 文档嵌入（doc 引用 ![]） | `docs/screenshots/<topic>/<name>.png` | ✅ commit, kebab-case |
| E2E 视觉回归 baseline | `tests/screenshots/<test-name>/baselines/` | ✅ commit |
| 一次性验证（GA verify / FEAT 验收） | `docs/screenshots/<release>-verification/` | ✅ commit, 带 release tag |
| 调试 / 临时 | `.playwright-mcp/` 或 `tmp/` | ❌ gitignored, 用完删 |

**MCP 工具用法**（避免根目录污染）:
```js
// ❌ 错: filename 是相对路径, 写到 cwd = 仓库根目录
browser_take_screenshot({ filename: "lock-screen.png" })

// ✅ 对: 显式路径到 docs/screenshots/<topic>/ 或 .playwright-mcp/
browser_take_screenshot({ filename: ".playwright-mcp/lock-screen.png" })
// 或归档版本
browser_take_screenshot({ filename: "docs/screenshots/v063-ga-verification/lock-screen.png" })
```

**示例 — v0.6.3 GA 截图归档**（本会话 7 个）:
```
docs/screenshots/v063-ga-verification/
├── attune-v063-01-lock-screen.png
├── attune-v063-02-main-chat.png
├── attune-v063-03-settings-member-cloud.png
├── attune-v063-FEAT1-cloud-endpoint-expanded.png
├── attune-v063-main-after-wizard.png
├── attune-v063-wizard-step1.png
└── attune-v063-wizard-step2-password.png
```

`docs/wizard-flow.md` 可 `![](screenshots/v063-ga-verification/attune-v063-wizard-step1.png)` 引用.

## Rust 商用线跨平台兼容规范

### 目标平台矩阵

attune 必须在以下平台 + 硬件组合上可编译、可运行、测试通过（按优先级排序）：

| 优先级 | 平台 | 架构 | Rust target | 状态 |
|--------|------|------|-------------|------|
| **P0** | Windows x86_64 | Intel/AMD CPU | `x86_64-pc-windows-msvc` | 待验证（v0.6 GA 前必须可用） |
| **P0** | Windows x86_64 + NVIDIA GPU | + CUDA GPU | 同上，Ollama 用 GPU | 待验证 |
| P1 | Linux x86_64 | Intel/AMD CPU | `x86_64-unknown-linux-gnu` | 主开发平台 ✅ |
| P1 | Linux x86_64 + NVIDIA GPU | + CUDA GPU | 同上，Ollama 用 GPU | 验证 |
| P2 | Linux **riscv64**（K3 一体机） | RISC-V RVA23（SpacemiT K3 X100，VLEN=256） | `riscv64gc-unknown-linux-gnu` | 走 `/data/RV/rv-gcc/install-15.2/` + rv-baseos sysroot 交叉编译，**镜像化部署**（非 .deb） |
| **暂不做** | macOS | x86_64 + arm64 Universal | `*-apple-darwin` | 资源后置，不投入 v0.6/v0.7 |
| **暂不做** | Linux aarch64 | ARM64（NAS / 通用 ARM） | `aarch64-unknown-linux-gnu` | 非 K3，无明确需求，v1.x 不投入 |

### 跨平台编译注意事项

**纯 Rust 依赖**（零跨平台风险）：
- argon2, aes-gcm, zeroize, hmac, sha2 — 纯 Rust 密码学
- tantivy, tantivy-jieba — 纯 Rust 全文搜索
- hdbscan — 纯 Rust 聚类
- axum, tokio, tower-http, reqwest, rustls — 纯 Rust 网络栈
- serde, serde_json, serde_yaml, clap, chrono, uuid — 纯 Rust 工具

**含 C/C++ 绑定的依赖**（需要交叉编译工具链）：
- `rusqlite (bundled)` — 内嵌 SQLite C 源码编译，需要 C 编译器（`cc` crate 自动检测）
- `usearch` — C++ HNSW 实现，需要 C++ 编译器，Windows 需要 MSVC

**交叉编译指南**：
```bash
# Linux → Windows (需要 mingw-w64 或 MSVC 交叉编译器)
rustup target add x86_64-pc-windows-gnu
# usearch 的 C++ 代码可能需要额外配置，建议在 Windows 原生编译

# Linux → riscv64 (K3 一体机, RVA23) — 走 rv-gcc 15.2 + rv-baseos sysroot
# per /home/qiurui/.claude/CLAUDE.md § RISC-V 验证与优化规范
source /data/RV/rva23-qemu/toolchain/env.sh   # $RVA23_CC / $RVA23_CROSS_SYSROOT
rustup target add riscv64gc-unknown-linux-gnu
CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_LINKER=$RVA23_CC \
CC_riscv64gc_unknown_linux_gnu=$RVA23_CC \
CFLAGS_riscv64gc_unknown_linux_gnu="--sysroot=$RVA23_CROSS_SYSROOT $RVA23_MARCH_RVA23" \
  cargo build --target riscv64gc-unknown-linux-gnu --release
# K3 一体机产物**不走 .deb**,走镜像化部署(参考 docs/k3-ai-service/)
```

### GPU / NPU 兼容性

**核心原则**：attune **本身不直接使用 GPU/NPU**。AI 推理走两条路径：

1. **HTTP（Ollama）** — Embedding / Rerank / Chat / Classify。Ollama 自动选 CPU/CUDA/ROCm/Metal 后端
2. **Subprocess（捆绑二进制）** — ASR (whisper.cpp) / OCR (PP-OCRv5 mobile (ONNX) + poppler)。安装包捆绑预编译二进制，attune 子进程调用
3. **HTTP（K3 推理服务）** — K3 一体机形态时，所有底座可走 K3 :8080（参考 `docs/k3-ai-service/`）

| 后端组合 | Embedding/LLM | ASR | OCR |
|----------|---------------|-----|-----|
| NVIDIA GPU + Ollama | Ollama CUDA | whisper.cpp CPU（CUDA build 可选） | PP-OCR (ORT, CPU/GPU 自动) |
| AMD GPU + Ollama | Ollama ROCm | whisper.cpp CPU | PP-OCR (ORT, CPU/GPU 自动) |
| Intel iGPU/NPU + Ollama | Ollama OpenVINO（实验） | whisper.cpp CPU | PP-OCR (ORT, CPU/GPU 自动) |
| 纯 CPU | Ollama CPU（qwen2.5:3b 远端 / 本地按需） | whisper-small Q8 ~3-5s/段 | PP-OCR (ORT, CPU/GPU 自动) |
| K3 一体机 | K3 :8080（IME/RVV） | K3 :8080（whisper Q8 IME） | K3 :8080（PPOCRv5） |

**开发时不需要 GPU**：测试使用 `MockLlmProvider` / `MockEmbeddingProvider` / `MockAsrProvider`，CI 无需 GPU。

### Ollama 多平台安装

| 平台 | 安装命令 | GPU 自动检测 |
|------|---------|-------------|
| Linux | `curl -fsSL https://ollama.com/install.sh \| sh` | NVIDIA (CUDA), AMD (ROCm) |
| Windows | 下载 OllamaSetup.exe | NVIDIA (CUDA) |
| macOS | `brew install ollama` 或下载 .dmg | Apple Silicon (Metal) |

安装后统一使用：
```bash
ollama pull bge-m3      # embedding 模型
ollama pull qwen2.5:3b  # chat/分类模型
```

### Rust 代码跨平台规范

在 attune 的 Rust 代码中，必须遵守以下跨平台规则：

1. **文件路径**: 使用 `std::path::PathBuf` 和 `dirs` crate，禁止硬编码 `/` 或 `\`
2. **权限**: `#[cfg(unix)]` 保护 `set_permissions(0o600)` 等 Unix 特有调用
3. **进程管理**: 使用 `std::process::Command` 跨平台，不依赖 shell 特性
4. **网络**: 使用 `reqwest` + `rustls`（纯 Rust TLS），不依赖系统 OpenSSL
5. **临时文件**: 使用 `tempfile` crate，不硬编码 `/tmp`
6. **换行符**: 文件解析不假设 `\n`，使用 `.lines()` 方法自动处理 `\r\n`
7. **编码**: 文件读取使用 `String::from_utf8_lossy` 容错，不 panic
8. **C/C++ 依赖**: `rusqlite` 用 `bundled` feature 自带 SQLite；`usearch` 需要 C++ 编译器，CI 矩阵必须验证
9. **条件编译**:
   ```rust
   // 正确: 用 cfg 保护平台特定代码
   #[cfg(unix)]
   { std::fs::set_permissions(&path, Permissions::from_mode(0o600))?; }
   
   // 错误: 不要直接调用 Unix API
   // std::os::unix::fs::PermissionsExt  // 仅在 #[cfg(unix)] 内使用
   ```

### CI 构建矩阵（规划）

```yaml
strategy:
  matrix:
    include:
      - os: ubuntu-latest
        target: x86_64-unknown-linux-gnu
        name: Linux x86_64
      # K3 一体机走 rv-gcc 15.2 交叉编译 + 镜像化,**不进 CI matrix**(GH runner 无 RISC-V)
      # 见上文「跨平台编译指南」riscv64gc-unknown-linux-gnu 路径
      - os: windows-latest
        target: x86_64-pc-windows-msvc
        name: Windows x86_64
```

每个 target 需要：
1. `cargo build --target $target --release` — 编译通过
2. `cargo test` — 仅在 native target 运行（交叉编译不跑测试）
3. 产物上传为 release artifact

### 测试隔离规范

所有测试必须满足以下跨平台约束：
- 使用 `tempfile::TempDir` 创建临时目录，不依赖 `/tmp`
- 不假设 Ollama 可用 — 使用 `MockLlmProvider` / `NoopProvider`
- 不假设 GPU 存在 — 纯 CPU 测试
- 不使用 `std::process::Command("sh")` — 如果需要进程交互，用跨平台方式
- SQLite `PRAGMA` 在所有平台行为一致（WAL 模式在 Windows/Linux 都支持）

## 芯片-驱动匹配

detector.py 中维护了精确匹配表:
- Intel: INTEL_NPU_CHIPS (meteor_lake/lunar_lake/arrow_lake) + INTEL_IGPU_CHIPS (alder~arrow)
- AMD: AMD_NPU_CHIPS (phoenix/hawk_point/strix_point/krackan_point)
- 每个芯片条目包含: PCI ID、最低内核版本、固件路径、最低驱动版本、已知问题
- /models/check API 输出完整检测报告 + 一键安装命令
