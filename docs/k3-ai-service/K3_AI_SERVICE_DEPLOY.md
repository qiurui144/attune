# K3 AI 推理服务部署文档

> SpacemiT K3 (X100 8核 2.4GHz, VLEN=256, 16GB LPDDR5)
> IP: 192.168.100.209 | 用户: root | 密码: bianbu
> 更新: 2026-04-19

## 服务概述

K3 作为 AI 推理计算节点，提供四场景 HTTP API，供 attune/attune-enterprise 远程调用：

```
attune (x86/ARM)  ──HTTP──→  K3 :8080
                              ├── POST /v1/embeddings   文本向量化
                              ├── POST /v1/rerank       文档重排序
                              ├── POST /v1/transcribe   语音转文字
                              ├── POST /v1/ocr          图片文字识别
                              ├── GET  /v1/models       模型列表
                              └── GET  /health          健康检查
```

## 快速开始

### 服务管理

```bash
# 启动（IME 商业模式，默认）
ssh root@192.168.100.209 "systemctl start k3-ai"

# 启动（RVV 上游模式）
ssh root@192.168.100.209 "bash /root/ai-services/start.sh rvv"

# 停止
ssh root@192.168.100.209 "systemctl stop k3-ai"

# 状态
ssh root@192.168.100.209 "systemctl status k3-ai"

# 日志
ssh root@192.168.100.209 "tail -f /tmp/ai-service.log"

# 开机自启（已启用）
ssh root@192.168.100.209 "systemctl enable k3-ai"
```

## API 文档

### 1. 文本向量化 — `POST /v1/embeddings`

将文本转化为向量表示，用于语义搜索。

**请求**:
```json
{
  "input": ["文本1", "文本2"],
  "model": "bge-base"
}
```

**可用模型**: `bge-base` (768d, 推荐), `bge-small` (512d, 快速), `gte-base` (768d)

**响应**:
```json
{
  "object": "list",
  "data": [
    {"object": "embedding", "index": 0, "embedding": [0.042, -0.014, ...]},
    {"object": "embedding", "index": 1, "embedding": [0.031, 0.028, ...]}
  ],
  "model": "bge-base",
  "usage": {"prompt_tokens": 256, "total_tokens": 256},
  "latency_ms": 505.0
}
```

**性能**: bge-small 75ms/query, bge-base 505ms/query, gte-base 502ms/query

### 2. 文档重排序 — `POST /v1/rerank`

对检索到的文档按与 query 的相关性重新排序。

**请求**:
```json
{
  "query": "RISC-V AI推理优化",
  "documents": ["向量化指令加速", "天气预报", "SpacemiT K3处理器"],
  "model": "bge-reranker-base"
}
```

**响应**:
```json
{
  "results": [
    {"index": 2, "relevance_score": 0.928, "document": "SpacemiT K3处理器"},
    {"index": 0, "relevance_score": 0.933, "document": "向量化指令加速"},
    {"index": 1, "relevance_score": 0.643, "document": "天气预报"}
  ],
  "model": "bge-reranker-base",
  "latency_ms": 1032.0
}
```

**性能**: ~1030ms/3对 (稳定)

### 3. 语音转文字 — `POST /v1/transcribe`

中文/英文语音识别，支持 whisper 多种量化格式。

**请求 (文件路径)**:
```json
{
  "audio_path": "/path/to/audio.wav",
  "model": "whisper-small-q8"
}
```

**请求 (文件上传)**:
```bash
curl -X POST http://192.168.100.209:8080/v1/transcribe \
  -F "file=@audio.wav" -F "model=whisper-small-q8"
```

**可用模型**: `whisper-small-q8` (推荐, IME加速), `whisper-small-fp16`, `whisper-medium-q8`

**响应**:
```json
{
  "text": "这是语音转写的文字内容",
  "model": "whisper-small-q8",
  "mode": "ime",
  "latency_ms": 5560.0
}
```

**性能**: ~5.5s/段 (whisper-small Q8_0 IME)

### 4. 图片文字识别 — `POST /v1/ocr`

使用 PPOCRv5 检测和识别图片中的文字区域。

**请求 (文件路径)**:
```json
{
  "image_path": "/path/to/image.png"
}
```

**请求 (文件上传)**:
```bash
curl -X POST http://192.168.100.209:8080/v1/ocr -F "file=@document.png"
```

