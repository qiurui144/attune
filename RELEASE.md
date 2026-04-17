# 版本计划

> **双产品线**：本文档记录 **Python 原型线** 的迭代历史。
> **Rust 商用线** 在 [`npu-vault/RELEASE.md`](npu-vault/RELEASE.md) 独立跟踪。

## 产品线分离（2026-04）

自 2026-04-09 起，项目明确分为两条产品线：

**Python 原型线**（`src/npu_webhook/`）：
- 快速验证新算法和功能设计
- ChromaDB + FTS5 混合搜索
- 用途：特性探索、性能对比、教学演示
- 持续迭代中，不追求产品级

**Rust 商用线**（`npu-vault/`）：
- 端到端加密（Argon2id + AES-256-GCM + Device Secret）
- tantivy + usearch 纯 Rust 搜索栈
- Axum HTTP Server + TLS + Bearer auth + 嵌入式 Web UI
- 跨平台单二进制（Linux / Windows / NAS / Android）
- 用途：生产部署、商用发布、NAS 远程访问
- 最新版本 v0.3.0，78 tests

两条线共享 Chrome 扩展协议（`/api/v1/*`），Python 原型验证的特性择优迁移到 Rust 商用线。

---

## 已发布

### v0.3.0 — Phase 3：长文本质量提升 + 文件直传 + 系统托盘

**长文本语义索引：**
- `extract_sections()` 语义章节切割（Markdown 标题边界 / 代码 def|class / 纯文本 1500 字段落）
- 两层 embedding 队列：Level 1 章节（priority 高 1 级）+ Level 2 段落块（现有 512 字滑动窗口）
- ChromaDB metadata 新增 `level` / `section_idx` 字段（向后兼容）
- 两阶段层级检索（`search_relevant()`）：章节召回 → 段落精排 → 父章节上下文扩展
- Stage 2 加入 `item_id` 约束，防止跨文档 section_idx 污染
- 动态注入预算（`_allocate_budget()`）：2000 字 score 加权分配，替代固定 300 字截断

**文件直传 API：**
- `POST /api/v1/upload` multipart 端点，支持 PDF / DOCX / MD / TXT / 代码
- `parse_bytes(data, filename)` 内存解析，无落盘 I/O
- 大小限制（默认 20MB）：`file.size` 预检 + 读后二次验证（双重防护）
- 上传后同步 FTS5 可搜、向量 embedding 后台异步处理

**Chrome 扩展 — 文件标签：**
- Side Panel 新增「文件」标签页（FilePage.jsx），拖拽 / 点击上传
- `crypto.randomUUID()` uid 跟踪并发上传状态，避免同名文件冲突
- 会话感知加权：Worker 记录本次会话上传的 item_id，SEARCH_RELEVANT 结果 score × 1.5
- `api.uploadFile()` FormData + session_id 透传

**系统托盘：**
- `tray.py`：pystray 系统托盘 + uvicorn daemon 线程
- 64×64 Pillow 绘制圆形图标，菜单含退出选项
- `pyproject.toml` 新增可选依赖组 `[tray]`

**测试：** 78 个（36 后端单元 + 42 扩展 E2E）

---

### v0.2.0 — Phase 0-2：后端核心 + Chrome 扩展 + Embedding

**后端（Phase 0-1）：**
- FastAPI + lifespan 生命周期 + 认证中间件 + CORS
- SQLite（WAL）+ FTS5 全文索引 + Embedding 优先级队列
- ChromaDB 向量存储（cosine）
- 多后端 Embedding：OllamaEmbedding（HTTP API）/ ONNXEmbedding（CPU/DirectML/ROCm）
- RRF 混合搜索引擎（向量 + 全文，可配置权重）
- 文档解析：MD / TXT / 代码 / PDF / DOCX
- 滑动窗口分块（句子边界感知）
- watchdog 多目录监听 + 增量索引管道（SHA-256 hash 去重）
- API：ingest / search / items / index / status / settings / models / ws

**Chrome 扩展（Phase 2）：**
- Manifest V3 + Preact + Vite 多阶段构建（IIFE/ESM/HTML）
- 平台适配器：ChatGPT / Claude / Gemini DOM 选择器 + 消息提取
- Content Script：MutationObserver 对话捕获 + 2s debounce + 流式完成检测
- 无感前缀注入：capture phase 拦截 + 知识分类前缀 + 平台输入框写入
- Background Worker：消息路由 + djb2 去重（session storage 持久化）+ 30s 健康检查
- Side Panel：搜索（source_type 过滤）/ 时间线（日期分组 + 分页 + 删除）/ 状态（8 项指标）
- Popup：连接状态 / 统计 / 注入开关 / 快速操作
- Options：后端地址 / 注入模式 / 排除域名 / 测试连接

**平台检测：**
- 芯片级精确匹配：Intel Meteor/Lunar/Arrow Lake、AMD Phoenix/Hawk/Strix/Krackan Point
- 内核版本比对 + 固件检查 + 内核模块检查 + 用户态运行时检查
- `/models/check` 部署检查 API + 一键安装命令生成
- `/models` 模型列表 + Ollama/ONNX 状态

**测试：** 62 个（20 后端单元 + 42 扩展 E2E Playwright Chromium）

## 路线图

### v0.4.0 — xPU 原生加速

- Intel NPU：OpenVINO 集成 + ONNX→IR 转换 + INT8 量化
- AMD NPU：DirectML EP 集成
- 硬件自动检测 → 最优设备自动切换
- 系统空闲检测 + 动态 batch size

### v0.5.0 — 分发与安装

- PyInstaller + AppImage（Linux）
- PyInstaller + NSIS EXE（Windows）
- /setup 首次安装引导页
- 模型内嵌 + WebSocket 下载进度
- 开机自启（systemd user service / Windows Service）

### v1.0.0 — 正式发布

- GitHub Actions CI/CD 完整流水线
- 多模态：图片 OCR + 图表理解
- 知识图谱：实体抽取 + 关联推理
- 多轮对话上下文持续注入
- Firefox / Edge 扩展适配
- 端到端加密存储
