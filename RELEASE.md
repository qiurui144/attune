# 版本计划

## 当前版本

### v0.2.0-dev — 后端核心（已完成）

- FastAPI 骨架 + lifespan 生命周期管理 + 认证中间件
- SQLite 数据库：knowledge_items CRUD + FTS5 全文索引 + embedding 队列 + 文件索引
- ChromaDB 向量存储（cosine 相似度）
- ONNX Runtime Embedding 引擎抽象（CPU / DirectML / ROCm），模型不存在时优雅降级
- 文档解析：Markdown / 纯文本 / 代码 / PDF / DOCX
- 滑动窗口分块（句子边界感知）
- RRF 混合搜索引擎（向量 + 全文，可配置权重）
- watchdog 多目录文件监听 + 增量索引管道（文件 hash 去重）
- Embedding 优先级队列 Worker（后台线程批量消费）
- 完整 API：ingest / search / items / index / status / settings
- Swagger UI 自动生成 API 文档
- YAML 配置文件 + 跨平台路径管理
- 日志：RotatingFileHandler 轮转（50MB × 3）
- 20 个单元测试 + 16 项 Playwright Chrome E2E 测试全部通过

## 路线图

### v0.3.0 — Chrome 扩展 + 记忆系统

- Manifest V3 + Vite + Preact 完整构建
- AI 平台适配器：ChatGPT + Claude + Gemini
- MutationObserver 对话自动捕获
- 无感前缀注入（按知识类型分类）
- Side Panel 完整 UI（搜索 / 时间线 / 技能 / 状态）
- Popup 快速操作面板
- Settings 页面
- WebSocket 实时通道（进度 / 通知）

### v0.4.0 — 技能系统

- Skill CRUD API + Jinja2 模板渲染
- URL glob 匹配自动触发
- 技能与知识库联动
- Side Panel 技能管理界面

### v0.5.0 — xPU 加速

- Intel NPU：OpenVINO 集成 + ONNX→IR 模型转换 + INT8 量化
- AMD NPU：DirectML EP
- 硬件自动检测 + 最优设备选择
- 系统空闲检测（CPU / GPU / 用户输入）
- APScheduler 调度 + 动态 batch size

### v1.0.0 — 正式发布

- GitHub Actions CI/CD 完整流水线
- PyInstaller + AppImage（Linux）
- PyInstaller + NSIS EXE（Windows）
- 系统托盘图标（pystray）
- /setup 首次安装引导页（含扩展下载 + 安装教程）
- 模型内嵌 + 可选下载（WebSocket 进度）
- 开机自启注册（systemd user service / Windows Service）

## 优化空间

### 短期

| 方向 | 措施 | 预期收益 |
|------|------|----------|
| 搜索质量 | 引入 bge-reranker-v2-m3 二次排序 | 精度 +15-25% |
| 分块策略 | 语义分块（句子边界 + 主题分割） | 检索相关性提升 |
| Embedding 缓存 | 相同/相似 query LRU 缓存 | 重复查询 <5ms |
| 增量索引 | content hash 部分变更检测 | 索引速度 10x |

### 中期

- 多模态：图片 OCR + 图表理解
- 知识图谱：实体抽取 + 关联推理
- 多轮对话上下文持续注入
- 多设备知识库同步（CRDTs）
- Firefox / Edge 扩展适配

### 长期

- 本地 LLM 知识摘要（NPU 运行 Qwen2-1.5B / Phi-3-mini）
- 基于用户反馈的主动学习
- 端到端加密存储
- Mac 支持（Apple Neural Engine via coremltools）
- 插件系统 + 开放 API

## 性能预测

| 规模 | 瓶颈 | 应对 |
|------|------|------|
| 10K 条目 | 无 | 当前架构满足 |
| 100K 条目 | ChromaDB 查询延迟 | HNSW 参数调优 / 切换 Qdrant |
| 1M 条目 | SQLite FTS5 + ChromaDB | 分库分表 / 冷热分离 |
