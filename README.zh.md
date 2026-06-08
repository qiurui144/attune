# Attune

[中文](README.zh.md) · [English](README.md) · [Wiki](https://wiki.your-company.com/attune/) · [价格 & 计划](https://wiki.your-company.com/plans/attune-pricing/)

> 📌 **本仓文档以中文为主，英文为辅**。中文版（本文）持续更新；英文版 [README.md](README.md) 作为面向国际开源社区的精简对照。

**私有 AI 知识伙伴** — 本地优先、全网增强、越用越懂你的专业。

Attune 是面向**任何领域个人知识工作者**的通用 AI 知识库 — 学生、独立开发者、研究员、写作者、AI 重度用户都能用。你的兴趣领域它会越用越懂；本地知识够用时在本地决定，不够用时主动上网补全；所有数据加密存在你自己的设备上，换设备、换工作都能带走。

**行业用户**（律师 / 专利代理 / 医生 / 学者 / 售前工程师）= Attune（本仓 OSS）+ 对应的 `attune-pro/<行业>-pro` 插件包，详见下方三产品矩阵。

## 📥 下载安装包

最新正式版：**server v1.2.0** · **desktop v1.2.0**（2026-06-01：GitConnector + WASM 跨平台 agent 分发 + 一键依赖部署），建立在 **v1.1.0（Agent Control Plane / ACP）** 与 **v1.0 GA** 之上。完整 changelog 见 [`rust/RELEASE.md`](rust/RELEASE.md)（版本 SSOT）。

去 [Releases 页面](https://github.com/qiurui144/attune/releases) 取二进制 — 选最新的 `vX.Y.Z`（server/CLI tarball）与 `desktop-vX.Y.Z`（Tauri 安装器）tag。每个归档随附 SHA256 校验文件。

| 形态 | tag 前缀 | 产物 |
|------|---------|------|
| **桌面应用**（含 Web UI + 系统托盘） | `desktop-vX.Y.Z` | Windows NSIS `.exe` / MSI · Linux `.deb` / `.rpm` / `.AppImage` |
| **Server / CLI**（headless / NAS / 服务器） | `vX.Y.Z` | `attune-linux-x86_64.tar.gz` · `attune-linux-aarch64.tar.gz` · `attune-macos-aarch64.tar.gz` · `attune-windows-x86_64.zip` |

> macOS Intel 走源码编译: `cargo build --release`（Apple Silicon Mac 已覆盖现代用户）。

### 系统包管理器（v1.0+ 推荐）

**v1.0.0** 起 Attune 接入主流包管理器,一行命令安装 + 自动升级:

```bash
# Windows
winget install qiurui144.Attune

# Debian / Ubuntu
curl -fsSL https://qiurui144.github.io/attune/attune-archive-keyring.gpg | sudo tee /usr/share/keyrings/attune-archive-keyring.gpg > /dev/null
echo "deb [signed-by=/usr/share/keyrings/attune-archive-keyring.gpg] https://qiurui144.github.io/attune/apt stable main" | sudo tee /etc/apt/sources.list.d/attune.list
sudo apt update && sudo apt install attune

# RHEL / Fedora / openSUSE
sudo curl -fsSL -o /etc/yum.repos.d/attune.repo https://qiurui144.github.io/attune/rpm/attune.repo
sudo dnf install attune
```

桌面应用还内置 **Tauri auto-updater** — 装上一次后,新版本会在应用内静默提示,点 "立即更新" 即可下载升级.完整安装 + 排障指南: [`docs/INSTALL.md`](docs/INSTALL.md).

## v1.0 GA 以来的更新（当前：v1.2.0）

v1.0→v1.2 在 GA 核心之上叠加生产级治理与跨平台能力。完整发布说明见 [`rust/RELEASE.md`](rust/RELEASE.md)；各能力 × 模块 × 技术栈映射见 [`rust/DEVELOP.md` → 能力矩阵 × 技术栈选型](rust/DEVELOP.md#能力矩阵--技术栈选型)。

- **Agent Control Plane（ACP，v1.1.0）** — 中央 agent 注册表 + typed handoff，声明式 DAG flow 执行器，每 `agent×model` 失败 telemetry，cost-aware 调度器，workspace 级质量门（阈值只升不降）。把整个 agent 生态当一个工程组织治理。
- **跨平台 agent 分发（WASM，v1.2.0）** — 确定性 agent/skill 可编到 `wasm32-wasip1`，由内嵌 `wasmtime` 执行；一个 `.attunepkg` 含一份 `.wasm` 即在 Windows / Linux / riscv64 全平台运行。WASM-safe 的 `attune-agent-sdk` leaf crate 让 `Agent` trait 零 native 依赖。
- **GitConnector（v1.2.0）** — 直接从 Git 仓库导入知识库（GitHub / GitLab / Gitea / Bitbucket / Codeberg / sr.ht 的 HTTPS）：clone → glob 过滤 → 入库 → 跟随上游 commit。本地完成、导入路径零 LLM 调用，带 SSRF 防护。
- **隐私出网门 OutboundGate + `PrivacyTier::L0`「永不出网」** — 每个网络 egress（LLM / Cloud / WebDAV / Web Search / Telemetry）统一经一处 gate 裁决 settings + PII 脱敏；标 L0 的内容拒绝任何云端 LLM 调用。
- **一键依赖部署** — Ollama readiness 检测 + 应用内一键 install/pull、底座模型一键 ensure、LM Studio 端点自动识别，非技术用户无需碰终端。

## v0.6.0-rc.5 亮点（2026-04-28）

🎯 **三赛道 PRO 级 benchmark** — 法律 + 通用英文 + 中文八股双赛道端到端验证：

| 场景 | Hit@10 | MRR | 评级 |
|------|--------|-----|------|
| 法律 / legal corpus | **0.80** | 0.50 | ✅ PRO |
| Rust / rust-book | **1.00** | **1.00** | ✅ PRO 满分 |
| 中文八股 / cs-notes | **1.00** | **1.00** | ✅ PRO 满分 |
| **答案 5 维度 (legal golden_qa)** | **25.00/25** (100%) | 10/10 excellent | ✅ vs baseline +39% |

🔒 **Phase A.5 三层隐私模型**：
- **L0 🔒**：文件级标记，chunk 永不出网（强制本地 LLM）
- **L1 默认**：12 类格式化 PII（身份证 ISO 7064 / 手机 / 邮箱 / 8 家 API key 等）+ 可逆 `[KIND_N]` placeholder + 出网审计 + CSV 导出（合规审查可用）
- **L3**（v0.7）：LLM 语义脱敏，Tier T3+/K3 硬件自动启用

🌐 **F-Pro 跨域污染防御**：
- `items.corpus_domain` 字段 + `[领域: legal]` chunk 前缀 + 跨域 penalty (0.4) + 关键词 query intent 检测（零 LLM 调用）
- 共享 vault 也能"逻辑分域" — 中文法律 query 不再拉出 Java 算法内容

📋 **证据流端到端**：chat citation 现在含真实 `breadcrumb`（章节路径）+ `chunk_offset_start/end`（Reader 跳转锚点）+ `confidence`（1-5，从 LLM 严格 prompt 解析）。

复现命令：`bash scripts/bench-orchestrator.sh all && python3 scripts/run-final-eval.py`。完整 benchmark 方法论见 [`docs/benchmarks/dual-track-baseline.md`](docs/benchmarks/dual-track-baseline.md)；v0.6 发布说明见 [`rust/RELEASE.md`](rust/RELEASE.md)（版本 SSOT）。

---

## 双产品线

本仓库包含两条并行的产品线：

- **Python 原型线**（`python/src/attune_python/`）— 快速验证算法与实验特性。基于 FastAPI + ChromaDB + SQLite FTS5
- **Rust 商用线**（`rust/`）— 面向**任何领域个人知识工作者**的通用 AI 知识库：主动进化、对话式、混合智能、本地加密。详见 [`rust/README.md`](rust/README.md)

Chrome 扩展协议相同，两个后端可任意切换。

---

## 三产品矩阵（Attune 在哪里）

> 决策性定位（2026-04-27）：Attune（本仓 OSS）是**通用个人知识库**，**零行业绑定**。行业深度（律师 / 医生 / 学者 / 售前 / 工程师 / 专利代理）由商业插件包 `attune-pro` 交付。律所 B2B 小团队场景由单独产品 `attune-enterprise` 处理。

| 产品 | License | 形态 | 用户群 |
|------|---------|------|--------|
| **`attune`**（本仓） | Apache-2.0 | Tauri 桌面 / Chrome 扩展 | **个人通用用户** — 通用 RAG / 加密 vault / 浏览捕获 / MCP outlet |
| **`attune-pro`**（私有） | Proprietary | Plugin packs (.attunepkg signed) 装载到 attune | **个人行业用户** — 律师 / 售前 / 专利 / 技术 / 医疗 / 学术 纵向 packs |
| **`attune-enterprise`**（独立产品） | Proprietary | Django + Vue B2B SaaS | **律所小团队** — 多租户 RBAC + 案件分配 + 多人协作 |

**等式**：
- 个人通用用户 = `attune (OSS)`
- 个人行业用户 = `attune (OSS)` + `attune-pro/<vertical>-pro` plugin pack
- 行业小团队 = `attune-enterprise`

三者技术上独立运行（无跨产品运行时依赖），战略上配套（同团队不同用户群）。完整战略 + 准入规则见 [`docs/oss-pro-strategy.zh.md`](docs/oss-pro-strategy.zh.md)（双语）。

> **2026-04 更新**：Rust 线新增 6 大能力 — 用户批注 + AI 批注（4 角度分析）、
> 上下文压缩流水线（摘要缓存 70-85% token 节省）、批注加权 RAG、Token Chip 成本透明、
> 硬件感知默认摘要模型、扫描版 PDF OCR 兜底。完整回归 57 断言 100% 通过，总测试 299。
> 详见 `rust/RELEASE.md`。

## 三大支柱（Rust 线）

### 主动进化
无需配置即可从每次查询中学习。本地未命中作为信号，后台 `SkillEvolver` 周期性请 LLM 生成同义词扩展，悄悄提升召回率。三个月后同样的 query 返回明显更相关的结果——无"重新训练"按钮。

### 对话式伙伴
RAG Chat 是主界面。每个回答都带可点的 citation chip 在侧栏打开原文。会话持久化且可跨时间搜索——三周前的讨论从断点续聊。

### 混合智能
本地知识优先。本地 vault 没命中时，无头 Chrome（或 Edge）自动浏览网页——**零 API key、零订阅费**。每个回答都明确标注来源（本地 / 网络）。专业数据加密在本地，公开信息现取现用。

---

## 自主可控 & 透明

- **零绑定定价**：你只为软件本身 + 自己的 LLM token 付费（如选云端模型）。无中间商、无搜索 API 订阅、无隐藏费用。
- Argon2id(64MB/3r) + AES-256-GCM 字段级加密 + Device Secret 多重防护，所有数据本地保存。
- 单个 ~30 MB 静态 Rust 二进制——零运行时依赖。
- 跨设备迁移：加密 `.vault-profile` 导出/导入。

---

## 适合人群

**OSS attune** 面向**任何领域的个人知识工作者**：

| 用户 | 主要价值 |
|------|---------|
| **学生 / 独立开发者** | 个人 RAG 覆盖阅读笔记 / 代码仓库 / 博客草稿；跨主题会话持久化 |
| **研究人员 / 写作者** | 跨主题对话式检索，引用可追溯到源段落 |
| **AI 重度用户 / Prosumer** | 私有版 AI 记忆：本地加密 + 可插拔 LLM + 自托管 |
| **任何领域的知识工作者** | 通用 vault + 浏览捕获 + 自动 bookmark + 跨会话延续 |

**行业用户** — 装 OSS attune **+** 对应的 `attune-pro/<行业>-pro` 插件包：

| 行业 | attune-pro 提供 |
|------|---------------|
| **律师** | `law-pro`：合同审查 / 风险矩阵 / 起草 / OA 答辩 / 条款检索 + CaseNo extractor + Case 卷宗工作流 |
| **专利代理** | `patent-pro`（M3+）：FTO 检索工作流 + USPTO/EPO 自动化 + 专利号 extractor |
| **销售 / 售前** | `presales-pro`：竞品分析 / BANT 评分 / 报价 / demo 脚本 |
| **医生 / 学者** | `medical-pro` / `academic-pro`（计划中）：医学术语 / 病例模板；引用图谱 / 论文助手 |

---

## 功能（Rust 商用线）

> 各能力 × 实现模块 × 技术栈选型的完整映射见 [`rust/DEVELOP.md` → 能力矩阵 × 技术栈选型](rust/DEVELOP.md#能力矩阵--技术栈选型)（不在此重复版本号 / 模块清单）。

- **首次运行向导** — 欢迎 · 主密码 · LLM 后端（本地 Ollama / 云端 API / demo）· 硬件检测推荐模型 · 首次绑定数据
- **内置 Chat + RAG** — citation chip 引用 · session 历史 · Token Chip 成本估算（本地免费 / 云端 $ 实时）
- **混合搜索** — usearch HNSW 向量 + tantivy BM25（jieba 中文分词 + LowerCaser/Stemmer 多语言）+ RRF 融合；本地未命中时浏览器自动化网搜（驱动系统 Chrome，零 API 费）
- **多层记忆** — L0 原始 chunk / L1 摘要 / L2 情景 / L3 语义，tier-aware 上下文装配按最省 tier 答题
- **Reader + 批注** — 全文阅读 + 用户批注（5 标签 × 4 色）+ AI 4 视角分析，批注影响 RAG 权重
- **采集来源** — 本地文件夹 · WebDAV · Email IMAP · GitConnector（Git 仓库导入）
- **Office Helper** — 结构化 OCR（document/receipt/table/card/id_card 等场景 + GB/Luhn 校验位）+ whisper.cpp 异步会议转写
- **Agent / Skill / Workflow** — ACP agent 治理 + WASM 跨平台分发 + SkillClaw 风格自进化 + workflow DAG
- **隐私优先** — 加密 vault（Argon2id + AES-256-GCM + Device Secret）+ OutboundGate 出网门 + L0 永不出网 + PII 脱敏 + DSAR/审计
- **插件架构** — Ed25519 签名 YAML 插件（社区 + 商业双轨）+ 插件市场 + MCP 接入
- **跨平台单二进制** — Linux + Windows（macOS 未来）；NAS 模式（rustls TLS + Bearer auth）；嵌入式 Preact Web UI

## 快速开始

### 5 步上手（Rust 商用线，推荐）

1. **下载** 二进制：从 [Releases](../../releases) 页拿对应平台的包，或源码 `cargo build --release`（见下文「源码编译」）
2. **运行** Linux：`./attune-server-headless --host 127.0.0.1 --port 18900`；Windows：双击 `attune-server-headless.exe`。首次运行会创建 `~/.local/share/attune/`（或 `%LOCALAPPDATA%\attune\`）
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
git clone <repo-url> && cd attune/python
python -m venv .venv && source .venv/bin/activate
pip install -i https://pypi.tuna.tsinghua.edu.cn/simple -e ".[dev]"
uvicorn attune_python.main:app --reload --port 18900
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

> 产品形态已演进：早期向 AI 网站 DOM 注入前缀的浏览器扩展 injector 已于 cleanup-r15 移除，产品转向**内置 Chat + RAG**（Tauri 桌面应用）。Chrome 扩展现仅做对话捕获 / 文件上传等采集入口（共享 `/api/v1/*` 协议），不再注入。

### 内置 Chat + RAG（主交互）

- 桌面应用打开即进两栏 chat-first 界面（仿 ChatGPT），左侧栏可收起为 64px 图标条。
- RAG 回答带可点击 citation chip → 在侧抽屉打开原文；session 持久化、跨时间检索。
- 发送按钮旁常驻 Token Chip 成本估算（🟢 本地免费 / 🔵 云端 $ 实时）；chat 头部 model chip 一键换模型。
- `⌘K` 全局命令面板在 Chat / Items / Reader / Sessions / Settings 间跳转；顶栏 🔒 随时锁定 vault。

### Reader + 批注

全文阅读 + 5 预设标签 × 4 颜色用户批注 + AI 4 视角分析（风险 / 过时 / 亮点 / 疑问）；批注影响 RAG 召回权重（亮点/风险 ×1.5、疑问 ×1.2、过时排除），批注内容 AES-256-GCM 加密。

### 采集来源

本地文件夹监听 · WebDAV 远程目录（ETag 增量）· Email IMAP（正文 + 附件，UID 增量）· **GitConnector**（从 Git 仓库导入，v1.2.0）。

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

## 写自己的 Skill（免费版 + Pro 版机制相同）

**Skill** 是一个小型 YAML + prompt 包，当你 chat 消息命中关键词或正则时 Attune 会主动建议运行它。免费版与 Pro 版加载机制完全一致 — Pro 只是预装更多 skill。**整个流程不需要手编 YAML：写好 / 下载到目录后，在 Settings → Skills 里 toggle 启用就行。**

**1. 建目录**

```
~/.local/share/attune/plugins/<plugin-id>/
```

（Windows：`%APPDATA%\attune\plugins\<plugin-id>\`）

**2. 写 `plugin.yaml`**

```yaml
id: my-plugin/contract-quick-review
name: 快速合同审查
type: skill
version: "0.1.0"
description: 30 秒读完合同关键风险

chat_trigger:
  enabled: true            # 插件作者可发布"默认禁用"
  needs_confirm: true      # 命中后弹确认再跑
  priority: 5              # 多 skill 同时命中时数字大的优先
  patterns:
    - '帮我.*审查.*合同'      # 任一正则命中即匹配
  keywords: ['审查合同', '合同风险']
  min_keyword_match: 1     # 关键词最少命中数
  exclude_patterns: ['起草']  # 命中即否决（即使 patterns/keywords 命中）
  requires_document: true  # 只在 chat 上下文含文件时触发
```

**3. 写 `prompt.md`** — 这是 skill 真正运行时加载给 LLM 的提示词。

**4. 重启 Attune**，让插件注册器重扫目录。

**5. 打开 Settings → Skills 标签**。新 skill 会列出，关键词高亮显示，toggle 启用 / 禁用即时生效，全程不再碰 YAML。

**分发给别人**：把目录打包为 `<plugin-id>.attunepkg`，对方解压到同样的 plugins 目录即装即用。Pro 版的行业 skill 集（律师 / 售前 / 学术）走完全一样的路径，只是出厂预装。

## 贡献

- 见 [CONTRIBUTING.md](CONTRIBUTING.md)（待补）和 [NOTICE](NOTICE)
- 商业插件（attune-pro、医师 / 学者 / 律师 Pro 包）开发由 [attune-pro 仓](https://github.com/qiurui144/attune-pro) 独立处理（闭源）
- bug 报告 / 特性请求：[GitHub Issues](https://github.com/qiurui144/attune/issues)

## 文档

- [产品定位设计](docs/superpowers/specs/2026-04-17-product-positioning-design.md)
- [前端重设计 spec](docs/superpowers/specs/2026-04-19-frontend-redesign-design.md)
- [UX 质量基础设施](docs/superpowers/specs/2026-04-19-ux-quality-design.md)
- [数据基础设施](docs/superpowers/specs/2026-04-19-data-infrastructure-design.md)
- [分发 & 合规](docs/superpowers/specs/2026-04-19-distribution-compliance-design.md)
- [测试金字塔](docs/TESTING.md)
- [OSS × Pro 战略](docs/oss-pro-strategy.zh.md)（双语）

## License

- 本仓（attune 开源核心）：[Apache-2.0](LICENSE)
- 商业插件（attune-pro）：[闭源专有](https://github.com/qiurui144/attune-pro)，会员制订阅
- 详见 [NOTICE](NOTICE)
