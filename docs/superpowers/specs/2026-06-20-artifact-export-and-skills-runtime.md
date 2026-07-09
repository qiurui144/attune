# Spec: Artifact Export Engine + Skills Orchestration Runtime

> Date: 2026-06-20 · Status: DRAFT (G1 pending) · Author: capability-layer design agent
> Sprint task: #119 (CAP: 交付物导出引擎 + skills 编排层)
> Supersedes nothing. Builds on: `2026-06-06-oss-document-intelligence.md`,
> `2026-06-19-writing-engine.md`, existing `attune-core::workflow` engine.

This spec turns attune from a **knowledge tool** (read / retrieve / extract / annotate /
summarize / write) into a **deliverable generator**: the user runs one *skill* over their
knowledge base and gets back a **downloadable professional artifact** — a spreadsheet, a
Word document, or a PDF — that is accurate, source-grounded, and cost-visible.

---

## 1. 目标定位 (Goal)

**用户痛点**:attune 现在能"想清楚"(summarize / compare / write / synthesize),但**产不出
能直接交付的成品**。用户要么手动把 chat 输出复制进 Word / Excel 再排版,要么根本拿不到结
构化下载物。北极星(**省时省力 + 数据安全**)要求:干完实事就产出可下载的专业交付物,不
让用户做"把 AI 输出搬进 Office 再手工排版"这种二次劳动。

**本能力做两件事**:

1. **导出引擎 (Export Engine)** — 把内存中的结构化结果(表格 / 文档结构 / 引用)**渲染并
   生成**为可下载文件:`xlsx` / `csv` / `md` / `docx` / `pdf`。现状是只能**读** docx/pdf/xlsx
   (`pdf-extract` / `calamine` / `zip`),**不能生成**。这是缺口 ①。
2. **Skills 编排运行时 (Skills Orchestration Runtime)** — 一个**声明式**的多步任务编排层,把
   已有 agent(`document_intelligence::compare/deep_summary`、`writing::draft/synthesis/outline`、
   `search`/RAG、`terminology`、`batch`)当作可复用**原语**串成端到端 skill:
   `RAG 检索 → agent 链 → 综合 → 渲染 → 导出 → 下载`。现状 `skills/`(SkillClaw 查询扩展)
   和 `skill_evolution`(失败信号→扩展词)**不是编排**,`workflow/`(已有 Skill/Deterministic
   step 引擎)是最接近的底座但缺 RAG / agent-chain / export step 类型与成本聚合。这是缺口 ②。

**对齐成本契约 (CLAUDE.md §成本感知与触发契约)**:skill 是 💰 多步任务 → **必须用户显式触
发,绝不后台偷跑**,执行前给**整链成本预估**;导出本身是 🆓 零/本地成本。

---

## 2. 范围边界 (Scope)

### 2.1 做什么 (IN — this slice)

**OSS attune-core(通用,任何领域个人用户都受益)**:
- `export/` 渲染 + 导出引擎:`xlsx`(rust_xlsxwriter)、`csv`(csv crate)、`md`、`docx`
  (docx-rs)、`pdf`(typst 渲染管线,见 §4 选型)。统一 `Artifact` 中间表示 → 各 backend。
- `skill_runtime/` 声明式编排运行时:**扩展现有 `workflow` schema**(不另起炉灶),新增
  `rag` / `agent` / `synthesize` / `render` / `export` step 类型 + 跨 step 成本聚合 + 显式
  触发 + 预估。
- **OSS 通用 skills**(4 个,作为示范 + 直接覆盖用户例子 3/4 的通用形态):
  - `kb-compare-table`(例子 3 通用形态):两文档/多文档参数比对 → xlsx/csv 表下载。
  - `kb-synthesis-doc`:多源 RAG → 综述 → docx/pdf 下载。
  - `outline-to-draft-doc`:大纲 + KB 材料 → 草稿 → docx/pdf 下载。
  - `kb-table-extract`:从一组文档抽取结构化字段 → 表格下载(复用 `batch` + extract)。
- 下载 HTTP 端点(`POST /api/v1/export` + `GET /api/v1/artifacts/{id}`)+ skill 执行端点
  (`POST /api/v1/skills/{id}/run`)+ Web UI **下载按钮** + **成本预估 chip**。

