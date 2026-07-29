# SpacemiT IME 私有指令集优化成果（商业产品线）

> 平台：SpacemiT local scheduler X100 (ime1_v2, xsmtvdotii)
> 工具链：SpacemiT GCC 15.2 (`-march=rv64gcv_xsmtvdotii`)
> 适用范围：**仅 SpacemiT K1/local scheduler 系列，不提交上游**
> 更新: 2026-04-19
>
> 本文档记录 SpacemiT 私有指令集（IME vmadot）的优化成果。
> 标准 RVV 优化见 [RISC-V_AI_Optimization_Results.md](RISC-V_AI_Optimization_Results.md)。
>
> **双线策略**：
> - 标准 RVV → 提交 ORT/llama.cpp 上游 PR
> - SpacemiT IME → 商业产品部署（local scheduler AI 服务 :8090）

---

## 一、IME 指令集验证

### 硬件验证（X100 用户态，无需 Spine 驱动）

| 指令 | 操作 | 每条计算量 | 吞吐 | 状态 |
|:-----|:-----|:---------|:-----|:-----|
| **vmadot** | int8 4×4×8 矩阵乘累加 | 128 MAC | 6.43 ns (15.4 cycles) | ✅ PASS |
| **vmadotu** | uint8 4×4×8 | 128 MAC | — | ✅ PASS |
| **vmadot1** | int8 滑窗（卷积优化） | 128 MAC | — | ✅ PASS |
| vfmadot | fp16 矩阵乘累加 | — | — | ❌ 工具链暂不支持 |

### 数据布局

```
vmadot v28, v0, v1

v0 (A): 行主序 A[m*8+k], m=0..3, k=0..7  → 32 bytes
v1 (B): 列主序 B[n*8+k], n=0..3, k=0..7  → 32 bytes
v28+v29 (C): 行主序 C[m*4+n], m=0..3, n=0..3 → 64 bytes (int32)

约束: vd 必须偶数寄存器 (v22/v24/v26/v28)
```

---

## 二、独立 GEMM 性能

| GEMM 尺寸 | vmadot 延迟 | 吞吐 | 正确性 |
|:----------|:-----------|:-----|:-------|
| 4×4×8 (1 tile) | 0.2 µs | 1.6 GOPS | ✅ CORRECT |
| 16×16×64 | 8.2 µs | 4.0 GOPS | ✅ CORRECT |
| 128×128×128 | 953 µs | 4.4 GOPS | ✅ CORRECT |
| 128×768×768 (Attention QKV) | 37 ms | 4.1 GOPS | ✅ CORRECT |
| 128×3072×768 (FFN up) | 149 ms | 4.1 GOPS | ✅ CORRECT |

vs 标量参考：**30x 加速**（独立 GEMM 级别）

---

## 三、ORT INT8 推理集成

### 16 列展开 vmadot kernel

```
内层循环:
  1. 加载 A: 4 行 × 8 K → 32 bytes (ld×4 = 4 条指令)
  2. 加载 B: 4 组 × 4 列 = 16 列的 B tiles → 4 × 32 bytes
  3. 4 条 vmadot: 共享 A, 不同 B → v28/v26/v24/v22
  4. 存储 4 个累加器 → 4 × 64 bytes

每 K 步: 4 条 vmadot = 4 × 128 = 512 MAC
A 打包成本分摊到 4 组 B 列 → 75% 减少
```

### CopyPackB 优化

B 矩阵在 MLAS CopyPackB 阶段直接打包成 vmadot 格式，**消除 kernel 内重排**：

```
标准 MLAS: B[col][AlignedK]  → 列连续存储
vmadot:    B[n_tile][k_tile][n_in*8+k_in]  → 4 列 × 8 K 块
```

### 端到端 INT8 结果

| 模型 | 旧 INT8 (1-col RVV) | **IME 16-col** | 加速比 |
|:-----|:-------------------|:--------------|:-------|
| bge-small-zh (33M) | 155ms | **73ms** | **2.12x** |
| bge-reranker-base (110M) | 946ms | **332ms** | **2.85x** |

