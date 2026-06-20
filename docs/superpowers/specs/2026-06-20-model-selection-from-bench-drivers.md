# Spec: 本地模型选型由 vlm-llm-bench/drivers 驱动 + bench→attune SSOT 数据管道

> Status: DRAFT (2026-06-20) — 设计 only,不改代码,本机不跑任何模型/评测(§1.6)。
> 关联:`2026-06-11-modelstack-lifecycle.md`(S8 ModelStack failover)、`2026-06-10-k3-integration-gaps.md`(G1-G8)、memory `project_hw_accel_matrix_resource_station`。
> 实测源:`/data/company/project/vlm-llm-benchmark/`(bench harness + drivers + reports)、`/data/company/project/attune-k3/`。

---

## 0. 调研结论先行 — drivers 目录是什么

`vlm-llm-benchmark/drivers/` 是 symlink → `/mnt/hdd/vlm-llm-benchmark/drivers/`,内容是**按硬件 tier 分目录的厂商 NPU/AI 驱动 + SDK blob 包**(不是评测 harness,不是模型适配代码):

| 目录 | 内容(实测 `ls`) | tier 映射 |
|---|---|---|
| `amd-win/` | `NPU_RAI1.5_280_WHQL.zip`、`NPU_RAI1.6.1_314_WHQL.zip`、`ryzen-ai-lt-1.7.1.exe`(Lemonade) | AMD Ryzen AI (XDNA NPU) on Windows |
| `intel-win/` | (空,占位) | Intel Core Ultra (NPU/iGPU) on Windows |
| `rk182x-linux/` | `RK1820_RK1828_AI_SDK_V1.0.{0,4}.tgz`、`RKNN3_SDK/`(含 `rknn3_models`、`v1.0.4`)、Quick-Start/RELEASE PDF | Rockchip RK1820/RK1828 PCIe NPU on Linux |
| `rk3588-linux/` | (空,占位;RKNPU3 driver 走 in-tree) | Rockchip RK3588 RKNPU3 on Linux |

**关键认知**:drivers/ = **驱动/SDK 制品仓**,与 `models.yaml`(模型选型 SSOT)、`reports/*.en.md`(实测选型结论)三者**共同**构成 bench 仓的"硬件→驱动→模型→实测"全链。本 spec 的"基础数据来源于 drivers" 不是字面只指 driver zip,而是指 **整个 bench 仓(drivers + models.yaml + reports)= attune 选型与底座制品的上游 SSOT**。这一点用户 directive 的字面("drivers 内容")与实际仓结构存在 gap → 见 §11 风险 R1。

---

## 1. 目标定位

**解决的用户痛点**:attune 当前本地底座模型选型(embedding/rerank/ASR/OCR)是**代码里硬编码 + CLAUDE.md 文字表**,与真实硬件上的实测性价比脱节:

- `embedding.rs` 硬写 `Xenova/bge-m3`,不分硬件 tier;CLAUDE.md 写 RAM-tier 表(bge-m3/base/small)但代码没实现这套 tier 选择。
- `reranker.rs` 硬写 `Xenova/bge-reranker-base`(且注释记录 known-issue 来回切),无实测背书。
- `ocr/ppocr.rs` 单引擎 PP-OCRv5 mobile;bench 实测 AMD DirectML OCR 比 CPU 快 3.4×、Intel DirectML OCR **CER 202% 全废**(必须走 OpenVINO)—— attune 完全不知道。
- ASR(whisper.cpp)与 bench 选定的 `sensevoice-small`(CER 7.69% vs whisper)路线不一致。
- riscv K3 / RK3588 / RK1820 NPU tier 在 attune 选型里**完全缺席**。

**产品对齐**(attune 北极星 = 降 token + 数据安全 + 硬件感知底座):本地底座选型必须**由实测数据决定**(§6.3 数据有源),且能随 bench 仓持续校准**自动流入** attune,而不是每次手改代码。

---

## 2. 范围边界