**pro attune-pro(行业 skills,组合 OSS 原语 + pro agents)**:
- 行业 skill 通过各 vertical 的 `plugin.yaml` 注册(复用现有 plugin 协议),skill YAML 声明
  里 `agent` step 可引用 pro agent(如 `target_hit_agent`、`tcm_knowledge_agent`),`export`
  step 复用 OSS 导出引擎。
- 用户例子 1(算法靶点+中医药综合方案)、2(评测体系框架)= **pro skills**(领域 agent 编排)。

### 2.2 不做 (OUT — v.next,写死防 scope creep)

- ❌ **可视化排版编辑器 / WYSIWYG**:导出是"一键生成成品",不是在线 Office 编辑。
- ❌ **图表/图片生成(chart→png 嵌 docx/pdf)**:本 slice 只做表格 + 文本结构;图表 v.next。
- ❌ **pptx / ods / rtf / epub** 等格式:本 slice 锁定 xlsx/csv/md/docx/pdf 五种。
- ❌ **模板上传自定义(用户传自己的 .docx 模板做 mail-merge)**:本 slice 用内置结构模板;
  "参考标书生成新标书"(例子 4)走 `writing::synthesis` 文本层 + 标准 docx 结构,**不做
  原 .docx 样式克隆**(v.next 才做 docx 样式继承)。
- ❌ **skill 市场化分发付费(pluginhub 上架第三方 OSS skill)**:本 slice skill 是内置 +
  pro plugin 注册;第三方 OSS skill marketplace 是 v.next。
- ❌ **导出文件的云端存储/分享链接**:artifact 落本地临时目录,下载即取,**不上云**(数据
  安全北极星)。TTL 后清理。

### 2.3 OSS / pro 边界(写死 — per `docs/oss-pro-strategy.md` v2 §4.3)

| 归属 | 判据 | 内容 |
|------|------|------|
| **OSS attune-core** | 对任何领域个人通用用户都有价值 | 导出引擎(全格式)、skill_runtime、4 通用 skills、下载端点 + UI |
| **pro attune-pro** | 行业绑定(靶点/中医/标书评分/评测体系) | 行业 skills(YAML in `plugins/<vertical>-pro/skills/`)+ 行业 agents |

**红线**:导出引擎与 skill_runtime **零行业逻辑**(无 case schema / 无领域 prompt);行业 skill
**只在 pro 仓**通过 plugin.yaml 注册,组合 OSS 原语。

---

## 3. 架构数据流 (Architecture & Data Flow)

### 3.1 Skill 编排数据流

```
                          POST /api/v1/skills/{id}/run   (user-triggered, 💰)
                                      │
                   ┌──────────────────┴──────────────────┐
                   │   skill_runtime::SkillRunner          │
                   │   (extends workflow::WorkflowRunner)  │
                   └──────────────────┬──────────────────┘
                                      │  step graph (DAG, var bindings)
   ┌──────────┬───────────┬──────────┼───────────┬────────────┬──────────┐
   ▼          ▼           ▼          ▼           ▼            ▼          ▼
 ┌────┐   ┌──────┐   ┌────────┐ ┌─────────┐ ┌──────────┐ ┌───────┐ ┌────────┐
 │rag │   │agent │   │synth-  │ │determin-│ │ render   │ │export │ │(pro)   │
 │step│──▶│step  │──▶│esize   │ │istic op │ │ step     │─▶│ step  │ │agent   │
 │    │   │(doc- │   │step    │ │(table/  │ │(Artifact │ │(xlsx/ │ │step    │
 │    │   │intel/│   │(writing│ │ merge/  │ │ build)   │ │ docx/ │ │via     │
 │    │   │write)│   │::synth)│ │ dedupe) │ │          │ │ pdf)  │ │plugin) │
 └────┘   └──────┘   └────────┘ └─────────┘ └──────────┘ └───┬───┘ └────────┘
   │          │           │          │           │          │
   └──────────┴───────────┴──────────┴───────────┴──────────┘
                          │  TokenBill aggregated across all LLM steps
                          ▼
                 ┌──────────────────────┐
                 │ SkillRunResult        │
                 │  artifact_id + bill   │   ──▶  GET /api/v1/artifacts/{id}  (🆓 download)
                 └──────────────────────┘
```

`rag step` = `search::search_with_context` over the vault (decrypt → chunks).
`agent step` = invoke an existing agent (compare / deep_summary / draft / outline / extract).
`synthesize step` = `writing::synthesize`. `render step` builds the typed `Artifact`.
`export step` = `export::render(artifact, format) → bytes → temp file → artifact_id`.