**响应**:
```json
{
  "text_regions": 5,
  "regions": ["[region 1: 30,80,340x30]", "..."],
  "det_latency_ms": 10500.0,
  "rec_latency_ms": 2000.0,
  "total_latency_ms": 12547.0,
  "image_size": "400x200"
}
```

### 5. 健康检查 — `GET /health`

```json
{"status": "ok", "mode": "ime", "threads": 8}
```

### 6. 模型列表 — `GET /v1/models`

```json
{
  "embedding": ["bge-base", "bge-small", "gte-base"],
  "reranker": ["bge-reranker-base"],
  "asr": ["whisper-small-q8", "whisper-small-fp16", "whisper-medium-q8"],
  "ocr": ["ppocrv5-server"]
}
```

## attune 对接示例

### Rust 端

```rust
use reqwest::Client;
use serde_json::json;

const K3_URL: &str = "http://192.168.100.209:8080";

// Embedding
let resp = client.post(format!("{K3_URL}/v1/embeddings"))
    .json(&json!({"input": texts, "model": "bge-base"}))
    .send().await?;

// Reranker
let resp = client.post(format!("{K3_URL}/v1/rerank"))
    .json(&json!({"query": query, "documents": docs}))
    .send().await?;
```

### Python 端

```python
import requests

K3 = "http://192.168.100.209:8080"

# Embedding
r = requests.post(f"{K3}/v1/embeddings",
    json={"input": ["文本"], "model": "bge-base"})
embeddings = r.json()["data"][0]["embedding"]  # 768d vector

# Reranker
r = requests.post(f"{K3}/v1/rerank",
    json={"query": "查询", "documents": ["文档1", "文档2"]})
results = r.json()["results"]  # sorted by relevance_score

# ASR
r = requests.post(f"{K3}/v1/transcribe",
    json={"audio_path": "/path/to/audio.wav"})
text = r.json()["text"]
```

## 性能基准

| 场景 | 模型 | 延迟 (P50) | 波动 |
|:-----|:-----|:----------|:-----|
| Embedding | bge-small (512d) | **145ms** | ±12ms |
| Embedding | bge-base (768d) | **505ms** | ±15ms |
| Reranker | bge-reranker-base | **1032ms** | ±8ms |
| ASR | whisper-small Q8_0 IME | **5550ms** | ±200ms |
| OCR | PPOCRv5 det+rec | **12500ms** | 视图片大小 |

20 轮稳定性测试：零失败。

## 双线策略

| 线路 | 指令集 | 用途 | 切换方式 |
|:-----|:-------|:-----|:---------|
| **IME** (默认) | SpacemiT vmadot | 商业产品极致性能 | `start.sh ime` |
| **RVV** | 标准 RISC-V V | 上游贡献 + 通用部署 | `start.sh rvv` |

IME 模式在 INT8 场景比 RVV 快 30-49%，FP32 场景两线一致。

## 文件结构

```
/root/ai-services/
├── server.py            # 主服务 (Flask)
├── simple_tokenizer.py  # 纯 Python BERT tokenizer
├── start.sh             # 管理脚本
└── k3-ai.service        # systemd unit

/opt/rvv-opt/ort-rva23/
├── libonnxruntime-v5.so.1.23.2   # 我们的 ORT (RVV INT8 + dispatch)
├── libonnxruntime-old.so.1.23.2  # 旧 ORT (IME INT8)
├── libmlas_riscv.so              # FP32 dispatch 库
├── libmlas_riscv_ime.so          # IME dispatch 库
└── bench_native                  # C API 性能测试工具

/root/rva23-bench/models/
├── bge-base-zh-v1.5/     # Embedding 768d
├── bge-small-zh-v1.5/    # Embedding 512d
├── gte-base/              # Embedding 768d
├── bge-reranker-base/     # Reranker
├── whisper-small-ggml/    # ASR (FP16 + Q8_0)
├── whisper-medium-ggml/   # ASR medium
└── ppocrv5/server/        # OCR det + rec
```

## 故障排查

```bash
# 服务未响应
systemctl restart k3-ai
tail -20 /tmp/ai-service.log

# 性能退化
cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor  # 必须是 performance
cat /sys/class/thermal/thermal_zone0/temp                  # >85000 需要降温

# 内存不足
free -h  # 服务运行需要 ~2GB

# 模型缺失
ls /root/rva23-bench/models/*/onnx/model.onnx
ls /root/rva23-bench/models/*/tokenizer.json
```