**本 spec 做(v1)**:
- (A) 定义 attune 的**硬件 tier × 角色(embedding/rerank/ASR/OCR/VLM/LLM)选型表**,每格引 bench 实测源或标 PENDING-VERIFY。
- (B) 设计 **bench→attune SSOT 数据管道**:bench 仓产出 `models.yaml` + reports → 抽取为 attune 可消费的 **`model-catalog.yaml`(选型 manifest)** + **`driver-catalog.yaml`(驱动制品 manifest)**,经 company-mirror 分发,attune 启动时拉取/缓存并喂给 S8 selector + accel 选 EP。
- (C) 衔接 S8 ModelStack failover(模型权重下载)与 hw-accel 资源站(EP stack 下载)。
- (D) 落地切片(§12)。

**本 spec 不做(后续 vX)**:
- 不改 bench 仓的 harness/维度逻辑(bench 是上游,attune 是消费方;只读其产物)。
- 不实现 driver 自动安装(driver 制品分发先做 manifest + 链接 + 校验和;一键安装是 v.next,沿用 stack_installer 模式)。
- 不在 attune 内嵌评测;attune 永不自测模型性能,只消费 bench 结论(§1.6)。
- 不做 cloud LLM 选型(LLM 走网关 token,见网关 per-agent 模型策略 memory;本 spec 仅覆盖**本地底座** + K3/RK NPU 本地 LLM)。

---

## 3. 架构数据流

```
┌─────────────────────────── vlm-llm-benchmark 仓 (上游 SSOT) ───────────────────────────┐
│  drivers/<tier>/*            models.yaml                reports/*.en.md                  │
│  (厂商 driver/SDK blob)     (模型注册表+caps+role)     (实测 verdict + 指标 + raw 引用)  │
└───────────────┬─────────────────────┬──────────────────────────┬───────────────────────┘
                │ (CI/手动 export)     │                          │
                ▼                      ▼                          ▼
      ┌───────────────────────────────────────────────────────────────────┐
      │  bench-export 工具 (新, 在 bench 仓 scripts/)                        │
      │  扫 models.yaml + 解析 reports JSON evidence → 产出两份 manifest:    │
      │  • model-catalog.yaml  (tier × role → repo/file/dims/verdict/源指标) │
      │  • driver-catalog.yaml (tier → driver 包名/版本/sha256/下载 URL)    │
      │  signed (entitlement key 或 plugin anchor 同信任域) + schema_version │
      └────────────────────────────────┬──────────────────────────────────┘
                                        │ 发布到 company-mirror
                                        ▼
      ┌───────────────────────────────────────────────────────────────────┐
      │  company-mirror (models.engi-stack.com)                             │
      │  attune-catalog/model-catalog.yaml   (HF-layout resolve 兼容)       │
      │  attune-catalog/driver-catalog.yaml                                 │
      │  attune-ep-stacks/<stack>/...  (既有 EP stack 制品, 不变)          │
      └────────────────────────────────┬──────────────────────────────────┘
                                        │ S8 download_with_failover (既有原语)
                                        ▼
┌─────────────────────────────── attune client ─────────────────────────────────────────┐
│  infer/catalog.rs (新)  ──拉取+校验+缓存catalog──►  infer/model_source.rs (S8 selector)  │
│         │ 选型决策(tier × role)                            │ 选最优下载源 + failover      │
│         ▼                                                  ▼                              │
│  platform/accel.rs (detect tier) ──► EP hint ──► stack_installer (拉 EP stack)            │
│         │                                                                                 │
│         ▼ 选定 (repo,file,ep) 喂给:                                                       │
│  embedding.rs / reranker.rs / asr.rs / ocr/ppocr.rs  (改:读 catalog 而非硬编码)          │
└─────────────────────────────────────────────────────────────────────────────────────────┘
```

**catalog 缺失/拉取失败时**:attune 回退到**内置 baseline 选型**(编译进二进制的 `model-catalog.default.yaml`,= 本 spec §选型表当前列),保证离线/首发可用。catalog 是**优化覆盖层**,不是硬依赖(graceful degradation,§7)。

**新增本地文件**:
- attune 缓存:`~/.local/share/attune/catalog/{model-catalog,driver-catalog}.yaml` + `.sig` + `.etag`。
- attune 内置 baseline:`rust/crates/attune-core/assets/model-catalog.default.yaml`(`include_str!`)。

