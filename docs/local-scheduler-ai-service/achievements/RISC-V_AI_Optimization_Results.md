# RISC-V AI 推理优化成果索引

> 更新: 2026-04-19 | 平台: SpacemiT local scheduler

## 文档结构

按 **软件栈 × 指令集** 拆分为独立文档：

### 成果文档（已完成）

| 文档 | 软件栈 | 指令集 | 用途 |
|:-----|:-------|:-------|:-----|
| [ORT_RVV_Standard.md](ORT_RVV_Standard.md) | ORT | **标准 RVV** | 上游 PR |
| [ORT_IME_SpacemiT.md](ORT_IME_SpacemiT.md) | ORT | **IME** | 商业部署 |
| [GGML_RVV_Standard.md](GGML_RVV_Standard.md) | ggml | **标准 RVV** | 上游 PR |
| [GGML_IME_SpacemiT.md](GGML_IME_SpacemiT.md) | ggml | **IME** | 商业部署 |

### 路线图（待完成）

| 文档 | 内容 | 工作量 |
|:-----|:-----|:-------|
| [ORT_RVV_Roadmap.md](ORT_RVV_Roadmap.md) | ORT MLAS 16 个缺失 kernel，5 个 Phase | ~22 天 |
| [GGML_RVV_Roadmap.md](GGML_RVV_Roadmap.md) | ggml 12 个优化靶点，5 个 Phase | ~18 天 |

### 其他

| 文档 | 内容 |
|:-----|:-----|
| [SpacemiT_IME_Optimization_Results.md](SpacemiT_IME_Optimization_Results.md) | IME 详细技术（vmadot 验证、TOPS、CopyPackB） |
| [Benchmark_Architecture.md](Benchmark_Architecture.md) | 评测体系方法论 |

## 核心数据一览

### 标准 RVV（上游贡献）

| 场景 | 框架 | 加速比 | 状态 |
|:-----|:-----|:-------|:-----|
| FP32 Embedding/Reranker/OCR | ORT dispatch | **3.0-3.9x** | 4 patches ready |
| INT8 GEMM | ORT qgemm_kernel_rvv | **2.56x** vs 标量 | patch ready |
| FP16 whisper encoder | ggml simd_gemm+vec_dot | **1.83x** | patch ready |
| flash_attn RVV | ggml | ❌ **未做 (61% 热点)** | 最大未开发收益 |

### SpacemiT IME（商业产品）

| 场景 | 框架 | 加速比 | vs RVV |
|:-----|:-----|:-------|:-------|
| INT8 GEMM | ORT qgemm_kernel_ime | **3.78x** vs 标量 | IME 快 48% |
| Q8_0 whisper | ggml + vmadot | **3.83x** vs 标量 | encoder 4.75s |

### 四场景部署 (local scheduler :8090)

| 场景 | 框架 | 延迟 | 指令集 |
|:-----|:-----|:-----|:-------|
| Embedding | ORT | 505ms (Python) / 144ms (native) | FP32 dispatch |
| Reranker | ORT | 1032ms (Python) / 143ms (native) | FP32 dispatch |
| ASR | ggml | 5550ms | IME Q8_0 |
| OCR | ORT | 12500ms | FP32 dispatch |
