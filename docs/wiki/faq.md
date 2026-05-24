# Attune 常见问题

## 安装 / 首次使用

### Q: 装哪个版本？
- **个人通用用户**：装 [Attune OSS (Apache-2.0)](https://github.com/qiurui144/attune/releases) 即可
- **律师 / 医生 / 学者 / 售前**：在 OSS 基础上，再装 [Attune Pro Plugin Pack](/plans/attune-pricing)
- **律所 B2B（多人/多租户/RBAC）**：用 LawControl

### Q: 必须装 Ollama 吗？
不必须。Attune 也能用云端 API（OpenAI / Anthropic / 阿里通义等）。但装 Ollama + bge-m3 + qwen2.5:3b 后体验最好（embedding 全本地、LLM 可本地可云）。

### Q: Linux 哪个发行版支持？
- ✅ **Ubuntu 22.04+** / Debian 12+
- ✅ **Arch / Manjaro**
- ⚠️ **CentOS 8 / RHEL** — 未官方测试
- ❌ Alpine（musl libc 不兼容某些 ONNX 依赖）

## 数据 / 隐私

### Q: 我的文件会被传到哪里吗？
**不会**。详见 [隐私模型](privacy.md)。简而言之：
- 文件 100% 本地（AES-256-GCM 加密 vault）
- 云 LLM 只看脱敏后的 ≤3000 字相关片段
- 每次出网都本地落 audit log（CSV 可导）

### Q: 怎么把某个文件标记为"绝对不出网"？
```
Settings → 文件管理 → 右键文件 → 标记为 🔒 机密
```
或 API: `PATCH /api/v1/items/{id}/privacy_tier {"tier":"L0"}`

### Q: 误删了文件能恢复吗？
默认 Attune 是软删除（`is_deleted=1`）。30 天内可在 Settings → Trash 恢复。30 天后清理脚本物理删除。

### Q: 卸载后数据怎么办？
卸载只移除 binary，**vault 数据保留在 `~/.local/share/attune/`**。要彻底清除：
```bash
rm -rf ~/.local/share/attune/
rm -rf ~/.config/attune/
```

## Chat / RAG

### Q: 为什么我的 query 不返回相关文档？
排查：
1. 文件**索引完成**了吗？看 `index/status` queue 是否为 0
2. **embedding 模型**正确？看 `/api/v1/ai_stack` 的 embedding loaded 字段
3. **query 分词**对吗？中文 query 需要 jieba 分词正常工作
4. 试试 search 直查：`curl http://localhost:18900/api/v1/search?q=xxx`

### Q: chat 答案没有引用号是为什么？
LLM 没遵守 prompt。排查：
1. 你用的 chat 模型支持指令吗？deepseek-r1:14b / qwen2.5:7b 都支持
2. 检索结果是否为空（`knowledge_count=0`）→ LLM 进 fallback prompt

### Q: 为什么 confidence 总是 3？
LLM 没在末尾输出 `【置信度: N/5】` marker。某些模型（如纯 base，不是 instruct）不会遵守。
切到 deepseek-r1:14b 或 qwen2.5:7b-instruct 试试。

## 性能

### Q: ingest 一个 1000 页 PDF 多久？
GPU 加速（bge-m3 Ollama F16）：约 50 chunks/s，1000 页 ≈ 5000 chunks ≈ 100 秒。  
CPU only（ORT 量化）：约 6 chunks/s，1000 页约 14 分钟。

### Q: 可以用 NVIDIA / AMD GPU 吗？
- **NVIDIA**：Ollama 自动 CUDA，attune-server 也 set CUDA_VISIBLE_DEVICES=0
- **AMD**：Ollama 自动 ROCm（实测 Radeon 780M 可用）
- **Intel iGPU/NPU**：Ollama 实验级 OpenVINO，建议优先 NVIDIA

### Q: 能跑在 NAS / 软路由 / Raspberry Pi 上吗？
- **K3 一体机**（RK3588 16GB+）：✅ 官方支持，本地 LLM + embedding/rerank
- **Raspberry Pi 4/5**：⚠️ 仅 indexing 路径可行，LLM 跑不动
- **Synology NAS**：实验级（Docker）

## 商业 / 升级

### Q: OSS 能商用吗？
✅ 可以。Apache-2.0 协议允许商业使用、修改、分发，**仅要求保留原作者署名**。

### Q: Attune Pro 怎么买？
v0.6 还在 EAP（Early Access Program）阶段，订阅未上线。详见 [价格 & 计划](/plans/attune-pricing)。

### Q: 试用期多久？
个人版（OSS）永久免费。Pro 计划 30 天免费试用（M3 商业化后启用）。

### Q: 数据能从 lawcontrol 迁过来吗？
能。Attune 有 `import-from-lawcontrol` 脚本（v0.7 提供 GUI），把 lawcontrol Project + 案件 + 文件迁到 Attune vault。但**用户需主动 export → import**，无后台自动桥接（保护隐私）。

## 开发 / 自定义

### Q: 怎么写自定义 plugin？
看 [PluginHub 开发指南](/pluginhub/)。简言之：
- 写 plugin.yaml（声明 name / id / pii_patterns / chat_trigger 等）
- 写 prompt.md（行业 system prompt）
- 用 attune-pro CLI 打包 + 签名（Ed25519）

### Q: 能贡献代码吗？
欢迎。看 [DEVELOP.md](https://github.com/qiurui144/attune/blob/develop/DEVELOP.md)。提 issue 或 PR 到 `develop` 分支。

### Q: 路线图在哪？
- v0.7：L3 NER + Settings UI 完整 + macOS preview
- v0.8：K3 一体机全本地链路 + 多 vault
- v0.9：BC2BC 协议（Cross-vault knowledge sharing）

详见 [GitHub milestones](https://github.com/qiurui144/attune/milestones)。

## 故障排除

### Q: vault 解锁失败 "invalid password"
- 确认 master password 拼写（区分大小写）
- 看 `~/.local/share/attune/vault.db-wal` 是否过大（>1GB）→ 可能损坏，从备份恢复
- v0.6 起 schema migration 全幂等，升级不会破坏 vault

### Q: `Reranker failed, keeping RRF order` 警告
v0.6.0-rc.5 默认已切 BAAI/bge-reranker-base 官方 ONNX，不再触发 Xenova 量化版的 Expand bug。
如还见到，确认 reranker 模型在 `~/.local/share/attune/models/BAAI_bge-reranker-base/` 完整下载。

### Q: 浏览器扩展失效
1. chrome://extensions → 开发者模式 → 重载
2. 确认 attune-server 跑在 :18900
3. 看 popup 状态指示器：disabled (红) / processing (黄) / captured (绿) / offline (灰)

### Q: 出网审计 CSV 怎么看？
```
Settings → Privacy → 出网审计 → 📥 下载 CSV
```
含字段：ts_iso / direction / provider / model / token / privacy_tier / pre_hash / post_hash / redactions / session_id。

不见原文，但能给合规员审计"什么时候、什么模型、脱敏了多少 PII"。