### 3.2 导出渲染数据流

```
  Artifact (typed IR — format-agnostic)
  ├── Table { columns: Vec<Column>, rows: Vec<Vec<Cell>>, title }
  ├── Document { blocks: Vec<Block> }   Block = Heading|Para|List|Table|PageBreak
  └── meta { source_grounding: Vec<GroundingRef>, generated_at, skill_id }
        │
        ├─ format=xlsx ─▶ rust_xlsxwriter ─▶ .xlsx bytes
        ├─ format=csv  ─▶ csv::Writer       ─▶ .csv  bytes
        ├─ format=md   ─▶ md serializer      ─▶ .md   bytes   (zero-dep, internal)
        ├─ format=docx ─▶ docx-rs            ─▶ .docx bytes
        └─ format=pdf  ─▶ Artifact→typst src ─▶ typst-as-lib ─▶ .pdf bytes (CJK font embedded)
```

The `Artifact` IR is the **single contract** between "what a skill produces" and "how it is
written to a file" — a skill never emits format-specific bytes; it emits an `Artifact`.

### 3.3 Skill 声明格式 (YAML — schema, extends `workflow::Workflow`)

Skill 用 **YAML 声明**(决策见 §"关键设计决策"):热插拔 + pluginhub 可分发 + pro 可注册。
现有 `workflow::WorkflowStep` 已是 `#[serde(tag="type")]` enum(Skill / Deterministic);本 slice
**新增 step 变体**,schema 向后兼容(老 workflow yaml 仍解析)。

```yaml
id: kb-compare-table
type: skill                       # was "workflow"; "skill" is the new top-level kind
version: "1.0.0"
trigger: { on: manual, scope: project }   # 💰 → always manual (never file_added)
cost_tier: llm_multi_step          # NEW: declares this is a paid multi-step skill
inputs:                            # NEW: typed user inputs (validated before run)
  - { name: doc_a, type: item_id, required: true }
  - { name: doc_b, type: item_id, required: true }
  - { name: fields, type: string_list, required: false }
steps:
  - type: rag                      # NEW
    id: load
    input: { item_ids: ["${doc_a}", "${doc_b}"] }
    output: docs
  - type: agent                    # NEW — references an existing OSS agent by capability id
    id: cmp
    agent: document_intelligence.compare
    input: { a: "${docs.0}", b: "${docs.1}", mode: structural, output_mode: marked }
    output: diff
  - type: render                   # NEW — build the Artifact IR
    id: build
    as: table                      # table | document
    input:
      title: "设备参数比对"
      from: "${diff.annotations}"  # mapping rules in render step impl
    output: artifact
  - type: export                   # NEW — terminal step; produces downloadable file
    id: out
    input: { artifact: "${artifact}", format: xlsx }
    output: file                   # → artifact_id
```

### 3.4 临时文件 / 下载

- 渲染产物落 `tempfile::TempDir`(per CLAUDE.md 跨平台规范,不硬编码 `/tmp`),路径**不可被
  用户控制**(artifact_id = server-generated uuid;**无路径穿越面**,见 §11)。
- `GET /api/v1/artifacts/{id}` 返回 `Content-Disposition: attachment; filename="..."` +
  正确 `Content-Type`。artifact TTL(默认 1h)后后台清理。**不落 vault、不上云**。

---

## 4. 模块边界 (Module Boundaries)

### 4.1 OSS attune-core

