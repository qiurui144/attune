# 快速开始 — Attune

## 1. 安装

### Linux

```bash
# AppImage (推荐，无需 root)
# 到 GitHub Releases 下载当前版本的 Attune_*_amd64.AppImage
xdg-open https://github.com/qiurui144/attune/releases/latest

# 或 deb 包
# 到 GitHub Releases 下载当前版本的 Attune_*_amd64.deb
sudo dpkg -i Attune_*_amd64.deb
```

### Windows

下载当前稳定版 [NSIS/MSI 安装包](https://github.com/qiurui144/attune/releases/latest)，双击安装。

### Chrome 扩展

```
chrome://extensions/ → 加载已解压的扩展程序 → 选 attune/extension/dist 目录
```

## 2. 首次启动

1. 启动 Attune 桌面应用
2. 设置 Master Password（用于加密 vault）
3. 配置 AI 执行路径：
   - 默认：填写云端 OpenAI-compatible endpoint / model / key
   - 本地高性能或边缘设备：填写 edge scheduler 地址，例如 `http://127.0.0.1:8090`

Attune 不默认安装 Ollama 或拉取模型权重。具体本地模型、RVV/AVX/OpenVINO/DirectML 等优化由 edge scheduler 或用户自管服务负责。

## 3. 第一次问答

### A. 上传一个文件

```
Settings → 文件管理 → 上传 → 选 .md / .txt / .pdf
```

后台会自动：
- 解析文件 → 提取章节 + chunk
- 生成 embedding（GPU 几秒，CPU 数分钟）
- 写入加密 vault（AES-256-GCM）

### B. Chat 问答

```
Chat 面板 → 输入问题 → 发送
```

Attune 会：
- 检索 top-5 相关 chunk（混合 BM25 + 向量 + reranker）
- 自动检测 query 领域（legal/tech/general）
- 把脱敏后的 chunks 喂给 LLM
- 返回带 [n] 引用号的答案 + citations 数组（每个含 title / breadcrumb / chunk_offset）

## 4. 验证 benchmark 数字（可选）

```bash
git clone https://github.com/qiurui144/attune
cd attune

# 1. 拉测试语料（GitHub 公开仓库 + 版本固化）
bash scripts/download-corpora.sh

# 2. 一站式 bench（使用当前配置的云端 LLM 或 edge scheduler）
bash scripts/bench-orchestrator.sh all

# 3. 跑 queries.json 15 题
python3 scripts/run-final-eval.py
```

预期输出：
```
Scen A 法律:    Hit@10=0.80  ✅ PRO
Scen B Rust:    Hit@10=1.00  ✅ PRO 满分
Scen C 中文八股: Hit@10=1.00  ✅ PRO 满分
```

## 5. 升级到 Attune Pro

如果你是律师/医生/学者等行业用户，装 attune-pro plugin pack：

```bash
# 假设已购买 license
attune-pro install --license <YOUR_KEY> law-pro
```

详见 [Attune Pro 价格 & 计划](/plans/attune-pricing)。

## 6. 常见问题

### Q: 我的文件会被上传到云端吗？

**不会**。Attune 把所有文件加密存在本地 vault（AES-256-GCM + Argon2id），云 LLM 只看脱敏后的 ≤3000 字相关片段。出网内容你能在 Settings → Privacy → 出网审计 看到完整记录 + CSV 导出。

### Q: LLM 必须用付费 API 吗？

不一定。三种模式：
- **远端 token**（默认）— 用 Anthropic / OpenAI / 阿里通义 等的 API key
- **edge scheduler** — 高性能本机、RISC-V、Windows 或 Linux x86 私有部署
- **自管 OpenAI-compatible 本地服务** — 仅作为高级/调试路径，Attune 不负责安装或生命周期

### Q: 哪些文件类型支持？

`.md` / `.txt` / `.pdf` / `.docx` / `.py` / `.rs` / `.js` 等。完整列表见 settings 文件类型 toggle。

### Q: 我能离线使用吗？

可以。Embedding / rerank / 全文搜索都在本地；chat 阶段如果配置了 edge scheduler 或自管本地 OpenAI-compatible 服务，可以完全离线使用。

### Q: macOS 支持吗？

v0.6 不支持。Windows P0 + Linux P1 优先。macOS 在 v0.7 / v0.8 路线图。
