# 开发指南

## 环境要求

- Python 3.11+
- Node.js 18+（Chrome 扩展开发）
- Chrome 浏览器
- Git

## 开发环境搭建

```bash
git clone <repo-url>
cd npu-webhook

# Python 后端
python -m venv .venv
source .venv/bin/activate
pip install -i https://pypi.tuna.tsinghua.edu.cn/simple -e ".[dev]"

# Chrome 扩展
cd extension && npm install && cd ..

# 验证
pytest tests/ -v        # 20 个测试应全部通过
ruff check src/ tests/  # lint
```

## 项目结构

```
npu-webhook/
├── src/npu_webhook/            # Python 后端
│   ├── main.py                 # FastAPI 入口 + lifespan + 路由注册
│   ├── config.py               # Pydantic Settings, YAML 配置加载
│   ├── app_state.py            # 全局状态容器（DB/引擎/队列实例）
│   ├── api/                    # API 路由
│   │   ├── ingest.py           # POST /ingest - 知识注入
│   │   ├── search.py           # GET /search + POST /search/relevant
│   │   ├── items.py            # CRUD /items - 知识条目
│   │   ├── index.py            # /index - 本地目录绑定
│   │   ├── status.py           # /status - 系统状态
│   │   ├── settings.py         # /settings - 配置管理
│   │   ├── skills.py           # /skills - 技能管理（Phase 3）
│   │   ├── model_routes.py     # /models - 模型管理（Phase 4）
│   │   ├── ws.py               # WebSocket 实时通道
│   │   └── setup.py            # /setup - 首次安装引导
│   ├── core/                   # 核心引擎
│   │   ├── embedding.py        # ONNX/OpenVINO Embedding 引擎
│   │   ├── vectorstore.py      # ChromaDB 向量检索封装
│   │   ├── fulltext.py         # jieba 分词 + FTS5 查询构建
│   │   ├── search.py           # RRF 混合搜索引擎
│   │   ├── chunker.py          # 滑动窗口文档分块
│   │   ├── parser.py           # 文件解析（MD/TXT/代码/PDF/DOCX）
│   │   └── skill_engine.py     # 技能模板渲染（Phase 3）
│   ├── indexer/                # 文件索引
│   │   ├── watcher.py          # watchdog 多目录监听
│   │   └── pipeline.py         # 解析→分块→存储→embedding 管道
│   ├── scheduler/              # 调度
│   │   ├── idle.py             # 系统空闲检测（Phase 4）
│   │   └── queue.py            # Embedding 优先级队列 Worker
│   ├── platform/               # 跨平台抽象
│   │   ├── base.py             # PlatformProvider ABC
│   │   ├── linux.py            # Linux 实现
│   │   ├── windows.py          # Windows 实现
│   │   ├── paths.py            # 平台检测 + Provider 工厂
│   │   └── detector.py         # NPU/GPU 硬件检测
│   ├── db/                     # 数据库
│   │   ├── sqlite_db.py        # SQLite 管理（schema/CRUD/FTS5/队列）
│   │   └── chroma_db.py        # ChromaDB 客户端封装
│   └── models/
│       └── schemas.py          # Pydantic 请求/响应模型
│
├── extension/                  # Chrome 扩展（Manifest V3 + Preact + Vite）
│   ├── manifest.json
│   ├── vite.config.js
│   ├── package.json
│   └── src/
│       ├── background/         # Service Worker
│       ├── content/            # Content Script（检测/捕获/注入/指示器）
│       ├── sidepanel/          # Side Panel（搜索/时间线/技能/状态）
│       ├── popup/              # Popup 快速操作
│       ├── options/            # 设置页面
│       └── shared/             # API 封装 + storage 工具
│
├── packaging/                  # 打包分发
│   ├── npu_webhook.spec        # PyInstaller 配置
│   ├── linux/                  # AppRun + desktop + systemd service
│   └── windows/                # NSIS installer + post-install
│
├── .github/workflows/          # CI/CD
│   ├── ci.yml                  # push/PR: lint + test + 扩展构建
│   ├── build-linux.yml         # AppImage 构建
│   ├── build-windows.yml       # NSIS EXE 构建
│   └── release.yml             # tag 触发发布
│
├── tests/                      # 测试
│   ├── conftest.py             # 测试 fixture（临时 DB 初始化）
│   ├── test_api.py             # API 端点测试（8 个）
│   ├── test_embedding.py       # Embedding 引擎测试
│   ├── test_indexer.py         # 分块/解析/索引管道测试（6 个）
│   ├── test_search.py          # 混合搜索 + FTS5 测试
│   └── test_platform.py        # 跨平台抽象测试（3 个）
│
├── pyproject.toml              # 项目配置 + 依赖
├── README.md                   # 项目说明
├── DEVELOP.md                  # 开发指南（本文件）
├── RELEASE.md                  # 版本计划
└── CLAUDE.md                   # Claude Code 项目指令
```

