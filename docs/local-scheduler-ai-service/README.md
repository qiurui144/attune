# local scheduler AI 推理服务

四场景 HTTP API，面向 attune/attune-enterprise 产品。

## 服务地址

`http://local-scheduler.local:8090`

| 接口 | 延迟 (P50) | 模型 |
|:-----|:----------|:-----|
| `POST /v1/embeddings` | bge-small **75ms**, bge-base **505ms** | 768d/512d 向量 |
| `POST /v1/rerank` | **1032ms** | bge-reranker-base |
| `POST /v1/transcribe` | **5550ms** | whisper-small Q8_0 IME |
| `POST /v1/ocr` | **12500ms** | PPOCRv5 det+rec |
| `GET /health` | <1ms | 健康检查 |

20 轮稳定性测试零失败，systemd 开机自启。

## 管理

```bash
systemctl start local-scheduler-ai     # 启动
systemctl stop local-scheduler-ai      # 停止
systemctl status local-scheduler-ai    # 状态
bash start.sh rvv         # 切换到 RVV 上游模式
bash start.sh ime         # 切换到 IME 商业模式
```

## 文档

- [部署文档](docs/LOCAL_SCHEDULER_AI_SERVICE_DEPLOY.md) — API 文档、对接示例、性能基准
- [开发文档](docs/LOCAL_SCHEDULER_AI_SERVICE_DEVELOP.md) — 架构、构建、双线策略

## 双线策略

- **IME 商业线**：SpacemiT vmadot 私有指令，INT8 比 RVV 快 30-49%
- **RVV 上游线**：纯标准 RVV，可提交 ORT/llama.cpp upstream PR
- FP32 dispatch 两线一致（27ms/144ms/143ms）

## 跨仓同步 — attune-local-scheduler → 主线 (#62, 2026-06-19)

`attune-local-scheduler`（独立 repo，local scheduler riscv64 一体机集成/部署栈）近期进展逐项分类。原则：**local scheduler 独有（riscv64 / SpacemiT IME / 设备 / 镜像部署 / 板级 config）留 attune-local-scheduler；对主线产品也有价值的（通用 bug fix / provider wiring / 逻辑改进）合入主线 develop**。

**重要前提**：`attune-local-scheduler` **不 fork 任何 attune-core 源码**（仅 scripts/config/deploy/docs/reports）。因此本轮**无可直接 cherry-pick 的代码改动**——共有价值体现为「主线源码状态确认」+「已识别的主线 G-class 缺口（需走主线 spec→impl→review→test）」。

### local scheduler 独有（留 attune-local-scheduler）

| 改进 | 为何 local scheduler 独有 |
|------|------|
| `provision-models.sh` reranker URL 修正（4578464） | 部署脚本；主线 `reranker.rs:49` v0.6 Phase B 默认**已正确**指向 BAAI full（非 Xenova quantized）。local-scheduler 脚本是把 provisioning 对齐主线源码，**主线无需改** |
| `sherpa-asr-bridge.sh`（whisper-cli 兼容桥 → SenseVoice） | 集成层桥；symlink `/usr/local/bin/whisper-cli`，零 attune-core 改动。riscv64/sherpa runtime 路径绑定 |
| `build-attune-riscv64.sh`（rv-gcc 15.2 交叉编译 + 3 桶 porting bridge：ort-sys load-dynamic / numkong march / linker startfile path） | riscv64 ISA + local scheduler sysroot 专属 |
| layout PicoDet / SenseVoice / diarization 模型 provisioning + .140 板部署记录 | 设备/镜像部署专属 |

### 主线 G-class 缺口（已识别，待主线 SDLC 周期实施）

attune-local-scheduler `docs/2026-06-18-followup-optimizations.md` 明确这两项是 **attune-core 上游 feature**（非 config/model add），应走主线正式 dev 周期，本轮不作 tail-of-session hack 直塞：

| 缺口 | 主线现状 | 实施路径（主线） |
|------|---------|------|
| **① 原生 sherpa-onnx ASR provider** | `attune-core/src/asr.rs` `AsrBackend` 是单一 whisper-cli 结构；ASR 增强当前经 local-scheduler 桥功能交付 | `AsrBackend` struct→enum（`WhisperCli` / `SherpaOnnx`），`transcribe_*` 按 backend 分派，`detect_asr_backend` 优先探测 sherpa；原生 diarization（`DiarizationBackend::SherpaOnnx`，替代 RISC-V 不可用的 WhisperX/pyannote torch）。**全 edition 受益**，非仅 local scheduler |
| **② SLANet 表格结构 ONNX 推理接线** | `attune-core/src/ocr/nontext/table_structure.rs` 已接 488 预处理、ORT session、structure logits 解码、字典覆盖与 HTML→cell 解析；model 缺失仍显式返回 `UnrecognizedV1{model-missing}` | local scheduler/桌面 release 环境需确认 SLANet 模型文件 provisioning、字典与所选 ONNX 变体一致，并补真实标注集结构 F1/准确率报告 |

> 同源 headless 落差（G6 audit，`attune-local-scheduler reports/2026-06-11_g6-headless-parity-audit.md`）：浏览器态文件拖拽 fallback（#2，纯前端）/ 文件夹路径输入（#1/#3）/ locked-mode 降级（#4，前置 G3）等属主线 headless Web UI backlog，同样走主线 SDLC，不在本同步轮内塞码。