| 模块 | 文件 | 职责 |
|------|------|------|
| **export 引擎** | `attune-core/src/export/mod.rs` | `Artifact` IR + `Table`/`Document`/`Block`/`Cell` 类型 + `Format` enum + `render(&Artifact, Format) -> Result<Vec<u8>>` dispatcher |
| | `export/xlsx.rs` | `rust_xlsxwriter` backend |
| | `export/csv.rs` | `csv` backend |
| | `export/markdown.rs` | 零依赖 md serializer |
| | `export/docx.rs` | `docx-rs` backend(CJK 字体由系统/嵌入处理) |
| | `export/pdf.rs` | `Artifact → typst 源 → typst-as-lib` 渲染,**嵌入 CJK 字体** |
| | `export/fonts.rs` | 嵌入字体加载(Noto Sans/Serif CJK 子集,见 §11 风险) |
| **skill 运行时** | `attune-core/src/skill_runtime/mod.rs` | `Skill`(extends `workflow::Workflow`)+ `SkillRunner` + `SkillRunResult` + 新 step 变体 |
| | `skill_runtime/steps.rs` | `rag` / `agent` / `synthesize` / `render` / `export` step 执行器 |
| | `skill_runtime/registry.rs` | 内置 skill YAML 注册 + pro plugin skill 注册入口 |
| | `skill_runtime/cost.rs` | 跨 step `TokenBill` 聚合 + 预估(复用 `batch::BatchEstimate` 思路 + `cost.rs`) |
| **内置 skills** | `attune-core/assets/skills/*.yaml` | 4 通用 skill 声明 |
| **HTTP** | `attune-server/src/routes/export.rs` | `POST /api/v1/export` + `GET /api/v1/artifacts/{id}` |
| | `attune-server/src/routes/skills.rs`(扩展现有) | `POST /api/v1/skills/{id}/run` + `GET /api/v1/skills` + `POST /api/v1/skills/{id}/estimate` |
| **UI** | `attune-server/ui/src/...` | 下载按钮 + 成本预估 chip + skill 选择/运行视图(i18n 走 `t()`) |

**复用(不 fork)**:`search`(rag)、`document_intelligence::{compare,deep_summary}`、
`writing::{draft,outline,synthesize}`、`batch`、`terminology`、`document_intelligence::token_bill::TokenBill`、
`document_intelligence::model_routing`、`cost`、`usage`、`pii::llm_chat_redacted_hardened`、
`workflow::WorkflowRunner`(扩展)、`tempfile`、`async_fs`。

### 4.2 pro attune-pro

| 模块 | 位置 | 职责 |
|------|------|------|
| 行业 skill 声明 | `attune-pro/plugins/<vertical>-pro/skills/*.yaml` | 组合 OSS 原语 + pro agent step |
| 注册 | `plugins/<vertical>-pro/plugin.yaml`(新增 `registers_skills:` 节) | plugin loader 把 skill YAML 注入 `skill_runtime::registry` |
| 行业 agents | 已有 `plugins/<vertical>-pro/bin/*`(无新增 agent,本 slice 只编排) | 被 `agent` step 引用 |

**跨仓边界**:pro 不改 attune-core 代码;只通过 plugin.yaml + skill YAML 声明式接入。OSS 提供
`skill_runtime::registry::register_plugin_skills(plugin)` 扩展点。

---

## 5. API 契约 (API Contracts)

所有路径 kebab-case;错误返回 `{"error": msg, "code": kebab}`(`AppError`)。

### 5.1 导出 REST

```
POST /api/v1/export
  body: { "artifact": <Artifact JSON>, "format": "xlsx"|"csv"|"md"|"docx"|"pdf" }
  200:  { "artifact_id": "uuid", "filename": "...", "size_bytes": N, "format": "xlsx" }
  4xx:  { "error": "...", "code": "unsupported-format" | "artifact-too-large" | "render-failed" }

GET /api/v1/artifacts/{id}
  200:  binary, Content-Disposition: attachment; filename="..."; Content-Type: <mime>
  404:  { "error": "artifact not found or expired", "code": "artifact-not-found" }
```

### 5.2 Skill 执行 REST

```
GET  /api/v1/skills
  200:  [ { "id", "version", "title", "description", "inputs": [...], "cost_tier", "source": "oss"|"pro:<vertical>" } ]

POST /api/v1/skills/{id}/estimate          (🆓 — no LLM call; static estimate)
  body: { "inputs": { ... } }
  200:  { "est_tokens": N, "est_usd": 0.00X, "est_seconds": S, "steps": [ {id, tier} ] }

POST /api/v1/skills/{id}/run               (💰 — user-triggered; gated; emits artifact)
  body: { "inputs": { ... }, "confirm_cost": true }   # confirm_cost guards accidental spend
  200:  { "artifact_id", "filename", "format", "token_bill": <TokenBill>,
          "unverified_spans": [...], "warnings": [...] }
  402:  { "error": "member-gated skill", "code": "member-required" }   # if any pro/tier-3 step
  4xx:  { "error", "code": "cost-not-confirmed" | "input-invalid" | "skill-not-found"
                          | "partial-failure" }
```

