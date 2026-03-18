# npu-webhook

个人知识库 + 记忆增强系统。通过 Chrome 扩展在 AI 对话和日常浏览中自动捕获、检索、注入知识，利用 NPU/iGPU 闲置算力处理 embedding。

## 技术栈

- 后端: FastAPI + Uvicorn, Python 3.11+
- 向量库: ChromaDB (嵌入式, cosine 相似度)
- 全文搜索: SQLite FTS5 + jieba 分词（LIKE 回退）
- Embedding: bge-small-zh-v1.5 (ONNX), 支持 OpenVINO (Intel NPU) / DirectML (AMD NPU)
- Chrome 扩展: Manifest V3 + Preact + Vite
- 打包: PyInstaller + AppImage (Linux) / NSIS (Windows)

## 已实现模块（Phase 0-1）

- `main.py` — lifespan 全链路初始化、路由注册、认证中间件、日志
- `config.py` — YAML 配置加载 + Pydantic Settings + 跨平台路径
- `app_state.py` — 全局状态容器
- `db/sqlite_db.py` — SQLite 管理（schema/CRUD/FTS5 同步/embedding 队列/文件索引）
- `db/chroma_db.py` — ChromaDB 客户端封装
- `core/embedding.py` — ONNX Embedding 引擎（tokenizers 加载/L2 归一化/工厂函数）
- `core/vectorstore.py` — 向量存储（批量写入/语义搜索）
- `core/search.py` — RRF 混合搜索引擎
- `core/fulltext.py` — jieba 分词辅助
- `core/chunker.py` — 滑动窗口分块（句子边界感知）
- `core/parser.py` — 文件解析器（MD/TXT/代码/PDF/DOCX）
- `indexer/watcher.py` — watchdog 多目录监听
- `indexer/pipeline.py` — 解析→分块→存储→embedding 管道（文件 hash 增量）
- `scheduler/queue.py` — Embedding 队列 Worker（后台线程/批量/即时处理）
- API: ingest/search/items/index/status/settings/ws/setup

## 开发规范

- Python 代码使用 ruff 格式化和 lint（line-length=120）
- 类型注解: 所有公开函数必须有类型注解
- 测试放 `tests/` 目录, 使用 pytest（当前 20 个测试）
- 调试代码放 `tmp/`, 使用后删除
- API 路径前缀: `/api/v1/`
- 后端端口: 18900
- 使用 venv 管理 Python 依赖
- pip 使用清华源

## 项目结构

- `src/npu_webhook/` — Python 后端
- `extension/` — Chrome 扩展（Manifest V3 + Preact + Vite）
- `packaging/` — 打包配置（PyInstaller/AppImage/NSIS）
- `.github/workflows/` — CI/CD（ci/build-linux/build-windows/release）
- `tests/` — 测试代码 + conftest.py（临时 DB fixture）