**无新增 DB 表**(catalog 是文件,不入 SQLite)。

---

## 4. 模块边界

| 模块/文件 | 角色 | 改动 |
|---|---|---|
| `attune-core/src/infer/catalog.rs` | **新** — catalog 拉取/校验/缓存/查询 API:`Catalog::resolve(tier, role) -> ModelChoice` | 新建 |
| `attune-core/assets/model-catalog.default.yaml` | **新** — 内置 baseline 选型(离线兜底) | 新建 |
| `attune-core/src/infer/model_source.rs` | S8 selector + company-mirror 源 | 加 `attune-catalog/*` repo 命名空间(复用 `resolve_sources_for` / `download_with_failover`) |
| `attune-core/src/infer/embedding.rs` | embedding provider | `qwen3_embedding_0_6b()` 改为 `Catalog::resolve(tier, Embedding)`(env override 仍最高优先) |
| `attune-core/src/infer/reranker.rs` | reranker provider | 同上 → `Catalog::resolve(tier, Rerank)` |
| `attune-core/src/asr.rs` | whisper.cpp ASR | 由 catalog 决定 whisper vs sensevoice + 模型档(small/medium/large-turbo) |
| `attune-core/src/ocr/ppocr.rs` | PP-OCR | 由 catalog 决定 EP(DirectML on AMD / OpenVINO on Intel / CPU 兜底) |
| `attune-core/src/platform/accel.rs` | tier 探测 | 加 `Tier` 派生(把 `AccelKind` + arch 映射到 catalog 的 tier key);加 riscv/RKNN tier |
| `attune-core/src/infer/stack_installer.rs` | EP stack 下载 | 不变(catalog 提供 ep hint,installer 拉对应 stack) |
| **bench 仓** `scripts/export_attune_catalog.py` | **新** — bench→catalog 导出器 | 在 bench 仓新建(attune 仓不含) |
| **cloud** company-mirror | catalog 托管 | 加 `attune-catalog/` 路径(cloud spec 配套 W,见 §10) |

**跨仓边界**:bench 仓产出 manifest(只读上游) → cloud company-mirror 托管 → attune client 消费。三者经 **HF-resolve URL 契约**(`{endpoint}/{repo}/resolve/main/{file}`)解耦,attune 不直连 bench 仓。

---

## 5. API 契约

### 5.1 `model-catalog.yaml` schema (v1)

```yaml
schema_version: 1
generated_at: "2026-06-20T..."          # bench export 时刻
harness_version: "<bench RELEASE 版本>"  # 来自 bench RELEASE.md, --compare 兼容性用
tiers:                                    # tier key 与 attune accel.rs Tier::id() 对齐
  amd-npu-win:                            # AMD Ryzen AI Windows
    embedding: { repo: "Xenova/qwen3-embedding-0.6b", file: "onnx/model_quantized.onnx", dims: 1024, ep: directml, verdict: PASS, source: "reports/2026-06-19-all-model-matrix-results.en.md:41", metric: "hit@1 1.0 p50 875ms" }
    rerank:    { repo: "Xenova/bge-reranker-base", file: "onnx/model_quantized.onnx", ep: cpu, verdict: PASS, source: "...:44", metric: "nDCG 1.0 p50 78ms" }
    ocr:       { engine: rapidocr, ep: directml, verdict: PASS, source: "...:32", metric: "CER 7.04% p50 468ms" }
    asr:       { engine: sensevoice, model: sensevoice-small, ep: directml, verdict: PASS, source: "...:36", metric: "CER 7.69% RTF 0.073" }
  intel-igpu-win:
    ocr:       { engine: rapidocr, ep: openvino, verdict: PASS, source: "...:34", metric: "CER 7.04%" }   # DirectML 在 Intel 全废
    ...
  riscv-k3:    { llm: { ... }, ... }      # K3 本地 LLM (bench reports/k3-riscv.en.md)
  rk1820-npu:  { llm: { ... }, asr: {...} }
  rk3588-rknpu: { embedding: {...} }
  cpu-fallback: { embedding: {repo: "Xenova/bge-m3", ...}, ... }   # = 内置 baseline
```