### 5.3 Skill 声明 schema

见 §3.3。Rust 类型:`skill_runtime::Skill` extends `workflow::Workflow`(`kind: "skill"`),新
step 变体 `Rag` / `Agent` / `Synthesize` / `Render` / `Export`,加 `inputs: Vec<InputSpec>`、
`cost_tier: CostTier`。`parse_skill_yaml(yaml) -> Result<Skill, String>`(镜像 `parse_workflow_yaml`)。

### 5.4 Artifact IR(JSON,稳定契约)

```jsonc
{
  "kind": "table" | "document",
  "title": "...",
  "table": { "columns": [{"key","label","width?"}], "rows": [[{"text"|"num"|"bool": ...}]] },
  "document": { "blocks": [ {"heading": {"level": N, "text"}}, {"para": {"text","grounding?"}},
                            {"list": {"ordered": bool, "items": [...]}}, {"table": {...}},
                            {"page_break": {}} ] },
  "meta": { "skill_id", "generated_at", "schema_version": 1 }
}
```

---

## 6. 扩展点 / 插件接口 (Extension Points)

1. **加新 skill(声明式)**:写一个 YAML(OSS → `assets/skills/`;pro → `plugins/<v>/skills/` +
   `registers_skills:` in plugin.yaml)。**无需改 Rust**。`skill_runtime::registry` 启动时加载
   内置 + plugin loader 注入 pro。
2. **加新导出格式**:实现 `export::Backend` trait(`fn render(&Artifact) -> Result<Vec<u8>>` +
   `fn mime()` + `fn ext()`),在 `export::render` dispatcher + `Format` enum 注册一处。
3. **加新 step 类型**:在 `WorkflowStep` enum 加变体 + `steps.rs` 加执行器(向后兼容,additive)。
4. **pro 挂行业 skill**:plugin.yaml `registers_skills: [ {id, file, case_kinds?} ]` → loader 调
   `skill_runtime::registry::register_plugin_skills`。skill 的 `agent` step 引用 pro agent 的
   capability id(经 `capability_dispatch` 解析,member-gate 在 route 层强制)。

---

## 7. 错误 + 边界 case (Errors & Boundaries)

| 场景 | 行为(graceful) | code / exit |
|------|----------------|-------------|
| 不支持的导出格式 | 400 早失败 | `unsupported-format` |
| Artifact 过大(rows > 1e6 / blocks > 1e5 / bytes > 50MB) | 拒绝渲染,提示分批 | `artifact-too-large` |
| 渲染库 panic / 字体缺失 | catch → `render-failed`,**不 panic 进程**;pdf CJK 字体缺失走嵌入兜底 | `render-failed` |
| skill 某 LLM step 失败(retry×3 后) | **部分失败 graceful**:已完成 step 产物 + `warnings[]` 标失败 step;若 terminal export 仍可产出降级 artifact 则 200+warning,否则 `partial-failure` | `partial-failure` |
| skill cost 未确认 | `confirm_cost != true` → 400,要求先 estimate | `cost-not-confirmed` |
| input 校验失败(缺 required / 类型错) | 400 早失败,**不进 LLM step** | `input-invalid` |
| 非会员触发含 tier-3/pro step 的 skill | 402,route 层 member-gate(复用 `member_verifier`) | `member-required` |
| artifact 过期/不存在 | 404 | `artifact-not-found` |
| 注入源(skill 拉的 KB 源含注入指令) | 复用 `writing::source_has_injection_instruction` 预筛,拒绝该 step | `source-injection-detected` |

**成本契约执行**:`/run` 永远 user-triggered;skill `trigger.on` 强制为 `manual`(解析时若
声明 `file_added` 且 `cost_tier=llm_multi_step` → 拒绝注册,防"💰 skill 后台偷跑")。

---

## 8. 成本契约 (Cost Contract)

| 阶段 | 资源层 | 行为 |
|------|--------|------|
| `/skills/{id}/estimate` | 🆓 CPU | 静态预估(基于 input 规模 × 每 step tier × `cost.rs` 单价),零 LLM |
| `/skills/{id}/run` LLM steps | 💰 时间/金钱 | 每 step 的 `TokenBill` 聚合进 `SkillRunResult.token_bill`;UI 运行前显示 estimate chip,运行后显示实际账单 |
| `render` + `export` | 🆓/⚡ 本地 | 纯 CPU 渲染,毫秒~秒级,零云成本 |
| 上限护栏 | — | `cost.rs` 复用现有 per-call token cap;skill 级 `max_total_tokens`(默认 50K)硬上限,超则中止 + `partial-failure`(防 §11 编排成本爆炸) |