选择方式：`MLAS_INT8_KERNEL=ime`

---

## 四、双线对比 (2026-04-19 新系统)

| 路径 | bge-reranker P50 | bge-small P50 | 指令集 | 硬件要求 |
|:-----|:----------------|:-------------|:-------|:---------|
| FP32 upstream (无优化) | 551ms | 82ms | — | 任意 |
| **FP32 RVV dispatch** | **143ms** | **27ms** | 标准 RVV | 任意 RISC-V |
| **INT8 IME vmadot** | **248ms** | **57ms** | xsmtvdotii | SpacemiT only |
| INT8 标准 RVV v3 | 366ms | 74ms | 标准 RVV | 任意 RISC-V |
| INT8 default (标量) | 936ms | ~80ms | — | 任意 |

### X100 Peak INT8 TOPS

| 指标 | 数值 |
|:-----|:-----|
| 单核 peak (4-way vmadot) | **490.65 GMACS = 0.49 TOPS** |
| 8核 peak | **3860 GMACS = 3.86 TOPS INT8** |
| 多核缩放 | 7.87x (近线性) |
| GEMM 实际效率 | 0.4% (数据搬运瓶颈) |
| A100 线性推算 | **26.2 TOPS INT4** (SpacemiT 宣称 60 TOPS) |

### 关键发现

1. **FP32 dispatch 是当前 X100 最优路径**（27ms/143ms），比所有 INT8 都快
2. **IME INT8 比标准 RVV INT8 快 30-49%**：vmadot 256 MACs/inst vs vwmulu 32 MACs/inst
3. **INT8 瓶颈在数据搬运**：PackB 占 27%，kernel 内 vwmulu widening chain IPC=0.76
4. **标准 RVV INT8 天花板**：缺少 vdot 指令，widening chain 限制 IPC，需要 IME 或上游 vdot 标准化

---

## 五、编译与部署

### 编译

```bash
# SpacemiT GCC（必须，标准 GCC 不识别 xsmtvdotii）
spacemit-toolchain-linux-glibc-x86_64-v1.2.2/bin/riscv64-unknown-linux-gnu-g++ \
    -O3 -march=rv64gcv_xsmtvdotii_zba_zbb_zbs -mcpu=spacemit-x100 \
    -DMLAS_TARGET_RISCV64 \
    -c qgemm_kernel_ime.cpp -o qgemm_kernel_ime.o
```

### 运行时选择

```bash
# 自动（IME > RVV > default）
LD_LIBRARY_PATH=/opt/rvv-opt/ort-rva23 bench_native model_int8.onnx --threads 8

# 强制 IME
MLAS_INT8_KERNEL=ime LD_LIBRARY_PATH=/opt/rvv-opt/ort-rva23 bench_native model_int8.onnx --threads 8

# 强制标准 RVV（对比用）
MLAS_INT8_KERNEL=rvv LD_LIBRARY_PATH=/opt/rvv-opt/ort-rva23 bench_native model_int8.onnx --threads 8
```

### 检测

```c
// 编译时
#ifdef __riscv_xsmtvdotii
// IME 指令可用
#endif

// 运行时（/proc/cpuinfo 不暴露 xsmtvdotii）
// 需要用 SIGILL 捕获或 SpacemiT 专有 API 检测
```

---

## 六、后续优化方向

| 方向 | 预期效果 | 复杂度 |
|:-----|:---------|:-------|
| A 矩阵 RVV strided load | 消除 memcpy，-10~15% | 中 |
| 32 列展开 (8 组 vmadot) | 进一步分摊 A 开销 | 低 |
| CopyPackA 直接生成 vmadot 格式 | 消除 kernel 内 A 打包 | 高 |
| vfmadot FP16 | 等 SpacemiT 工具链支持 | 依赖厂商 |