每个 role entry 必含 `verdict`(PASS/MEASURED/PENDING-VERIFY)+ `source`(reports 行号或 raw JSON 路径,§6.3)。`PENDING-VERIFY` 项 attune 仍可下发但 UI 标"未校准"。

### 5.2 `driver-catalog.yaml` schema (v1)

```yaml
schema_version: 1
tiers:
  amd-npu-win:
    - { name: "Ryzen AI 1.7.1 (Lemonade)", file: "ryzen-ai-lt-1.7.1.exe", sha256: "<PENDING>", url: "{mirror}/attune-drivers/amd-win/ryzen-ai-lt-1.7.1.exe", min_version: "1.6.1", note: "XDNA NPU LLM server" }
  rk1820-npu:
    - { name: "RKNN3 SDK", file: "RK1820_RK1828_AI_SDK_V1.0.4.tgz", sha256: "<PENDING>", url: "...", note: "rkllm3-server" }
```

attune **不自动装** driver(v1);UI Settings 展示"你的硬件需要 X 驱动 + 下载链接 + 校验和"(沿用 detector.py 一键命令心智)。

### 5.3 attune REST(扩展既有 `/api/v1/ai-stack`)

- `GET /api/v1/ai-stack/catalog` — 返回当前生效 catalog(已选 tier + 每 role 的 choice + verdict + source)。
- `POST /api/v1/ai-stack/catalog/refresh` — 显式触发重新拉取 catalog(**非请求路径同步阻塞**,R3)。

### 5.4 bench 导出器 CLI

```bash
# 在 bench 仓
python scripts/export_attune_catalog.py --out dist/attune-catalog/ --sign <key>
# 产出 model-catalog.yaml + driver-catalog.yaml + .sig；CI 上传到 company-mirror
```

---

## 6. 扩展点 / 插件接口

- **加新 tier**:bench 仓 `targets.yaml` 加 target + 跑实测 → reports 出 → 导出器自动产出新 tier block → attune accel.rs 加对应 `Tier` 派生即可消费(无需改 provider 代码)。
- **加新 role**(如未来 TTS / VLM-local):catalog schema 加 role key + provider 读 `Catalog::resolve(tier, NewRole)`。
- **加新下载源**:复用 S8 `builtin_sources()`,catalog 自身经同一 selector 下载。
- **driver 一键安装(v.next)**:driver-catalog 已带 sha256 + url,后续接 stack_installer 同款 `download_with_failover` + 平台特定安装钩子。

---

## 7. 错误 + 边界 case

| 场景 | 行为 |
|---|---|
| catalog 拉取失败(网络/源全死) | 回退内置 `model-catalog.default.yaml`;UI 提示"用内置选型,未更新";exit 不阻塞 |
| catalog 签名校验失败 | 拒绝该 catalog,回退内置 baseline,记 warn(信任链,不静默接受,对齐 entitlement 快照纪律) |
| catalog 含未知 tier/role | 跳过该 entry,该 role 回退 baseline;不 panic |
| tier 探测命中硬件但 catalog 该 tier 缺 role | 该 role 用 `cpu-fallback` tier 的选项 |
| verdict=PENDING-VERIFY 的模型 | 可下发,但 UI 标"未校准";LLM agent 路径若依赖则按 §4.5 兜底 |
| driver 缺失(NPU 无驱动) | accel.rs `driver_ready=false` → 该 tier 降级到 CPU EP,catalog 选 cpu-fallback;不报死 |
| Intel + DirectML OCR(已知全废 CER 202%) | catalog **禁止** Intel tier 选 directml OCR;强制 openvino(实测背书) |
| schema_version 不匹配 | 拒绝高于本 attune 支持的 schema;回退 baseline(向后兼容,§10) |

错误码 kebab:`catalog-fetch-failed` / `catalog-sig-invalid` / `catalog-schema-unsupported` / `tier-role-missing`。

---

## 8. 成本契约