## API 参考

所有 API 前缀 `/api/v1/`，后端运行在 `localhost:18900`。完整交互文档访问 `/docs`（Swagger UI）。

### 知识注入

```
POST /api/v1/ingest
Body: {title, content, source_type, url?, domain?, tags?, metadata?}
Response: {id, status}
```

`source_type` 取值：`webpage` / `ai_chat` / `selection` / `file` / `note`

内容长度低于 `min_content_length`（默认 100 字符）或域名在排除列表中时返回 400。

### 搜索

```
GET  /api/v1/search?q=关键词&top_k=10&source_types=ai_chat,note
POST /api/v1/search/relevant  Body: {query, top_k, source_types?}
Response: {results: [{id, title, content, score, source_type, url, created_at}], total}
```

搜索流程：向量语义搜索（ChromaDB）+ 全文搜索（FTS5，LIKE 回退）→ RRF 融合排序。

### 知识条目

```
GET    /api/v1/items?offset=0&limit=20&source_type=note
GET    /api/v1/items/{id}
PATCH  /api/v1/items/{id}   Body: {title?, tags?, metadata?}
DELETE /api/v1/items/{id}
```

### 本地目录索引

```
POST   /api/v1/index/bind     Body: {path, recursive?, file_types?}
DELETE /api/v1/index/unbind?dir_id=xxx
GET    /api/v1/index/status
POST   /api/v1/index/reindex
```

绑定后自动启动 watchdog 监听 + 后台全量扫描。文件变更时自动增量索引（通过文件 SHA-256 hash 去重）。

### 系统

```
GET   /api/v1/status          # 系统状态（版本/设备/模型/统计）
GET   /api/v1/status/health   # 健康检查
GET   /api/v1/settings        # 获取配置
PATCH /api/v1/settings        # 更新配置
WS    /api/v1/ws              # WebSocket 实时通道
GET   /setup                  # 首次安装引导页
```

## 数据库 Schema

### SQLite 主要表

| 表 | 用途 |
|---|------|
| `knowledge_items` | 知识条目（标题/内容/来源/标签/元数据）|
| `knowledge_fts` | FTS5 全文索引（独立表，item_id 关联）|
| `embedding_queue` | Embedding 任务队列（优先级 P0-P3）|
| `bound_directories` | 绑定的本地目录 |
| `indexed_files` | 文件索引记录（路径/hash/关联 item）|
| `skills` | 技能模板 |
| `app_config` | KV 配置存储 |
| `optimization_history` | 优化历史记录 |

### ChromaDB

单一 collection `knowledge_embeddings`，metadata 包含 `item_id`、`chunk_index`、`source_type`、`created_at`。

## 核心流程

### 知识注入流程

```
POST /ingest → 校验(长度/域名) → SQLite 插入 + FTS5 同步
→ Chunker 分块 → embedding_queue 投递(P1)
→ 后台 Worker 取出 → ONNX embed → ChromaDB 写入
```

### 混合搜索流程

```
GET /search → 并行: ChromaDB 向量搜索 + FTS5/LIKE 全文搜索
→ RRF 融合排序: score = w_vec/(k+rank_vec) + w_fts/(k+rank_fts)
→ 补全 item 信息 → 返回结果
```

### 目录索引流程

```
POST /index/bind → watchdog 监听 + 后台全量扫描
→ 文件变更 → parser 解析 → chunker 分块
→ SQLite 存储 + embedding_queue 投递(P2)
→ 文件 hash 增量去重
```

## 启动序列

lifespan 初始化顺序：
1. 日志配置（RotatingFileHandler + 控制台）
2. SQLite 初始化（schema 创建 + WAL 模式）
3. ChromaDB 初始化（PersistentClient）
4. Embedding 引擎创建（模型不存在时 None，搜索回退 FTS5）
5. VectorStore + HybridSearchEngine 组装
6. Chunker + IndexPipeline 创建
7. EmbeddingQueueWorker 启动（后台线程）
8. DirectoryWatcher 加载绑定目录并启动

## 认证

- localhost（`127.0.0.1` / `::1`）：免认证
- 非本机：需要 `X-API-Token` 请求头（`auth.mode: token` 时）

## 测试

```bash
pytest tests/ -v               # 全部 20 个测试
pytest tests/test_api.py -v    # API 端点测试
pytest tests/test_search.py -v # 搜索测试
pytest tests/test_indexer.py   # 索引管道测试
```

测试使用临时目录，不影响本地数据。`conftest.py` 自动为 API 测试初始化临时 state。

## 代码规范

```bash
ruff check src/ tests/          # lint
ruff format src/ tests/         # 格式化
mypy src/                       # 类型检查
```

- Python 代码使用 ruff 格式化（line-length=120）
- 公开函数必须有类型注解
- 测试放 `tests/`，调试代码放 `tmp/`（使用后删除）
