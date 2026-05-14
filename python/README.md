# attune-python — Python 原型线

实验/验证用 FastAPI 后端。生产环境使用 [`../rust/`](../rust/README.md) Rust 商用线。

## 定位

- **算法实验**：embedding model 切换、chunk 策略迭代、检索算法 A/B
- **快速原型**：新 feature 在 Python 端先 prototype 再迁 Rust
- **不为分发优化**：未做加密、未做静态打包、不在 release 矩阵内

验证有效的特性会 promote 到 Rust 商用线。

## 技术栈

- **后端**: FastAPI + Uvicorn (Python 3.11+)
- **向量库**: ChromaDB (嵌入式, cosine 相似度)
- **全文搜索**: SQLite FTS5 + jieba 分词
- **Embedding**: Ollama bge-m3 / ONNX Runtime (CPU/DirectML/ROCm) / OpenVINO (Intel NPU/iGPU)
- **打包**: PyInstaller + AppImage (Linux) / NSIS (Windows) — 原型线少用

## 快速开始

```bash
cd python
python -m venv .venv
source .venv/bin/activate  # Windows: .venv\Scripts\activate
pip install -e ".[dev]"

# 启动后端 (默认 :18900)
python -m attune_python.main
```

测试：

```bash
pytest tests/ -v --ignore=tests/test_extension.py   # 默认套件
pytest tests/test_extension.py                       # 扩展 E2E (需系统 Chrome)
```

详细测试矩阵 + 跨层 fixture 见 [`../docs/TESTING.md`](../docs/TESTING.md)。

## 目录结构

- `src/attune_python/` — 主代码
  - `main.py` — FastAPI 入口 + lifespan
  - `config.py` — Pydantic Settings + YAML
  - `core/` — embedding / search / chunker / parser
  - `db/` — SQLite / ChromaDB 封装
  - `api/` — REST 路由 (`/api/v1/*`)
  - `scheduler/` — embedding queue worker
  - `indexer/` — 文件监听 + pipeline
  - `models/` — Pydantic schemas
- `tests/` — pytest 单测 + Playwright E2E + manual checklist
- `tests-e2e/` — vault lifecycle / health / evidence chain 端到端

## API

后端端口 `:18900`，前缀 `/api/v1/`：

```
GET  /api/v1/status         系统状态
POST /api/v1/ingest         上传/索引
POST /api/v1/search         混合搜索 (RRF + 两阶段层级)
GET  /api/v1/items          条目列表
GET  /api/v1/models/check   芯片-驱动匹配检测
WS   /ws/                   实时进度
```

## 开发规范

- ruff 格式化 + lint (line-length=120)
- 所有 public 函数必须有类型注解
- 测试代码放 `tests/`，调试 demo 放 `tmp/` 用完即删
- pip 使用清华源，venv 隔离依赖

## 与 Rust 商用线的关系

| 维度 | Python (本目录) | Rust (../rust/) |
|------|----------------|-----------------|
| 角色 | 实验/原型 | 生产/发布 |
| 加密 | 无 | Argon2id + AES-256-GCM |
| 分发 | 开发者自起 | Windows MSI / Linux deb / Tauri 桌面 |
| 测试数 | 13 (后端 API) | 200+ (含 integration) |
| 性能优化 | 不针对 | rusqlite + tantivy + usearch |

总体规则：**新 feature 先 Python 验证 → 择优 port 到 Rust**。

## 文档索引

- [../README.md](../README.md) — 仓库总览 + 下载
- [../CLAUDE.md](../CLAUDE.md) — 行为标准 + 项目约束
- [../docs/TESTING.md](../docs/TESTING.md) — 测试矩阵 + 跨层 fixture
- [tests/MANUAL_TEST_CHECKLIST.md](tests/MANUAL_TEST_CHECKLIST.md) — 人工验收清单
