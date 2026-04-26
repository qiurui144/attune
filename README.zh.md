# Attune

[English](README.md) · [中文](README.zh.md)

个人 AI 知识库 + 记忆增强系统。

本仓库包含两条并行的产品线：

- **Python 原型线**（本目录 `src/npu_webhook/`）— 快速验证算法与实验特性。基于 FastAPI + ChromaDB + SQLite FTS5
- **Rust 商用线**（`rust/`）— 面向知识密集型专业人士的**私有 AI 知识伙伴**：主动进化、对话式、混合智能、本地加密。详见 [`rust/README.md`](rust/README.md)

Chrome 扩展协议相同，两个后端可任意切换。

> **2026-04 更新**：Rust 线新增 6 大能力 — 用户批注 + AI 批注（4 角度分析）、
> 上下文压缩流水线（摘要缓存 70-85% token 节省）、批注加权 RAG、Token Chip 成本透明、
> 硬件感知默认摘要模型、扫描版 PDF OCR 兜底。完整回归 57 断言 100% 通过，总测试 299。
> 详见 `rust/RELEASE.md`。

## 功能

- **自动捕获** — MutationObserver 监听 ChatGPT / Claude / Gemini 对话，user+assistant 配对后自动入库
- **无感注入** — 发送提问时自动搜索知识库，将相关知识按类型（笔记 / 历史对话 / 网页）以前缀拼接；动态预算 2000 字，按相关性分配
- **层级语义分块** — 两层粒度（章节 ~1500 字 / 段落块 512 字），两阶段层级检索（章节召回 → 段落精排 → 父章节上下文），语义完整性显著优于固定截断
- **文件直传** — Side Panel 拖拽上传 PDF / DOCX / MD / TXT / 代码，后端自动解析入库，会话内上传文件优先检索
- **混合搜索** — 向量语义搜索（ChromaDB）+ FTS5 全文搜索（jieba 分词），RRF 融合排序
- **本地目录索引** — 绑定文件夹，watchdog 实时监听变更，自动解析 MD / TXT / 代码 / PDF / DOCX
- **多后端 Embedding** — Ollama HTTP API（推荐）/ ONNX Runtime / OpenVINO（Intel NPU/iGPU）
- **芯片级检测** — 自动识别 Intel Meteor/Lunar/Arrow Lake、AMD Phoenix/Hawk/Strix Point，精确匹配驱动
- **知识管理 UI** — Side Panel（搜索 / 时间线 / 文件 / 状态）+ Popup 快速操作 + Options 设置
- **系统托盘** — pystray 系统托盘常驻，uvicorn 后台线程，双击图标自动启动
- **跨平台** — Linux + Windows，AppImage / EXE 一键安装

## 快速开始

### 5 步上手（Rust 商用线，推荐）

1. **下载** 二进制：从 [Releases](../../releases) 页拿对应平台的包，或源码 `cargo build --release`（见下文「源码编译」）
2. **运行** Linux：`./attune-server --host 127.0.0.1 --port 18900`；Windows：双击 `attune-server.exe`。首次运行会创建 `~/.local/share/attune/`（或 `%LOCALAPPDATA%\attune\`）
3. **打开** 浏览器访问 `http://localhost:18900/`，自动进入 5 步首次运行向导
4. **设主密码 + 选 LLM 后端**（向导第 3 步）：参考下文「AI 模型平台」表格选 endpoint + model 并粘贴 API key（用主密码加密存储）
5. **绑定数据**（向导最后一步）：拖文件、绑文件夹，或先跳过，之后用 Items / Reader 操作

完成。Cmd+K 在 Chat / Items / Reader / 会话 / 设置之间跳转，全局顶栏可随时锁定 vault。

### AI 模型平台

Attune 走 **OpenAI 兼容 chat 协议**，任何暴露 `/v1/chat/completions` 的服务都能接。Settings → AI 大脑 tab 有「快捷预设」下拉，自动填 endpoint + model，你只需粘贴 API key。

