# npu-webhook

个人知识库 + 记忆增强系统。通过 Chrome 扩展在 AI 对话和日常浏览中自动捕获、检索、注入知识，利用 NPU/iGPU 闲置算力处理 embedding。

## 技术栈

- 后端: FastAPI + Uvicorn, Python 3.11+
- 向量库: ChromaDB (嵌入式)
- 全文搜索: SQLite FTS5 + jieba
- Embedding: bge-small-zh-v1.5 (ONNX), 支持 OpenVINO (Intel NPU) / DirectML (AMD NPU)
- Chrome 扩展: Manifest V3 + Preact + Vite
- 打包: PyInstaller + AppImage (Linux) / NSIS (Windows)

## 开发规范

- Python 代码使用 ruff 格式化和 lint
- 类型注解: 所有公开函数必须有类型注解
- 测试放 `tests/` 目录, 使用 pytest
- 调试代码放 `tmp/`, 使用后删除
- API 路径前缀: `/api/v1/`
- 后端端口: 18900
- 使用 venv 管理 Python 依赖
- pip 使用清华源

## 项目结构

- `src/npu_webhook/` - Python 后端
- `extension/` - Chrome 扩展
- `packaging/` - 打包配置
- `tests/` - 测试代码