UI:skill 卡片常驻 `~预估 NK tok · $0.00X · ~Ns`(本地 step 显示 `本地`);`/run` 前必经
`confirm_cost`(显式按钮)。导出按钮标 `🆓 本地导出`。

---

## 9. 测试矩阵 (Test Matrix) — per §6.1 六类下限

### 9.1 导出准确性(round-trip — 最高价值)

每种格式 **生成 → 重新解析 → 内容相等** 验证(用现有 read 能力做 round-trip oracle):
| 格式 | 生成 | 重解析(oracle) | 断言 |
|------|------|----------------|------|
| xlsx | rust_xlsxwriter | `calamine` 读回 | 单元格值/类型/表头一致 |
| csv | csv::Writer | csv::Reader | 行列值一致 + CJK/逗号/换行转义正确 |
| docx | docx-rs | `zip` 解 + 解析 document.xml | 段落/标题/表格文本一致 |
| pdf | typst-as-lib | `pdf-extract` 抽文本 | 文本内容子串匹配 + **中文不乱码**(CJK 字体嵌入验证) |
| md | 内部 serializer | 反解析 | 表格/标题结构一致 |

### 9.2 六类下限(每 OSS skill + export 引擎)

| 类型 | 下限 | 内容 |
|------|------|------|
| happy | 4 通用 skill 各 ≥1 端到端 | 例子 3/4 通用形态真跑(mock LLM + real-LLM gate) |
| edge | ≥5 | 空表 / 单行 / 超宽列 / 1e5 行 / 纯 CJK / emoji / 嵌套 list |
| error | ≥3 | 不支持格式 / artifact 过大 / step 失败 partial / cost 未确认 / input 非法 |
| adversarial | ≥3 | csv 注入(`=cmd`/`@`/`+`/`-` 公式注入防护)、xlsx 公式注入、artifact_id 路径穿越、注入源 |
| concurrent | ≥1 | N 并发 export → artifact_id 不串、temp 隔离 |
| resource | ≥1 | 1e6 行拒绝、50MB 上限、TTL 清理 |
| i18n | ≥2 | 中英/繁简/RTL/emoji 在 5 格式里都正确(尤其 pdf CJK) |
| degrade | ≥1 | LLM 不可用 → render/export 仍可对已有结构产出(无 LLM 的纯结构 skill) |

### 9.3 LLM step real-LLM gate

含 agent/synthesize step 的 skill 走 §4.5 三 tier(deepseek-v4 / qwen-3.x / 强云)+ N=3,
F1/pass ≥ 0.85;低于 floor → RELEASE.md 标 model tier。

### 9.4 4 个用户例子作为验收 case(端到端)

每个例子 = 一个验收 e2e(skill 声明 → run → 下载文件 → 重解析校验内容)。详见附录。

---

## 10. 向后兼容 (Backward Compatibility)

- **不改任何现有 agent API**:`compare`/`deep_summary`/`draft`/`synthesize`/`outline`/`batch`
  签名不变;skill 通过 `agent` step **调用**它们,不修改它们。
- **workflow schema 向后兼容**:新 step 变体是 `#[serde(tag="type")]` enum 的 **additive** 增
  补;老 `workflow.yaml`(Skill/Deterministic)仍解析运行。`kind: "workflow"` 与 `kind: "skill"`
  并存(`skill` 是 superset)。
- **Artifact / Skill schema 加 `schema_version`**,additive 演进(per `WritingResult` 先例)。
- **新 crate 依赖**(rust_xlsxwriter / docx-rs / typst-as-lib / csv)全 **纯 Rust**,不破坏现有
  P0/P1 平台(Win/Linux x86_64)交叉编译;local scheduler riscv64 走镜像化路径同样纯 Rust 可编(见 §11)。
- 老 client 不调新端点 → 零影响;`/skills` 列表新增 `cost_tier`/`inputs` 字段 additive。

---

## 11. 风险登记 (Risk Register)

