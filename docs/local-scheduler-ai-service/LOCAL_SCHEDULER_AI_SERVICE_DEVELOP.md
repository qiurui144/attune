# local scheduler AI 服务开发文档

## 架构

```
server.py (Flask :8090)
  ├── AIEngine
  │   ├── TokenizerPool     → simple_tokenizer.py (纯 Python WordPiece)
  │   ├── OrtSessionPool    → onnxruntime (系统 SpacemiT ORT 1.24.2)
  │   ├── embed()           → bge-base/small/gte ORT 推理
  │   ├── rerank()          → bge-reranker-base ORT 推理
  │   ├── transcribe()      → whisper-cli 子进程
  │   └── ocr()             → PPOCRv5 det+rec ORT + OpenCV
  └── Flask routes
      ├── /v1/embeddings
      ├── /v1/rerank
      ├── /v1/transcribe
      ├── /v1/ocr
      ├── /v1/models
      └── /health
```

## 本地开发

```bash
# 在 local scheduler 上直接修改
LOCAL_SCHEDULER_HOST=${LOCAL_SCHEDULER_HOST:-local-scheduler.local}
ssh root@$LOCAL_SCHEDULER_HOST
cd /root/ai-services
vim server.py

# 重启
systemctl restart local-scheduler-ai

# 查看日志
tail -f /tmp/ai-service.log
```

## 从宿主机部署

```bash
# 修改后部署
LOCAL_SCHEDULER_HOST=${LOCAL_SCHEDULER_HOST:-local-scheduler.local}
scp server.py simple_tokenizer.py root@$LOCAL_SCHEDULER_HOST:/root/ai-services/
ssh root@$LOCAL_SCHEDULER_HOST "systemctl restart local-scheduler-ai"
```

## 添加新模型

1. 将 ONNX 模型放入 `/root/rva23-bench/models/<model-name>/onnx/model.onnx`
2. 放入 tokenizer: `tokenizer.json` (从 HuggingFace 下载)
3. 在 `server.py` 的 `EMBEDDING_MODELS` 或 `RERANKER_MODELS` 中注册
4. 重启服务

## ORT 版本说明

服务使用 local scheduler 系统预装的 **SpacemiT ORT 1.24.2**（Python 包）：
- 优点：SpacemiT 优化过的 RISC-V kernel，无需额外配置
- 限制：不支持我们的 `MlasRiscvSetDispatch` API（FP32 dispatch 不生效）

bench_native 使用我们编译的 **ORT 1.23.2 + dispatch**：
- `libonnxruntime-v5.so.1.23.2`: RVV INT8 kernel + dispatch API
- `libonnxruntime-old.so.1.23.2`: IME INT8 kernel

两者性能对比 (bge-base FP32):
- Python ORT 1.24.2 (无 dispatch): ~505ms
- bench_native + dispatch: ~144ms (3.5x 更快)

## 双线构建

### ORT 交叉编译 (宿主机)

```bash
cd /home/qiurui/Documents/RV/rv-onnxruntime
# 全量编译
CLEAN_BUILD=1 bash scripts/build-rva23.sh
# 增量编译
cd src/onnxruntime/build/Linux/Release
cmake --build . --config Release -j24
# 部署
scp libonnxruntime.so.1.23.2 root@local scheduler:/opt/rvv-opt/ort-rva23/
```

### whisper.cpp 交叉编译 (宿主机)

```bash
cd /home/qiurui/Documents/RV/rv-whisper-cpp
bash scripts/build-rva23.sh
scp build/bin/whisper-cli* root@local scheduler:/opt/rvv-opt/whisper-cpp/bin/
```

## 上游补丁

`/home/qiurui/Documents/RV/rv-onnxruntime/patches/` 包含 4 个 upstream-ready 补丁：

| # | 补丁 | 目标 |
|:--|:-----|:-----|
| 1 | GCC 15 build fix | ORT upstream |
| 2 | RISC-V MLAS dispatch | ORT upstream |
| 3 | RVV INT8 GEMM kernel | ORT upstream |
| 4 | POSIX mmap/THP | ORT upstream |

纯标准 RVV，无 SpacemiT 私有指令，可提交上游 PR。
