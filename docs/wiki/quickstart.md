# 快速开始 — Attune

## 1. 安装

### Linux

```bash
# AppImage (推荐，无需 root) — v1.0.0
wget https://github.com/qiurui144/attune/releases/download/v1.0.0/attune.AppImage
chmod +x attune.AppImage
./attune.AppImage

# 或 deb 包
wget https://github.com/qiurui144/attune/releases/download/v1.0.0/attune.deb
sudo dpkg -i attune.deb
```

### Windows

下载 MSI 安装包 [v1.0.0](https://github.com/qiurui144/attune/releases/tag/v1.0.0)，双击安装。

### Chrome 扩展

```
chrome://extensions/ → 加载已解压的扩展程序 → 选 attune/extension/dist 目录
```

## 2. 首次启动

1. 启动 Attune 桌面应用
2. 设置 Master Password（用于加密 vault）
3. （可选）配置 LLM API key 或装 Ollama 本地 LLM
   ```bash
   # 推荐配置
   curl -fsSL https://ollama.com/install.sh | sh
   ollama pull bge-m3            # embedding (568MB)
   ollama pull qwen2.5:3b        # chat (轻量) 或
   ollama pull deepseek-r1:14b   # chat (强推理)
   ```

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

# 2. 一站式 bench
ATTUNE_EMBEDDING_BACKEND=ollama \
ATTUNE_CHAT_MODEL=deepseek-r1:14b \
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
- **本地 Ollama** — 装 qwen2.5:3b 或 deepseek-r1:14b 在自己机器上跑
- **K3 一体机** — 律所/医院私有部署（v0.7+）

### Q: 哪些文件类型支持？

`.md` / `.txt` / `.pdf` / `.docx` / `.py` / `.rs` / `.js` 等。完整列表见 settings 文件类型 toggle。

### Q: 我能离线使用吗？

可以。Embedding / rerank / 全文搜索都是本地的；chat 阶段如果配了本地 LLM（Ollama），完全离线可用。

### Q: macOS 支持吗？

v0.6 不支持。Windows P0 + Linux P1 优先。macOS 在 v0.7 / v0.8 路线图。