| # | 风险 | 严重度 | 缓解 |
|---|------|--------|------|
| R1 | **纯 Rust PDF 库的中文字体/排版**:printpdf/genpdf 无内置 CJK 字体 → 中文乱码/缺字 | 高 | **选 typst-as-lib**(typst 排版引擎为库):成熟 CJK 排版 + 自动断行。**嵌入 Noto Sans/Serif CJK 子集**到 binary(`export/fonts.rs`,`include_bytes!` 子集字体 ~5-10MB);测试硬验"pdf 抽回文本中文不乱码"。备选 genpdf 需手动加载系统字体(跨平台不可靠)→ 不选。 |
| R2 | **typst-as-lib 包体/编译重**:typst 依赖链大,拖慢 build + 增二进制 | 中 | 评估实测包体增量;若 > 30MB 或编译 > 现有 2× → 退 `docx-rs` 优先 + pdf 走"docx→系统 libreoffice headless"可选路径(但引入系统依赖,违背纯 Rust 原则)→ 优先坚持 typst,profile gate 不过才退。**G2 plan 阶段先做 spike 实测**。 |
| R3 | **skill 编排成本爆炸**:多 step × 大 KB → token 失控 | 高 | skill 级 `max_total_tokens` 硬上限(默认 50K)+ 每 step `cost.rs` cap + `/run` 强制 `confirm_cost` + estimate 前置 + 超限中止 `partial-failure`。 |
| R4 | **导出文件注入 / 路径穿越** | 高 | artifact_id = server uuid(用户不可控路径);temp 落 `TempDir`;**csv/xlsx 公式注入防护**(单元格首字符 `=`/`@`/`+`/`-` 前置 `'`,adversarial 测试覆盖);`GET /artifacts/{id}` 只查注册表不拼路径。 |
| R5 | **docx-rs 表格/复杂结构能力有限** | 中 | 本 slice 只用段落/标题/列表/简单表格(不做样式克隆,§2.2 已 OUT);能力边界写进模板。 |
| R6 | **部分 agent 失败导致半成品交付** | 中 | §7 partial-failure 契约:terminal export 产降级 artifact + warnings,UI 红标未完成 step;grounding 不足 span 走 `unverified_spans` 红警(复用 writing 先例)。 |
| R7 | **并发 export temp 冲突** | 低 | 每 run 独立 TempDir + uuid;concurrent 测试覆盖。 |
| R8 | **local scheduler riscv64 纯 Rust 库可编性** | 中 | typst/rust_xlsxwriter/docx-rs 纯 Rust;G2 plan 加 riscv64 交叉编译 spike(font include_bytes 同样可用)。 |

---

## 附录 A — 4 个用户例子的 skill 分解

| # | 用户例子 | skill 分解(step 链) | 用到的 agent | 导出 | OSS/pro |
|---|----------|---------------------|--------------|------|---------|
| **1** | 算法靶点命中方案 + 中医药知识 → 一套实操方案 | `rag`(靶点库)→ `agent: target_hit`(pro)→ `rag`(中医药库)→ `agent: tcm_knowledge`(pro)→ `synthesize`(OSS writing::synthesize,structure=plan)→ `render: document` → `export: docx/pdf` | pro `target_hit_agent` + `tcm_knowledge_agent` + **OSS** `writing::synthesize` | docx / pdf | **pro**(行业 agent 编排)— skill YAML in `plugins/medical-pro/skills/algo-tcm-plan.yaml`(跨 vertical 引用,medical 主挂) |
| **2** | 底层架构更新 → 全维度最新评测体系 → 确保技术先进性 | `rag`(多源技术文档)→ `agent: deep_summary`(OSS)×N → `agent: eval_framework`(pro,定义评测维度)→ `synthesize`(framework structure)→ `render: document`(含维度表)→ `export: docx/pdf` | OSS `deep_summary` + pro `eval_framework_agent` | docx / pdf | **pro**(评测体系=领域框架)— `plugins/tech-pro/skills/eval-framework.yaml` |
| **3** | 一个文档中两个设备参数不同 → 输出表格下载 | `rag`(单/双文档)→ `agent: compare`(OSS,mode=structural,output=marked)→ `render: table`(参数差异映射成行)→ `export: xlsx/csv` | **OSS** `document_intelligence::compare` | xlsx / csv | **OSS**(通用比对)— `assets/skills/kb-compare-table.yaml` |
| **4** | 参考标书 + 知识库设备参数 → 新标书 → 下载 doc/pdf | `rag`(参考标书 + 设备参数文档)→ `agent: outline_reverse`(OSS,从参考标书抽结构)→ `agent: outline_forward`/`draft`(OSS,套结构填新内容)→ `synthesize`(OSS,合材料)→ `render: document` → `export: docx/pdf` | **OSS** `writing::{outline,draft,synthesize}`(通用)+(可选 pro `bid_scoring_agent` 评分增强) | docx / pdf | **OSS 基础形态**(参考式生成是通用写作)+ pro 可选挂评分 — `assets/skills/reference-to-bid-doc.yaml`(OSS),pro 增强版 `plugins/presales-pro/skills/bid-doc-pro.yaml` |