| 厂商 | base_url | 推荐模型 | 价格（输入）* | 拿 key |
|------|----------|---------|--------------|--------|
| **DeepSeek** | `https://api.deepseek.com/v1` | `deepseek-chat` | ¥1/M tok | [platform.deepseek.com](https://platform.deepseek.com/api_keys) |
| **阿里百炼 / Qwen** | `https://dashscope.aliyuncs.com/compatible-mode/v1` | `qwen-plus` | ¥4/M tok | [bailian.console.aliyun.com](https://bailian.console.aliyun.com/?apiKey=1) |
| **智谱 GLM** | `https://open.bigmodel.cn/api/paas/v4` | `glm-4-plus` | ¥50/M tok | [open.bigmodel.cn](https://open.bigmodel.cn/usercenter/apikeys) |
| **月之暗面 Kimi** | `https://api.moonshot.cn/v1` | `moonshot-v1-8k` | ¥12/M tok | [platform.moonshot.cn](https://platform.moonshot.cn/console/api-keys) |
| **百川** | `https://api.baichuan-ai.com/v1` | `Baichuan4-Turbo` | ¥15/M tok | [platform.baichuan-ai.com](https://platform.baichuan-ai.com/console/apikey) |
| **Ollama 本地** | `http://localhost:11434/v1` | `qwen2.5:7b` | 免费 / 本地算力 | `curl -fsSL https://ollama.com/install.sh \| sh && ollama pull qwen2.5:7b` |
| **OpenAI** | `https://api.openai.com/v1` | `gpt-4o-mini` | ~¥3/M tok | [platform.openai.com](https://platform.openai.com/api-keys) |

*以上为各家输入 token 价格估算（写作时点）；具体以官方价格页为准（含输出 token 价、首充优惠等）。

**推荐**：日常用 DeepSeek（性价比最高），有 16 GB+ GPU 选 Ollama 本地，重要场景上 OpenAI。

### Python 原型线

#### 1. 后端

```bash
git clone <repo-url> && cd attune
python -m venv .venv && source .venv/bin/activate
pip install -i https://pypi.tuna.tsinghua.edu.cn/simple -e ".[dev]"
uvicorn npu_webhook.main:app --reload --port 18900
```

验证：`curl http://localhost:18900/api/v1/status/health` → `{"status":"ok"}`

#### 2. Embedding 模型

**Ollama（推荐）：**

```bash
curl -fsSL https://ollama.com/install.sh | sh
ollama pull bge-m3
```

后端默认 `device: auto`，自动连接 Ollama bge-m3（1024 维）。无 Ollama 时回退 ONNX，无模型时回退 FTS5 全文搜索。

**ONNX（可选）：** 将 `model.onnx` + `tokenizer.json` 放到 `~/.local/share/attune/models/bge-m3/`。

#### 3. Chrome 扩展

```bash
cd extension
npm install --registry https://registry.npmmirror.com
npm run build
```

Chrome → `chrome://extensions` → 开发者模式 → 加载已解压的扩展 → 选择 `extension/` 目录。

#### 4. 部署检查

```bash
curl -s -X POST http://localhost:18900/api/v1/models/check | python3 -m json.tool
```

返回内核 / 芯片 / 驱动 / 模型 / 依赖完整报告和一键安装命令。

#### 5. 测试

```bash
pytest tests/ -v    # 78 个测试（36 后端单元 + 42 扩展 E2E）
```

## 使用手册

### 对话捕获

扩展在 ChatGPT / Claude / Gemini 页面自动注入 Content Script：

- **状态指示器**：右下角圆点（绿=在线、黄=处理中、红=离线、灰=禁用），点击打开 Side Panel
- **自动捕获**：检测到新的 user+assistant 对话对时，2s debounce + 流式完成检测后自动入库
- **去重**：djb2 hash 即时去重 + Worker 端 1h TTL 缓存（session storage 防 SW 被杀）

### 知识注入

发送消息时自动触发：

1. 拦截发送按钮点击（capture phase）
2. 读取输入内容 → `/search/relevant` 搜索知识库
3. 有结果时构建前缀并修改输入框：
   ```
   [以下是来自个人知识库的相关参考，请结合回答]
   📝 个人笔记: ...
   💬 历史对话: ...
   📄 本地文件: ...
   ---
   {原始问题}
   ```
4. 释放点击完成发送

注入模式可在 Options 设置：自动 / 手动 / 禁用。

### Popup 面板

- 连接状态指示（绿/红）+ 知识条目数 / 向量数
- 注入开关 toggle
- 「打开知识面板」→ Side Panel、「设置」→ Options

### Side Panel

| 标签 | 功能 |
|------|------|
| 搜索 | 输入关键词 + source_type 过滤，点击展开详情 |
| 时间线 | 按日期分组，分页加载，支持删除 |
| 文件 | 拖拽上传 PDF/DOCX/MD/TXT/代码，进度显示，会话内优先检索 |
| 状态 | 8 项指标（连接/版本/设备/模型/条目/向量/待处理/监控目录） |

### 本地目录索引

```bash
# 绑定目录
curl -X POST http://localhost:18900/api/v1/index/bind \
  -H "Content-Type: application/json" \
  -d '{"path": "/home/user/notes", "recursive": true}'

# 查看索引状态
curl http://localhost:18900/api/v1/index/status
```

支持：`.md` `.txt` `.py` `.js` `.ts` `.go` `.rs` `.java` `.pdf` `.docx`

## 硬件支持

### 芯片-驱动匹配表

| 芯片代 | NPU/iGPU | 最低内核 | 驱动模块 | 软件栈 |
|--------|----------|---------|---------|--------|
| Intel Meteor Lake (Core Ultra 1xx) | NPU 11 TOPS + Xe-LPG | 6.3 / 6.5 | intel_vpu + i915 | OpenVINO 2024.0+ / Level Zero |
| Intel Lunar Lake (Core Ultra 2xx V) | NPU 48 TOPS + Xe2 | 6.8 | intel_vpu + xe | OpenVINO 2024.4+ / Level Zero |
| Intel Arrow Lake (Core Ultra 2xx) | NPU 13 TOPS + Xe-LPG+ | 6.8 | intel_vpu + i915 | OpenVINO 2024.4+ / Level Zero |
| Intel Alder/Raptor Lake | Iris Xe iGPU | 5.15 / 6.0 | i915 | OpenVINO GPU 推理 |
| AMD Phoenix (Ryzen 7x40) | XDNA1 10 TOPS | 6.10 | amdxdna | IOMMU SVA |
| AMD Hawk Point (Ryzen 8x40) | XDNA1 16 TOPS | 6.10 | amdxdna | IOMMU SVA |
| AMD Strix Point (Ryzen AI 3xx) | XDNA2 50 TOPS | 6.14 | amdxdna | 6.18-6.18.7 有回归 |
| AMD Krackan Point (Ryzen AI 2xx) | XDNA2 50 TOPS | 6.14 | amdxdna | IOMMU SVA |

### 一键安装

部署检查 API 根据检测到的芯片自动生成安装命令：

```bash
# Intel NPU/iGPU
sudo apt-get install -y intel-npu-firmware level-zero intel-level-zero-gpu intel-opencl-icd
pip install openvino

# AMD NPU (内核 >= 6.14)
sudo modprobe amdxdna
sudo apt-get install -y linux-firmware

# AMD NPU (内核 < 6.14)
sudo apt install amdxdna-dkms  # 需要 AMD 官方源

# Ollama（通用，推荐）
curl -fsSL https://ollama.com/install.sh | sh && ollama pull bge-m3
```

## 配置

配置文件：Linux `~/.config/attune/config.yaml`，Windows `%APPDATA%\attune\config.yaml`

```yaml
embedding:
  model: "bge-m3"            # bge-m3 / bge-small-zh-v1.5 / bge-large-zh-v1.5
  device: "auto"             # auto / ollama / cpu / directml / rocm / openvino

search:
  default_top_k: 10
  vector_weight: 0.6
  fulltext_weight: 0.4

ingest:
  min_content_length: 100
  max_upload_mb: 20           # 文件上传大小限制（MB）
  excluded_domains: ["mail.google.com", "web.whatsapp.com"]
```

`device: auto` 优先 Ollama，失败回退 ONNX。不存在配置文件时使用默认值。

## API

所有端点前缀 `/api/v1/`，完整文档访问 `http://localhost:18900/docs`。

| 方法 | 路径 | 用途 |
|------|------|------|
| POST | `/ingest` | 知识注入（纯文本） |
| POST | `/upload` | 文件直传（multipart，PDF/DOCX/MD/TXT/代码） |
| GET | `/search?q=&top_k=` | 混合搜索 |
| POST | `/search/relevant` | 相关知识搜索（注入用，层级检索 + 动态预算） |
| GET/PATCH/DELETE | `/items[/{id}]` | 知识条目 CRUD |
| POST/DELETE/GET | `/index/bind\|unbind\|status` | 目录索引管理 |
| GET | `/status` | 系统状态 |
| GET/PATCH | `/settings` | 配置管理 |
| GET | `/models` | 模型列表 + 设备检测 |
| POST | `/models/check` | 部署前置检查 |
| POST | `/models/download` | 触发模型下载 |

## 数据存储

| 数据 | Linux | Windows |
|------|-------|---------|
| 数据库 | `~/.local/share/attune/knowledge.db` | `%LOCALAPPDATA%\attune\knowledge.db` |
| 向量库 | `~/.local/share/attune/chroma/` | `%LOCALAPPDATA%\attune\chroma\` |
| 模型 | `~/.local/share/attune/models/` | `%LOCALAPPDATA%\attune\models\` |
| 配置 | `~/.config/attune/config.yaml` | `%APPDATA%\attune\config.yaml` |

## License

MIT
