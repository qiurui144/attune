# 开发指南

## 环境要求

- Python 3.11+
- Node.js 18+ (Chrome 扩展开发)
- Chrome 浏览器

## 项目结构

```
src/npu_webhook/          # Python 后端
├── api/                  # API 路由 (ingest/search/items/skills/settings/status/ws)
├── core/                 # 核心业务 (embedding/vectorstore/fulltext/search/chunker/parser)
├── indexer/              # 文件索引 (watcher/pipeline)
├── platform/             # 跨平台抽象 (linux/windows/paths/detector)
├── scheduler/            # xPU 调度 (idle/queue)
├── db/                   # 数据库 (sqlite/chroma)
└── models/               # Pydantic 数据模型

extension/                # Chrome 扩展 (Manifest V3 + Preact + Vite)
├── src/background/       # Service Worker
├── src/content/          # Content Script (捕获+注入)
├── src/sidepanel/        # Side Panel (主 UI)
├── src/popup/            # Popup 快速操作
└── src/options/          # 设置页面

packaging/                # 打包配置
tests/                    # 测试代码
```

## API 概览

所有 API 前缀 `/api/v1/`，后端运行在 `localhost:18900`。

| 端点 | 说明 |
|------|------|
| POST /ingest | 知识注入 |
| GET /search | 混合搜索 (向量+全文, RRF) |
| POST /search/relevant | 获取注入用相关知识 |
| CRUD /items | 知识条目管理 |
| CRUD /skills | 技能管理 |
| POST /index/bind | 绑定本地目录 |
| GET /status | 系统状态 |
| WS /ws | WebSocket 实时通道 |

## 测试

```bash
pytest tests/
```

## 代码规范

- 格式化 + lint: `ruff check . && ruff format .`
- 类型检查: `mypy src/`