**归属总结**:例子 3、4 的**核心通用形态进 OSS**(比对成表、参考式生成成文是任何领域都需要
的);例子 1、2 进 **pro**(靶点/中医/评测体系是领域 agent,无通用价值)。例子 4 OSS 提供基础
skill,pro 提供"标书评分增强"叠加版 — 体现 OSS 原语 + pro 增强的组合模型。

---

## 附录 B — 关键设计决策(结论 + 理由)

### B.1 导出库选型(纯 Rust,跨平台无系统依赖优先)

| 格式 | **选定库** | 理由 | 落选 |
|------|-----------|------|------|
| **xlsx** | **`rust_xlsxwriter`** | 纯 Rust、活跃维护、API 完整(类型/格式/公式)、无系统依赖、round-trip 可用 calamine 验 | `xlsxwriter`(C 绑定,跨平台风险);`umya-spreadsheet`(偏读) |
| **csv** | **`csv`** | 事实标准,纯 Rust,RFC4180 转义,CJK 安全 | — |
| **docx** | **`docx-rs`** | 纯 Rust,支持段落/标题/列表/表格,够本 slice 结构需求 | `docx`(停更);LibreOffice headless(系统依赖,违纯 Rust) |
| **pdf** | **`typst-as-lib`(typst 引擎为库)** | **CJK 排版成熟 + 自动断行 + 字体嵌入**,纯 Rust;解决 R1 中文乱码这一最大风险 | `printpdf`/`genpdf`(无内置 CJK,手动字体跨平台不稳);`weasyprint`/wkhtmltopdf(系统依赖) |
| **md** | 内部零依赖 serializer | 平凡,不引依赖 | — |

**CJK 字体**:`export/fonts.rs` `include_bytes!` 嵌入 Noto Sans/Serif CJK **子集**进 binary
(pdf 用),保证离线 + 跨平台一致渲染(R1 缓解)。瘦包影响:+5-10MB(可接受,远小于模型)。

### B.2 Skill 声明格式:**YAML(声明式)** — 不用纯 Rust trait

**结论:YAML 声明 + Rust 执行器混合**(声明用 YAML,step 执行器是 Rust)。
**理由**:(1)**热插拔 + pluginhub 可分发**(pro vertical 在 plugin.yaml `registers_skills:` 注
册,无需重编 attune-core);(2)**复用现有 `workflow` YAML 引擎**(已是 `serde(tag="type")` enum,
additive 扩展即可);(3)类型安全在 **step 执行器层**(Rust)保证,声明层校验在 `parse_skill_yaml`
+ `inputs` schema validation。纯 Rust trait 会牺牲热插拔与 pro 声明式注册 → 落选。

### B.3 编排底座:**扩展 `workflow` 而非新建** — 避开 SkillClaw 命名

`skills/`(SkillClaw 查询扩展)与 `skill_evolution`(失败信号→扩展词)**不是编排**,**不复用其
代码**。新模块命名 **`skill_runtime/`**(避免与 `skills/` 混淆),其 runner **扩展
`workflow::WorkflowRunner`**(已有 step graph + var binding + deterministic ops),新增 rag/agent/
synthesize/render/export step 类型。这样既不重造编排引擎,又不污染 SkillClaw 语义。

---

## G1 待评审项(spec → 用户批准 → writing-plans → plan 评审 → impl)

1. typst-as-lib 包体/编译实测(R2)— plan 阶段 spike 先行,不过则退路。
2. 4 通用 skill 的具体 input schema 与 render 映射规则细化(plan 任务级)。
3. pro `registers_skills:` 协议字段与 plugin loader 注入点(跨仓,需 attune-pro 同步)。
