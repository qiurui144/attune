# 本地 LLM 部署指导 (可选)

attune **主要使用云版本大模型** — 默认走 OpenAI / Anthropic / Gemini / DeepSeek
等云端兼容 API. **本文档仅适用于**:

- 完全离线 / 隐私敏感场景
- 自有 GPU 服务器, 希望节省云端 token 成本
- 开发 / 测试 attune 时本地 mock LLM

attune **不内置 Ollama, 不打包模型权重**. 本地 LLM 由用户自行部署.

## 1. 装 Ollama

### Linux
```bash
curl -fsSL https://ollama.com/install.sh | sh
```

### Windows
下载 [OllamaSetup.exe](https://ollama.com/download/windows) 双击安装.

### macOS
```bash
brew install ollama
# 或下载 .dmg: https://ollama.com/download/mac
```

## 2. 拉模型 (按硬件选)

| RAM | GPU | 推荐模型 | 下载命令 |
|-----|-----|---------|---------|
| 8 GB | 无独显 | qwen2.5:3b (Q4_K_M) ~2 GB | `ollama pull qwen2.5:3b` |
| 16 GB | 无独显 / 核显 | qwen2.5:7b (Q4_K_M) ~4.7 GB | `ollama pull qwen2.5:7b` |
| 32 GB | NVIDIA 8GB+ / AMD ROCm | qwen2.5:14b (Q4_K_M) ~9 GB | `ollama pull qwen2.5:14b` |
| 64 GB | NVIDIA 16GB+ | qwen2.5:32b (Q4_K_M) ~20 GB | `ollama pull qwen2.5:32b` |
| 128 GB | NVIDIA 24GB+ / Apple M3 Ultra | qwen2.5:72b / llama3.3:70b | `ollama pull qwen2.5:72b` |

低于 14b 的本地模型在长法律推理 / 复杂 RAG 场景**质量不如云端**, 仅建议轻量场景.

## 3. 配 attune 走本地 LLM

应用窗口 → 设置 → 云端大模型:
- Endpoint: `http://127.0.0.1:11434/v1` (Ollama 默认)
- Model: `qwen2.5:7b` (与上一步 pull 一致)
- API key: 任意 (Ollama 不校验)

## 4. 验证

应用窗口 → 内置 chat → 发一句 "你好":
- 返回中文 → 本地 LLM 工作正常
- 报错 → 检查 `ollama serve` 是否在跑 / 端口 11434 是否通

## 5. 硬件加速

- **NVIDIA**: Ollama 自动检测 CUDA
- **AMD GPU (Linux ROCm)**: `HSA_OVERRIDE_GFX_VERSION=11.0.0 ollama serve` (APU)
- **Apple Silicon**: 自动 Metal 后端
- **Intel ARC / NPU**: Ollama 实验性 OpenVINO, 见上游

## 6. 故障排查

| 现象 | 排查 |
|------|------|
| `ollama: command not found` | 未装 Ollama, 回到 §1 |
| attune chat 返 `connection refused` | `ollama serve` 没启动 / 端口被占 |
| GPU 没用上 | `nvidia-smi` / `rocm-smi` 检查驱动 |
| 中文生成质量差 | 升级模型 (qwen2.5:7b → 14b) 或回云端 |

## 7. 与云端切换

随时可在应用窗口设置面板切换:
- Endpoint = `http://127.0.0.1:11434/v1` → 本地
- Endpoint = `https://api.openai.com/v1` → OpenAI
- Endpoint = `https://api.deepseek.com/v1` → DeepSeek
- 付费用户: 设置面板锁定, gateway 自动选 (用户不持 raw key)

## 8. 不要做的事

- ❌ 别让 attune 自动启动 Ollama — Ollama 由用户管, 不抢用户进程
- ❌ 别让 attune 自动拉模型 — 用户决定下哪个 (网络 / 磁盘 / 偏好)
- ❌ 别把本地 LLM 作默认 — attune 主云端定位, 本地仅可选

---

参考: [Ollama 官方文档](https://ollama.com/docs), [模型库](https://ollama.com/library).