- catalog 拉取 = 🆓 零成本(小 YAML,pre-flight/显式触发,非请求路径)。
- 模型权重下载 = ⚡ 本地算力/带宽(首次运行,既有 S8 行为,UI 显示进度)。
- driver 下载 = 用户显式触发(UI 给链接);本地底座推理 = ⚡;LLM 仍走 💰 网关 token。
- catalog 选型本身不触发任何 LLM 调用(纯查表)。

---

## 9. 测试矩阵

| 类型 | 用例(≥下限) |
|---|---|
| golden | 每 tier × role 的 `Catalog::resolve` 返回期望 repo/ep(从 default.yaml fixture) |
| 边界 | 空 catalog / 缺 tier / 缺 role / 未知 schema_version / 签名失败 → 全部回退 baseline |
| 错误 | 拉取超时 / 非 2xx / sha 不符 → failover 链 + 最终 baseline |
| 异常 | Intel+directml OCR 被拒;driver_ready=false 降级 |
| 集成 | mock company-mirror 服 catalog → 拉取→校验→resolve→provider 真起(local_onnx) |
| 回退 | 内置 default.yaml 与 §选型表一致性(快照测试,改表必同步) |
| 兼容 | 老 attune(无 catalog.rs)+ 新 catalog → env override 与硬编码路径仍工作 |

**bench 侧测试**:导出器 `export_attune_catalog.py` 加 pytest:对 fixture models.yaml + reports → 断言产出 catalog 的 verdict/source 字段非空、PENDING-VERIFY 正确标注、schema 合法。

---

## 10. 向后兼容

- catalog 是**新增覆盖层**,缺失时行为 = 当前硬编码(回退 baseline = 把现状 freeze 进 default.yaml)→ 老部署零影响。
- `ATTUNE_EMBEDDING_MODEL` 等 env override **保持最高优先级**(S8 既有契约),catalog 不夺权。
- schema_version 演进:attune 拒绝高于自身支持的 schema(不盲解析未知字段);bench 导出器 bump schema 时必同步 attune `catalog.rs` 支持范围 + RELEASE.md。
- company-mirror 加 `attune-catalog/` / `attune-drivers/` 路径 = cloud 侧新 W(non-breaking,新增路径)。
- migration:首版发布把当前 embedding.rs/reranker.rs 硬编码值原样写进 default.yaml,确保"切到 catalog"前后选型字节级一致,再逐 tier 用 bench 实测覆盖。

---

## 11. 风险登记

| # | 风险 | 缓解 |
|---|---|---|
| R1 | **directive 字面("基础数据来源于 drivers")与仓实际结构 gap**:drivers/ 只是 driver blob,真正的选型 SSOT 是 models.yaml+reports。 | 本 spec 把"基础数据源"定义为**整个 bench 仓产物**(driver + model + report),导出器统一抽取;需用户确认这一解读(§0)。 |
| R2 | **bench↔attune 漂移**:bench 改 models.yaml/reports 后 catalog 未重导,attune 用旧选型。 | catalog 带 `generated_at` + `harness_version`;CI 在 bench tag 时自动重导+发布;attune UI 显示 catalog 时间戳。 |
| R3 | **probe 进请求路径**(S8 既有红线)。 | catalog 拉取/选源同 S8 纪律:仅 pre-flight/显式触发,绝不同步阻塞请求路径。 |
| R4 | **signing 信任域**:catalog 用哪把 key 签? | 建议复用 entitlement signing key 或独立 catalog key(独立信任域 anchor,对齐 entitlement 快照纪律);**PENDING 用户拍板**。 |
| R5 | **driver 制品分发合规/体积**:厂商 driver(Ryzen AI exe、RKNN SDK tgz)版权与镜像分发授权。 | v1 只在 driver-catalog 放**官方下载 URL + sha256**,不镜像厂商二进制(避免再分发授权问题);仅 attune 自有 EP stack 镜像。**PENDING-VERIFY 合规**。 |
| R6 | **PENDING-VERIFY 项被当 PASS 用**。 | catalog verdict 字段强制;attune UI 区分;LLM 依赖路径走 §4.5。 |

