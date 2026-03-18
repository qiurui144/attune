# npu-webhook

本地优先的个人知识库 + 记忆增强系统。

通过 Chrome 扩展在 AI 对话（ChatGPT / Claude / Gemini）和日常浏览中自动捕获知识，利用 NPU / iGPU / CPU 闲置算力处理 embedding，实现可检索的知识积累和无感前缀注入。

## 功能

- **记忆增强** — 自动捕获 AI 回答、搜索记录和网页内容，形成可检索的知识积累
- **记忆注入** — 对话时无感地将相关知识按类型（笔记 / 历史对话 / 技能）以前缀注入，提升 AI 回答精准度
- **混合搜索** — 向量语义搜索 + FTS5 全文搜索，RRF 融合排序
- **本地目录索引** — 绑定文件夹，watchdog 实时监听变更，自动解析 Markdown / 纯文本 / 代码 / PDF / DOCX
- **闲时计算** — 利用 Intel NPU / AMD NPU / iGPU / CPU 闲置算力处理 embedding 队列
- **跨平台** — 支持 Linux + Windows，AppImage / EXE 一键安装

## 架构

```
┌──────────────────────────────────────────────────────────────┐
│              Chrome Extension (Manifest V3)                   │
│  Content Script ──→ Background SW ──→ Side Panel / Popup     │
│  (捕获+注入)        (消息路由)        (知识管理UI+设置)       │
└───────────┬──────────────────┬───────────────────────────────┘
            │ REST API         │ WebSocket
            ▼                  ▼
┌──────────────────────────────────────────────────────────────┐
│              FastAPI Backend (localhost:18900)                 │
│                                                               │
│  API:      ingest │ search │ items │ index │ skills │ ws     │
│                                                               │
│  Core:     EmbeddingEngine(ONNX/OpenVINO) │ RRF HybridSearch │
│            Chunker │ Parser │ VectorStore │ FullText(FTS5)    │
│                                                               │
│  Storage:  ChromaDB (向量) │ SQLite + FTS5 (全文 + 元数据)    │
│                                                               │
│  Indexer:  watchdog 目录监听 │ 解析→分块→embedding 管道       │
│  Sched:    Embedding 优先级队列 │ xPU 空闲调度                │
└──────────────────────────────────────────────────────────────┘
```

## 快速开始

### 环境要求

- Python 3.11+
- Node.js 18+（Chrome 扩展开发）
- Chrome 浏览器

### 后端

```bash
# 创建虚拟环境
python -m venv .venv
source .venv/bin/activate          # Linux
# .venv\Scripts\activate           # Windows

# 安装依赖
pip install -i https://pypi.tuna.tsinghua.edu.cn/simple -e ".[dev]"

# 启动开发服务器
uvicorn npu_webhook.main:app --reload --port 18900

# 访问
# API 文档: http://localhost:18900/docs
# 健康检查: http://localhost:18900/api/v1/status/health
# 系统状态: http://localhost:18900/api/v1/status
```

### Embedding 模型（可选）

后端启动时自动检测模型。无模型时搜索回退到 FTS5 全文搜索。

```bash
# 下载 bge-small-zh-v1.5 ONNX 模型到数据目录
# Linux: ~/.local/share/npu-webhook/models/bge-small-zh-v1.5/
# Windows: %LOCALAPPDATA%\npu-webhook\models\bge-small-zh-v1.5\
# 需要文件: model.onnx + tokenizer.json
```

### Chrome 扩展（Phase 2）

```bash
cd extension
npm install
npm run dev
# Chrome → 扩展管理 → 开发者模式 → 加载已解压的扩展 → 选择 extension/dist
```

### 测试

```bash
# 单元测试（20 个）
pytest tests/ -v

# 代码规范
ruff check src/ tests/
ruff format --check src/ tests/
```

## 配置

配置文件位置：
- Linux: `~/.config/npu-webhook/config.yaml`
- Windows: `%APPDATA%\npu-webhook\config.yaml`

```yaml
server:
  host: "127.0.0.1"
  port: 18900

embedding:
  model: "bge-small-zh-v1.5"
  device: "auto"          # auto/cpu/npu/gpu
  batch_size: 16

search:
  default_top_k: 10
  rrf_k: 60
  vector_weight: 0.6
  fulltext_weight: 0.4

ingest:
  min_content_length: 100
  excluded_domains:
    - "mail.google.com"
    - "web.whatsapp.com"

logging:
  level: "INFO"
  max_size_mb: 50
```

不存在配置文件时使用默认值，所有配置项均可选。

## 数据存储

| 数据 | 位置（Linux） | 位置（Windows） |
|------|---------------|-----------------|
| SQLite 数据库 | `~/.local/share/npu-webhook/knowledge.db` | `%LOCALAPPDATA%\npu-webhook\knowledge.db` |
| ChromaDB 向量 | `~/.local/share/npu-webhook/chroma/` | `%LOCALAPPDATA%\npu-webhook\chroma\` |
| 日志 | `~/.local/share/npu-webhook/logs/` | `%LOCALAPPDATA%\npu-webhook\logs\` |
| ONNX 模型 | `~/.local/share/npu-webhook/models/` | `%LOCALAPPDATA%\npu-webhook\models\` |
| 配置 | `~/.config/npu-webhook/config.yaml` | `%APPDATA%\npu-webhook\config.yaml` |

## 分发

| 平台 | 格式 | 获取方式 |
|------|------|----------|
| Linux | AppImage | GitHub Releases |
| Windows | EXE 安装包 (NSIS) | GitHub Releases |
| Mac | DMG（规划中） | — |

## License

MIT
