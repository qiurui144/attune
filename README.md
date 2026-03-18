# npu-webhook

本地优先的个人知识库 + 记忆增强系统。

## 功能

- **记忆增强**：自动捕获 AI 对话（ChatGPT/Claude/Gemini）、搜索记录和网页内容
- **记忆注入**：对话时无感将相关知识按类型以前缀注入，提升 AI 回答精准度
- **闲时计算**：利用 NPU/iGPU 闲置算力处理 embedding
- **跨平台**：支持 Windows + Linux，一键安装

## 架构

```
Chrome Extension ──REST/WS──→ FastAPI Backend (localhost:18900)
                                ├── ChromaDB (向量检索)
                                ├── SQLite FTS5 (全文搜索)
                                ├── Embedding Engine (NPU/iGPU/CPU)
                                └── File Watcher (本地目录索引)
```

## 快速开始

### 开发环境

```bash
python -m venv .venv
source .venv/bin/activate  # Linux
# .venv\Scripts\activate   # Windows
pip install -i https://pypi.tuna.tsinghua.edu.cn/simple -e ".[dev]"
uvicorn npu_webhook.main:app --reload --port 18900
```

### Chrome 扩展

```bash
cd extension
npm install
npm run dev
# Chrome → 扩展管理 → 开发者模式 → 加载已解压的扩展 → 选择 extension/dist
```

## 分发

| 平台 | 格式 | 获取方式 |
|------|------|----------|
| Linux | AppImage | GitHub Releases |
| Windows | EXE 安装包 | GitHub Releases |

## License

MIT