---

## 12. 落地切片表

| 版本 | 主题 | 关键交付 | tag 位置 | blockedBy |
|---|---|---|---|---|
| v1.x.0 | catalog schema + 内置 baseline | `catalog.rs` 读取/校验/resolve + `model-catalog.default.yaml`(= 现状 freeze) + golden/边界测试 | develop | — |
| v1.x.1 | provider 接线 | embedding/reranker/asr/ocr 改读 catalog(env override 保留);accel.rs `Tier` 派生(含 riscv/RKNN) | develop | v1.x.0 |
| v1.x.2 | bench 导出器 | bench 仓 `export_attune_catalog.py` + pytest;首版产出 model/driver catalog(引实测) | (bench 仓) | v1.x.0 |
| v1.x.3 | company-mirror 分发 + S8 衔接 | catalog 经 company-mirror + S8 failover 下载;签名校验;`/ai-stack/catalog` REST | develop | v1.x.1, v1.x.2, cloud-W |
| v1.x.4 | UI + driver manifest | Settings 显示生效选型 + verdict + 硬件所需 driver 下载链接 | develop | v1.x.3 |

---

## 选型表(按硬件 tier × 角色 — 引 bench 实测)

> 来源:`vlm-llm-benchmark/reports/2026-06-19-all-model-matrix-results.en.md`(AMD/Intel Win 矩阵)、`reports/2026-06-18-local-model-limits.en.md`(LLM 上限)、`reports/k3-riscv.en.md`、`reports/rk3588.en.md`(2026-06-20 校准)。**verdict/指标行号见各 entry。**

### AMD Ryzen AI (Windows, XDNA NPU + RDNA iGPU/Vulkan)

| 角色 | 选型(bench 实测) | EP | verdict | 指标 | 源 |
|---|---|---|---|---|---|
| embedding | `qwen3-embedding-0.6b`(`bge-m3` 为多语言备选) | ollama/onnx | PASS | hit@1 1.0, p50 875ms | matrix:41-42 |
| rerank | `bge-reranker-base`(v2-m3 慢 3.7×) | local cross-encoder | PASS | nDCG 1.0, p50 78ms | matrix:44-45 |
| OCR | `rapidocr` **DirectML** | directml | PASS | CER 7.04%, p50 **468ms**(CPU 1592ms) | matrix:32 |
| ASR | `sensevoice-small` | local_onnx | PASS | CER 7.69%, RTF 0.073 | matrix:36 |
| LLM(本地) | `qwen2.5-7b`(质量) / `llama3.2-3b`(并发/长ctx) | ollama Vulkan | 上限 16k ctx | 7b TG 16 t/s; 3b 37.9 t/s saturate | limits:11,19 |

### Intel Core Ultra (Windows, NPU + Arc/Iris iGPU)

| 角色 | 选型(bench 实测) | EP | verdict | 指标 | 源 |
|---|---|---|---|---|---|
| embedding | `qwen3-embedding-0.6b` | ollama | PASS | hit@1 1.0, p50 617ms | matrix:52 |
| rerank | `bge-reranker-base` | local cross-encoder | PASS | nDCG 1.0, p50 148ms | matrix:54 |
| OCR | `rapidocr` **OpenVINO**(DirectML 全废) | openvino | PASS(DirectML FAIL) | OpenVINO CER 7.04%; **DirectML CER 202%** | matrix:33-34 |
| ASR | `sensevoice-small` | local_onnx | PASS | CER 7.69%, RTF 0.341 | matrix:37 |
| LLM(本地) | `qwen2.5-7b`(质量) / `llama3.2-1b`(并发) | ollama Vulkan(Arc) | 上限 16k ctx | 7b TG 9 t/s; 1b 32.5 t/s | limits:15,22 |

### NVIDIA (Linux/Win, CUDA) — PENDING-VERIFY

bench 有 `jetson-agx`(cuda)target 但 reports 无 dGPU CUDA 实测 → **PENDING-VERIFY**。baseline:embedding `bge-m3` / rerank `bge-reranker-base` / OCR `rapidocr` CUDA EP / ASR whisper.cpp CUDA build。需 bench 补 NVIDIA tier 实测。

