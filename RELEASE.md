# 版本计划

## 当前版本

v0.1.0 - 项目初始化

## 路线图

### v0.2.0 - 后端核心
- FastAPI 骨架 + 中间件
- SQLite + FTS5 + ChromaDB
- ONNX Runtime CPU embedding (bge-small-zh-v1.5)
- 文档解析 + 目录索引
- RRF 混合搜索
- 核心 API

### v0.3.0 - Chrome 扩展
- Manifest V3 + Preact + Vite
- AI 平台适配 (ChatGPT/Claude/Gemini)
- 对话捕获 + 知识注入
- Side Panel / Popup / Settings UI

### v0.4.0 - 技能系统
- Skill CRUD + Jinja2 模板
- 技能与知识库联动

### v0.5.0 - xPU 加速
- Intel NPU (OpenVINO) + AMD NPU (DirectML)
- 空闲检测 + 优先级队列

### v1.0.0 - 正式发布
- AppImage (Linux) + NSIS EXE (Windows)
- 首次安装引导
- 系统托盘 + 开机自启

## 优化空间

- Reranker 二次排序
- 语义分块
- Embedding 缓存
- 多模态 (OCR)
- 知识图谱
- 本地 LLM 摘要
- Mac 支持 (Apple Neural Engine)
