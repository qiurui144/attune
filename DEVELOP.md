# 开发指南

> **双产品线说明**：
> - 本文档覆盖 **Python 原型线**（`src/npu_webhook/`），用于快速验证和算法实验
> - **Rust 商用线**位于 [`rust/`](rust/)，有独立的 [`DEVELOP.md`](rust/DEVELOP.md)
> - 两者共享 API 协议（`/api/v1/*`），Chrome 扩展可任意切换后端

## 分支模型（GitFlow Lite）

仓库采用简化的 GitFlow，**只有两条长期分支**：

| 分支 | 用途 | 推送方式 |
|------|------|---------|
| `main` | **稳定发布线**。每次合入对应一个 git tag（`vX.Y.Z`）。生产部署、外部用户安装包从这里出。 | 仅通过 `develop → main` PR + tag 合入 |
| `develop` | **集成线**。日常开发汇总，所有 feature/* 在这里集成验证后再升 `main`。 | 仅通过 `feature/* → develop` PR 合入 |
| `feature/<name>` | **短期特性分支**。一个 feature/sprint 一条，merge 后**立即删除**（远端 + 本地）。命名约定：`feature/sprint-N-<thing>` 或 `feature/<topic>`。 | 本地开发 → push → PR → squash merge |

### Tag 时机

- **`vX.Y.Z-alpha.N`**：`develop` 上完成一个 sprint 的成果聚合，先打 alpha 跑内部 dogfood / Playwright E2E。例：`v0.6.0-alpha.1`
- **`vX.Y.Z-beta.N`**：alpha 修完反馈后，外部小范围灰度
- **`vX.Y.Z-rc.N`**：候选发布，准备合入 `main`
- **`vX.Y.Z`**：正式发布。**只在 `main` 分支打**。tag message 列出累积 commit 数 + 核心能力清单 + 测试统计

### Tag 双轨制（v0.7+ 起明确）

attune 的发布产物分两条独立线，对应**两个独立 tag 命名空间 + 两个独立 CI workflow**：

| Tag 前缀 | 触发的 workflow | 产物 | 适用场景 |
|---------|----------------|------|---------|
| `vX.Y.Z[-alpha/beta/rc.N]` | `.github/workflows/rust-release.yml` | **server / cli 二进制 tarball**（attune-server-headless + attune CLI），跨 Linux x86_64/aarch64 + macOS x86_64/arm64 + Windows x86_64 | 开发者、NAS 部署、服务端、headless 场景 |
| `desktop-vX.Y.Z[-alpha/beta/rc.N]` | `.github/workflows/desktop-release.yml` | **Tauri 桌面安装器**（NSIS / MSI / .deb / AppImage） | 终端用户桌面安装（含 Web UI） |

**两条线版本号必须保持一致**（如同时发 v0.6.0 + desktop-v0.6.0），用同一份 RELEASE.md changelog。

### Release Checklist（GA 发布流程）

候选 release（rc.N）通过 dogfood 后，正式 GA 流程严格按以下顺序：

```
┌─────────────────────────────────────────────────────────────┐
│ 1. develop 端验收                                             │
│    □ 6 层测试金字塔全绿（unit/integration/smoke/corpus/       │
│       quality/e2e — 共 1235+ 测试）                            │
│    □ 20 轮全面健康检查 ≥ 17/20 PASS                           │
│    □ Playwright E2E 主流程绿（参照 docs/e2e-test-report.md）  │
│    □ rc.N 已跑过 ≥ 24h 无 regression                          │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│ 2. develop → main 合入                                        │
│    git checkout main                                          │
│    git pull origin main                                       │
│    git merge develop --no-ff -m "merge: develop → main for vX │
│    # 预期无冲突（main 永远是 develop 的祖先）                  │
│    git push origin main                                       │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│ 3. 在 main 上打两个 GA tag（双轨同时）                        │
│    git checkout main                                          │
│    git tag -a vX.Y.Z -m "vX.Y.Z: <核心能力> + <测试统计>"      │
│    git tag -a desktop-vX.Y.Z -m "desktop vX.Y.Z: <安装器变更>" │
│    git push origin vX.Y.Z desktop-vX.Y.Z                      │
│    # 上述两条 push 自动触发对应 workflow                      │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│ 4. 验证 CI 产物                                                │
│    □ rust-release.yml ✅ 5 平台二进制 tarball 上传 GitHub      │
│      Releases (vX.Y.Z 页面)                                   │
│    □ desktop-release.yml ✅ Win+Linux 安装器上传 GitHub        │
│      Releases (desktop-vX.Y.Z 页面)                           │
│    □ 校验 SHA256（rust-release.yml 自动生成 *.sha256）        │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│ 5. README + 官网更新                                           │
│    □ README.md / README.zh.md Download 表格更新到 vX.Y.Z      │
│    □ official-web (cloud/) v 号更新                            │
│    □ wiki-web 跟进 (release notes)                             │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│ 6. develop 起新版本周期                                        │
│    git checkout develop                                       │
│    # 接下来的 commits 自然进入下一个 vX.Y+1 周期              │
│    # （不需要手动 bump 版本号，rc.N tag 即标记节奏）          │
└─────────────────────────────────────────────────────────────┘
```

**关键不变量**：
- ❌ **永不**在 develop 上打 `vX.Y.Z` 无后缀 tag — 那是 main 专属
- ❌ **永不** force-push main 或 develop —（CLAUDE.md 已禁）
- ❌ **永不**直接 commit 到 main — 必须经 develop 合入
- ✅ **永远**两条 tag 同步 push（`vX.Y.Z` + `desktop-vX.Y.Z`），让 GA 用户能同时拿到 server 和 desktop 产物

### 远端清理

feature 分支 squash merge 后**立刻删远端**，避免分支墓地：

```bash
git push origin --delete feature/<name>
git fetch --prune
git branch -d feature/<name>     # 本地删
```

GitHub 网页端勾选"Delete branch"也可以。

## 编译命令汇总

### Rust 商用线

```bash
# 本地原生编译（Linux x86_64 / Windows x86_64 / macOS）
cd rust && cargo build --release
# 产物: rust/target/release/attune-server (~30 MB 静态二进制)

# 嵌入式 Web UI 一同编译（include_str! 自动打包）
cd rust && cargo build --release -p attune-server

# Linux → Windows 交叉编译（需要 cargo-xwin）
rustup target add x86_64-pc-windows-msvc
cargo install cargo-xwin
cd rust && cargo xwin build --release --target x86_64-pc-windows-msvc

# Linux → aarch64（K3 一体机 / 树莓派）
sudo apt install gcc-aarch64-linux-gnu g++-aarch64-linux-gnu
rustup target add aarch64-unknown-linux-gnu
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
  cargo build --release --target aarch64-unknown-linux-gnu

# Tauri 2 Desktop 打包（Win MSI / Linux deb / AppImage）
cd apps/desktop && cargo tauri build
# Linux 产物: src-tauri/target/release/bundle/{deb,appimage}/*
# Windows 产物: src-tauri/target/release/bundle/msi/*.msi（需在 Windows 上跑）
```

### Python 原型线

```bash
python -m venv .venv && source .venv/bin/activate
pip install -i https://pypi.tuna.tsinghua.edu.cn/simple -e ".[dev]"

# AppImage（Linux）
cd packaging && bash build-appimage.sh

# NSIS EXE（Windows，需要 Wine 或在 Windows 上跑）
cd packaging && makensis attune.nsi
```

### Web UI（Preact + Vite）

```bash
cd rust/crates/attune-server/ui
npm install --registry https://registry.npmmirror.com
npm run build         # 产物 → dist/，会被 attune-server cargo build 通过 include_str! 内嵌
```

### Chrome 扩展

```bash
cd extension
npm install --registry https://registry.npmmirror.com
npm run build         # 三阶段构建（pages / content IIFE / background ESM）
# 产物: extension/dist/，加载到 chrome://extensions 测试
```

### 验证测试

```bash
# Rust 全量
cd rust && cargo test --release --workspace -- --test-threads=2

# Python 全量
pytest tests/ -v

# Playwright E2E（扩展 + UI）
cd rust/crates/attune-server/ui && npm run test:e2e
```

## 环境搭建

```bash
git clone <repo-url> && cd attune

# Python 后端
python -m venv .venv && source .venv/bin/activate
pip install -i https://pypi.tuna.tsinghua.edu.cn/simple -e ".[dev]"

# Chrome 扩展
cd extension && npm install --registry https://registry.npmmirror.com && cd ..

# Embedding（开发用 Ollama 最简单）
curl -fsSL https://ollama.com/install.sh | sh && ollama pull bge-m3

# 验证
pytest tests/ -v    # 78 个测试全部通过
ruff check src/ tests/
```

## 项目结构

```
src/npu_webhook/
├── main.py                 # FastAPI 入口 + lifespan 初始化
├── config.py               # Pydantic Settings + YAML 配置
├── app_state.py            # 全局状态容器
├── api/                    # API 路由
│   ├── ingest.py           # POST /ingest（两层入队：章节+段落块）
│   ├── upload.py           # POST /upload multipart 文件直传
│   ├── search.py           # GET /search + POST /search/relevant（层级检索）
│   ├── items.py            # CRUD /items
│   ├── index.py            # /index 目录绑定
│   ├── status.py           # /status 系统状态
│   ├── settings.py         # /settings 配置
│   ├── model_routes.py     # /models 模型管理 + 部署检查
│   ├── skills.py           # /skills
│   ├── ws.py               # WebSocket
│   └── setup.py            # /setup 安装引导
├── core/                   # 核心引擎
│   ├── embedding.py        # OllamaEmbedding / ONNXEmbedding / OpenVINO
│   ├── vectorstore.py      # ChromaDB 向量检索
│   ├── search.py           # RRF 混合搜索 + 两阶段层级检索 + 动态预算
│   ├── fulltext.py         # jieba + FTS5
│   ├── chunker.py          # 滑动窗口分块 + extract_sections() 语义章节切割
│   └── parser.py           # 文件解析 + parse_bytes() 内存解析
├── db/                     # 数据库
│   ├── sqlite_db.py        # SQLite (schema/CRUD/FTS5/队列)
│   └── chroma_db.py        # ChromaDB 封装
├── indexer/                # 文件索引
│   ├── watcher.py          # watchdog 监听
│   └── pipeline.py         # 索引管道
├── scheduler/              # 调度
│   └── queue.py            # Embedding 队列 Worker
├── platform/               # 跨平台
│   ├── base.py             # NPUDevice + PlatformProvider ABC
│   ├── detector.py         # 硬件检测 + 芯片匹配 + 驱动检查
│   ├── linux.py / windows.py / paths.py
│   └── ...
├── tray.py                 # 系统托盘入口（pystray + uvicorn 后台线程）
└── models/schemas.py       # Pydantic 模型

extension/                  # Chrome 扩展 (Manifest V3 + Preact + Vite)
├── src/
│   ├── content/            # Content Script
│   │   ├── detector.js     # 平台适配器 (ChatGPT/Claude/Gemini)
│   │   ├── capture.js      # MutationObserver 对话捕获
│   │   ├── indicator.js    # 状态指示器
│   │   └── index.js        # 入口整合
│   │   # 注：injector.js 于 cleanup-r15 删除（2026-04-12 转 RAG 后弃用）
│   ├── background/worker.js    # 消息路由 + 去重 + 健康检查 + 会话感知加权
│   ├── sidepanel/
│   │   ├── pages/SearchPage.jsx
│   │   ├── pages/TimelinePage.jsx
│   │   ├── pages/FilePage.jsx   # 文件拖拽上传（uid 并发安全）
│   │   ├── pages/StatusPage.jsx
│   │   └── App.jsx             # 四标签路由
│   ├── popup/Popup.jsx     # 快速操作面板
│   ├── options/Options.jsx # 设置页面
│   └── shared/             # messages.js（FILE_UPLOADED）/ api.js（uploadFile）/ storage.js
├── vite.config.js          # 多阶段构建 (IIFE/ESM/HTML)
└── manifest.json
```

## 启动序列

lifespan 初始化顺序（`main.py`）：

1. 日志（RotatingFileHandler 50MB×3）
2. SQLite（schema + WAL）
3. ChromaDB（PersistentClient）
4. Embedding（Ollama 优先 → ONNX 回退 → None 降级）
5. VectorStore + HybridSearchEngine
6. Chunker + IndexPipeline
7. EmbeddingQueueWorker（后台线程）
8. DirectoryWatcher（加载绑定目录）

## Embedding 引擎架构

```
create_embedding_engine(device="auto")
  ├─ device in (auto, ollama) → OllamaEmbedding(HTTP API) → 成功返回
  │                           → 失败: auto 继续, ollama 返回 None
  ├─ device == openvino → 降级警告 → ONNX CPU
  └─ ONNX 模式 → 查找 model_dir → ONNXEmbedding(CPU/DirectML/ROCm)
                → 未找到 → None (搜索回退 FTS5)
```

## 芯片检测流程

`platform/detector.py` 中的 `full_platform_check()`:

1. **内核信息**：`platform.release()` 获取版本
2. **硬件扫描**：Intel NPU(`/dev/accel*`) → AMD NPU(`amdxdna`) → Intel iGPU(`lspci`) → AMD iGPU → Ollama → CPU
3. **芯片级匹配**：通过 PCI ID / lspci 关键词识别具体芯片代（Meteor Lake / Strix Point 等）
4. **驱动栈检查**：内核模块(`lsmod`) + 固件(`/lib/firmware/`) + 用户态运行时(`dpkg -l`)
5. **版本比对**：当前内核版本 vs 芯片最低内核要求
6. **命令生成**：缺失组件 → 精确安装命令

## 扩展构建

```bash
cd extension
npm run build    # 三阶段构建

# 阶段 1: Pages (Preact HTML)  → dist/{popup,sidepanel,options}/
# 阶段 2: Content (IIFE)       → dist/content/index.js
# 阶段 3: Background (ESM)     → dist/background/worker.js
# 最后:   cp indicator.css      → dist/content/indicator.css
```

Content Script 必须是 IIFE（Chrome 不支持 ES module content script），Background Worker 可以是 ES module。

## 测试

```bash
pytest tests/ -v                          # 全部 78 个
pytest tests/test_api.py -v               # API 端点 (13)
pytest tests/test_extension.py -v         # 扩展 E2E (42, 需要后端运行)
pytest tests/test_search.py -v            # 搜索引擎 + 层级检索 (12)
pytest tests/test_chunker.py -v           # 分块 + extract_sections (4)
pytest tests/test_parser.py -v            # parse_bytes (3)
pytest tests/test_upload.py -v            # 文件上传 API (4)
pytest tests/test_indexer.py -v           # 索引管道 (6)
pytest tests/test_platform.py -v          # 平台检测 (6)
pytest tests/test_embedding.py -v         # Embedding (1)
```

扩展 E2E 测试使用 Playwright Chromium（`--load-extension`），注意 Google Chrome 不支持此参数。

## 代码规范

- ruff：line-length=120, target=py311
- 公开函数必须有类型注解
- 测试放 `tests/`，调试代码放 `tmp/`（使用后删除）
- API 前缀 `/api/v1/`，端口 18900
- pip 使用清华源

```bash
ruff check src/ tests/
ruff format src/ tests/
```

## 数据库 Schema

| 表 | 用途 |
|---|------|
| `knowledge_items` | 知识条目 |
| `knowledge_fts` | FTS5 全文索引 |
| `embedding_queue` | Embedding 任务队列（P0-P3），含 `level`/`section_idx` |
| `bound_directories` | 绑定目录 |
| `indexed_files` | 文件索引记录（路径/hash） |

`embedding_queue` 关键字段：`level`（1=章节, 2=段落块）、`section_idx`（所属章节序号，Stage 2 检索用）。

ChromaDB collection: `knowledge_embeddings`（cosine 相似度）。metadata 包含 `level`、`section_idx`、`item_id`、`source_type`。

## 两层索引机制

入库时每个文档产生两层向量：

```
content → extract_sections()
    ├── Level 1 (章节, ~1500字): priority=max(1, p-1)，先处理
    └── Level 2 (段落块, 512字): priority=p，标准处理
```

检索时三阶段层级检索（`search_relevant()`）：

```
Stage 1: ChromaDB(level=1) → top-5 候选章节（召回）
Stage 2: ChromaDB(level=2, section_idx IN [...], item_id IN [...]) → top-K 段落（精排）
Stage 3: 取每个命中段落的父章节文本作为注入内容（语义完整）
```

动态预算分配（`_allocate_budget()`）：按 score 权重将 2000 字预算分配给各结果，最低 100 字/条。

## 认证

- localhost（`127.0.0.1` / `::1`）免认证
- 非本机需 `X-API-Token` 请求头