### RISC-V K3 一体机 (SpacemiT K3, llama.cpp RVV)

| 角色 | 选型 | verdict | 指标 | 源 |
|---|---|---|---|---|
| LLM(本地) | `qwen2.5-0.5b` | PASS(partial) | TTFT p50 640ms, TPS 1.4 t/s, gsm8k 66% | k3-riscv:20 |

> 注:K3 throughput 极低(1.4 t/s),不适合交互/并发;底座 embedding/rerank/ASR/OCR 在 K3 一体机走 K3 推理服务(attune-k3 CLAUDE 推理统一收口 :8090),非本 catalog 的 ONNX 路径。

### Rockchip (RK3588 RKNPU3 + RK1820 PCIe NPU, Linux)

| 角色 | 选型 | 芯片 | verdict | 指标 | 源 |
|---|---|---|---|---|---|
| LLM(NPU) | `qwen3-vl-2b-rk1820` | RK1820 | PASS | TTFT p50 144.6ms, TPS 109 t/s, translation PASS | rk3588:40,64-73 |
| embedding(NPU) | `minicpm-embed-rk3588` | RK3588 RKNPU3 | PASS | hit@1 1.0, p50 143ms | rk3588:58,83-91 |
| ASR(NPU) | `rk-asr-rk1820` | RK1820 | MEASURED | RTF 0.040-0.066;CER PENDING-VERIFY | rk3588:41,75-81 |
| (Model Zoo) | qwen2.5-0.5b/3b, internvl3-2b, qwen2.5-vl-3b | RK1820 | PENDING-VERIFY | 官方 TPS 158.9/82.5/119.4/47.4 | rk3588:49-52 |

### CPU-fallback (任意无加速硬件 / 离线首发 = 内置 baseline)

| 角色 | 选型 | 源 |
|---|---|---|
| embedding | `Xenova/bge-m3`(现状) / 低 RAM `multilingual-e5-small/base` | attune embedding.rs:42-53 |
| rerank | `Xenova/bge-reranker-base`(现状) | attune reranker.rs:61 |
| OCR | PP-OCRv5 mobile(现状) / `rapidocr-cpu`(bench PASS CER 7.04%) | matrix:29-30 |
| ASR | whisper.cpp(现状) — **bench 实测 sensevoice-small CPU FAIL CER 23%** → CPU tier ASR 建议保留 whisper | matrix:35 |

---

## 对照现状 — 改哪些(精确 diff)

| 当前(attune 硬编码) | bench 实测建议 | 动作 |
|---|---|---|
| embedding 不分 tier,硬 `Xenova/bge-m3` | AMD/Intel `qwen3-embedding-0.6b`(实测 PASS,更轻),CPU 保留 bge-m3 | catalog tier-aware;v1 baseline 不变 |
| rerank 硬 `Xenova/bge-reranker-base`(known-issue 来回切) | `bge-reranker-base` 实测 PASS nDCG 1.0,确认为正解 | catalog 固化背书 |
| OCR 单 PP-OCRv5,**EP 不区分** | **AMD→DirectML(快3.4×)、Intel→OpenVINO(DirectML 全废 CER202%)** | catalog 强制 EP;最高价值修正 |
| ASR whisper.cpp(所有 tier) | AMD/Intel **sensevoice-small**(CER 7.69%);CPU 保留 whisper(sensevoice CPU FAIL) | catalog tier-aware ASR engine |
| K3/RK NPU tier 缺席 | K3 qwen2.5-0.5b;RK1820 qwen3-vl-2b/minicpm-embed/rk-asr | catalog 新 tier(K3 走 :8090 收口) |
| 选型在代码/CLAUDE 文字 | catalog manifest 由 bench 自动导出 | 建管道,去硬编码 |

> 最高价值单点:**Intel DirectML OCR 实测 CER 202% 完全不可用**——attune 当前无 EP 区分逻辑,若在 Intel 机走 DirectML 会 OCR 全错。catalog 强制 Intel→OpenVINO 直接堵死该 production 事故。
