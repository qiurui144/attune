# npu-webhook

个人知识库 + 记忆增强系统。通过 Chrome 扩展在 AI 对话和日常浏览中自动捕获、检索、注入知识，利用 Ollama / NPU / iGPU 闲置算力处理 embedding。

## 技术栈

- 后端: FastAPI + Uvicorn, Python 3.11+
- 向量库: ChromaDB (嵌入式, cosine 相似度)
- 全文搜索: SQLite FTS5 + jieba 分词（LIKE 回退）
- Embedding: Ollama bge-m3 (默认) / ONNX Runtime (CPU/DirectML/ROCm) / OpenVINO (Intel NPU/iGPU)
- Chrome 扩展: Manifest V3 + Preact + Vite 多阶段构建
- 打包: PyInstaller + AppImage (Linux) / NSIS (Windows)

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
- `content/injector.js` — 无感前缀注入 (capture phase 拦截，动态预算)
- `content/indicator.js` — 4 状态指示器 (disabled/processing/captured/offline)
- `background/worker.js` — 消息路由 + 去重缓存 (session storage) + 30s 健康检查 + 会话感知加权
- `popup/Popup.jsx` — 连接状态 / 统计 / 注入开关
- `options/Options.jsx` — 后端地址 / 注入模式 / 排除域名 / 测试连接
- `sidepanel/` — 搜索 / 时间线 / 文件 (拖拽上传, uid 并发安全) / 状态
- `shared/messages.js` — 统一消息类型（含 FILE_UPLOADED）+ 通信辅助
- `shared/api.js` — 后端 API 封装 (动态 baseUrl, 含 uploadFile)

## 开发规范

- Python 代码使用 ruff 格式化和 lint（line-length=120）
- 类型注解: 所有公开函数必须有类型注解
- 测试放 `tests/` 目录, 使用 pytest（当前 78 个测试: 36 后端单元 + 42 扩展 E2E）
- 扩展 E2E 测试使用 Playwright Chromium（非 Google Chrome）
- 调试代码放 `tmp/`, 使用后删除
- API 路径前缀: `/api/v1/`
- 后端端口: 18900
- 使用 venv 管理 Python 依赖
- pip 使用清华源

## 项目结构

- `src/npu_webhook/` — Python 后端
- `extension/` — Chrome 扩展（Manifest V3 + Preact + Vite）
- `packaging/` — 打包配置（PyInstaller/AppImage/NSIS）
- `.github/workflows/` — CI/CD
- `tests/` — 测试代码 + conftest.py

## 芯片-驱动匹配

detector.py 中维护了精确匹配表:
- Intel: INTEL_NPU_CHIPS (meteor_lake/lunar_lake/arrow_lake) + INTEL_IGPU_CHIPS (alder~arrow)
- AMD: AMD_NPU_CHIPS (phoenix/hawk_point/strix_point/krackan_point)
- 每个芯片条目包含: PCI ID、最低内核版本、固件路径、最低驱动版本、已知问题
- /models/check API 输出完整检测报告 + 一键安装命令
