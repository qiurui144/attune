# attune 主仓测试大纲审计报告

> 审计对象:`docs/TESTING.md`(622 行)+ 实际测试代码
> 仓库状态:develop @ d21ae25(代码版本 **v1.1.0** ACP)
> 审计类型:只读分析 + 关键真跑抽验(不信大纲自报)
> 审计日期:2026-05-30
> 审计依据:全局 §6.1 测试方案规范 / §6.2 测试执行 SOP / §6.3 不轻易下结论

---

## 0. 执行摘要(TL;DR)

| 维度 | 结论 |
|------|------|
| §6.1 SSOT 6 要素 | **4 全 / 2 部分**(数据集来源 + 通过判据齐;v 历史 trace 停在 v0.6.3) |
| 大纲 vs 实际一致性 | **严重漂移** — 大纲停在 v0.6.1/v0.6.3 GA,实际已 v1.1.0;统计数字 630→实测 1499(core lib) |
| 6 类下限覆盖(代码层) | **8 类全有代码覆盖**,但大纲只显式登记 ~3 类(security 表 7 条) |
| ACP 测试入大纲 | **完全未入** — ACP-1~7 / governor / chat-flow wire 测试 0 处反映在 TESTING.md |
| 真跑抽验 | attune-core lib **1499 passed / 0 failed / 1 ignored**(4.22s);ACP wire 2+3 passed,**数字属实非空壳** |
| 死引用 | `docs/FEATURES.md` 缺失(被引 6+ 次);`scripts/run-final-eval.py` 缺失;`quality-eval` bin 缺失 |

**总体判定**:测试**代码**质量高、覆盖全、真跑全绿;但测试**大纲(TESTING.md)严重滞后**,
落后实际 ~5 个 minor + 整个 ACP capability,且存在多处死引用。大纲已不能作为可信 SSOT 使用。

---

## 1. §6.1 SSOT 6 要素逐项核对

| # | 要素 | 状态 | 证据 / 缺口 |
|---|------|------|------------|
| 1 | **测试目标(what+why)** | ✅ PASS | §0 顶部「产品级测试,可复现、可追溯、覆盖用户真实场景」+ 非目标 3 条 |
| 2 | **测试矩阵(场景×输入×期望)** | △ 部分 | 有 5 层金字塔表(§1.1)+ 能力↔层矩阵(§1.3)+ 安全表(§3.1 7 条)+ perf 表(§2.2);**但矩阵停在 F-01~F-18 18 能力 / v0.6.1,ACP / agent flow / governor 无矩阵行** |
| 3 | **视角划分(白/灰/黑盒)** | △ 部分 | 隐含在 Unit(白)/ Integration(灰)/ System+E2E(黑)分层,**但未显式用「白盒/灰盒/黑盒」措辞标注**(§6.1 要求显式视角划分) |
| 4 | **数据集来源** | ✅ PASS | §2.1 Tier 0/1/2 分级 + 语料清单(rust-book tag 1.75.0 / cs-notes c47a2a7 等 commit 锁定),版本固化纪律明确 |
| 5 | **通过判据(可量化)** | ✅ PASS | min_pass_rate 1.0/0.95;perf assert throughput>N(50% baseline);precision@K 降>5% 报警;Hit@10/MRR baseline 数字齐 |
| 6 | **v 历史 trace** | △ 部分 | 有 R15-R40 附录 D(2026-05-01,v0.7 时点)+ 成熟度路线 M1-M8;**但 trace 完全停在 v0.6.3 / v0.7,v0.8→v1.1.0 共 5+ minor + ACP 无任何 trace** |

**6 要素结论:4 PASS / 2 部分**(均因 v0.6→v1.1 漂移所致,非结构性缺失)。

---

## 2. 大纲 vs 实际一致性(核心 gap)

### 2.1 版本漂移(P0)

- TESTING.md 最新版本引用:**v0.6.0 / v0.6.1 / v0.6.3**(§1.2「v0.6.1 GA」「v0.6.3 baseline」)
- 实际代码版本:`rust/crates/attune-core/Cargo.toml = 1.1.0`
- **漂移跨度**:v0.7 → v0.8 → v0.9 → v1.0 GA → v1.0.x(11 minor)→ v1.1.0 ACP,大纲一个都没追

### 2.2 统计数字漂移(P0)

| 项 | TESTING.md 声称 | 实测(2026-05-30) | 差 |
|----|----------------|------------------|----|
| attune-core unit | 535(§1.2)/ 587(附录 R18-R20) | **1499 passed / 0 failed / 1 ignored**(`cargo test -p attune-core --lib`,4.22s) | +900~960 |
| 总测试(跨套件) | 630 passed / 3 ignored「跨 19 套件」(§1.2) | core tests/ 53 文件 + server tests/ 34 文件 = **87 集成套件**(非 19) | 严重低估 |
| `#[ignore]` 总数 | 3(§1.2)/ 6(附录 #3) | **77**(grep `#[ignore`,src+tests) | +71 |

> 注:#[ignore] 突增主要是 real-LLM gate / corpus / 慢测走 nightly 通道(合理),但大纲未更新计数 → 误导。

### 2.3 死引用 / 失效链接(P1)

| 引用位置 | 引用对象 | 实际 |
|---------|---------|------|
| §0 / §1.3 / 附录 C(6+ 处) | `docs/FEATURES.md`(声称是能力↔测试矩阵 SSOT) | **文件不存在**(find 全仓 0 命中)。TESTING.md 自称「反向组织」,但正向 SSOT 已消失 |
| §2.3.2 / §5.6 | `scripts/run-final-eval.py` | **不存在** |
| §2.3.3 / §5.6 | `cargo run --bin quality-eval` | **bin 不存在**(src/bin 仅 headless.rs) |
| §1.2 / §2.2 | `rust/tests/corpus_integration_test.rs` | 路径已变为 `tests/corpus_integration_test.rs`(文件在,路径标注过时) |

### 2.4 一致性结论

大纲说有的:FEATURES.md / run-final-eval.py / quality-eval **不存在**;数字全部低估。
实际有的:1499 core + ACP + 87 集成套件 + 77 ignore **大纲漏记**。**双向严重不一致**。

---

## 3. 6 类下限覆盖矩阵(§6.1 8 类)

抽查 attune-core / attune-server 测试代码(grep + 文件名),代码层覆盖 vs 大纲登记:

| 类别 | 代码层覆盖 | 证据文件 | 大纲(TESTING.md)登记 |
|------|-----------|---------|----------------------|
| happy path | ✅ | 全套件主路径 | ✅ 隐含 |
| edge case | ✅ | `golden/*/error/e01-empty-input.yaml`、boundary `#[test]` | ✅ §2.1 Tier 0 |
| error case | ✅ | golden `expected_error`、`oss_agent_real_llm_gate.rs` | △ 部分 |
| **adversarial**(SQLi/XSS/prompt-inj/path-traversal) | ✅ | `agent_gate_orchestrator.rs`、`pii_chat_path_redact_test.rs`、vault/search/chat src;§3.1 安全表 S-001~S-007 | △ 仅 7 条静态表,无 ACP/agent 注入 case |
| 多并发 | ✅ | `concurrent_stress_test.rs`、`vault::concurrent_*`(实测 passed) | ❌ 大纲未列(仅 R15 附录提一句) |
| 资源耗尽 | ✅ | `oom_behavior_test.rs`、`stress_large_scale_test.rs`、S-003 大文件 DoS | △ 仅 S-003 一条 |
| i18n | ✅ | `entities_test.rs`、`golden/*/e03-non-utf8-like.yaml`、中文 jieba 分词套件 | △ 数据源提中英混合,无独立 i18n 测试节 |
| 降级 | ✅ | MockLlm/MockEmbedding 遍布、`rag_quality_benchmark.rs` | △ §3 提 mock,无降级专节 |

**缺几类**:代码层 **8 类全覆盖**(0 缺);**大纲层只显式登记约 3 类**(happy/edge/security 表),
多并发 / 资源耗尽 / i18n / 降级 4 类在大纲中**未成节、未成矩阵行**,仅散落在附录/数据源描述。

→ **gap 是"大纲没把已有的测试组织进矩阵",不是"测试缺失"**。

---

## 4. ACP(v1.1.0)测试入大纲情况

实际存在的 ACP 相关测试(已编译可跑):

| 文件 | 真跑结果 |
|------|---------|
| `attune-server/tests/acp4_governor_wire_test.rs` | ✅ 2 passed / 0 failed(0.09s) |
| `attune-server/tests/acp5_chat_flow_wire_test.rs` | ✅ 3 passed / 0 failed(0.01s) |
| `attune-core/tests/governor_integration.rs` | 存在(CI 隔离注意,per CLAUDE.md 集成测试 mock) |
| `attune-core/tests/agent_gate_orchestrator.rs` | 存在 |
| `attune-core/src/governor.rs`、`agent_telemetry`、`agents/flow*`、`agents/scheduler*` 内联 | 计入 1499 core lib(全绿) |

**入大纲情况**:**0 处**。TESTING.md 全文 grep 无 "ACP / governor / cost / cot_budget / output_cap / flow_runner / agent flow" 任何测试矩阵行。
ACP-1~7 + chat-flow(任务描述 1499 core + 261 integration)这套 capability 的测试体系**完全游离在大纲之外**。

→ 违反 §3.1「新 capability 大纲同步」+ §6.1「测试矩阵随 feature 更新」。

---

## 5. 真跑抽验数据(§6.3 数据有源)

| 命令 | 结果 | 耗时 |
|------|------|------|
| `cargo test -p attune-core --lib` | **1499 passed; 0 failed; 1 ignored** | 4.22s |
| `cargo test -p attune-server --test acp4_governor_wire_test` | 2 passed; 0 failed | 0.09s |
| `cargo test -p attune-server --test acp5_chat_flow_wire_test` | 3 passed; 0 failed | 0.01s |

**结论**:大纲声称的 pass(虽数字过时)在**当前代码上属实** — 1499 是真实通过数,非 skip/空壳。
1 ignored = 极低,无"用 ignore 绕过失败"嫌疑。pass 率 100%(deterministic,符合 §7.2 Gate 2 ≥1.00)。
磁盘:审计后 `/data` available 233G(🟢),cargo test 产物未额外清(主 target 已存在)。

---

## 6. Gap 清单(P0/P1/P2)

### P0(大纲已失去 SSOT 可信度,发版前必修)
- **P0-1** 版本 trace 停在 v0.6.3,落后 v1.1.0 ~5 minor + 整个 ACP。§1.2 统计、§7 成熟度路线、附录 D 全过时。
- **P0-2** `docs/FEATURES.md` 缺失但被 TESTING.md 引为正向 SSOT(6+ 处死引用)。整个「能力↔测试」双向索引断裂。
- **P0-3** ACP(v1.1.0)测试体系 0 入大纲 —— 违反 §3.1 新 capability 同步。

### P1(失效引用 / 数字误导)
- **P1-1** `scripts/run-final-eval.py`、`quality-eval` bin 被 §2.3 / §5.6 引用但不存在 → 照大纲跑会失败(违反 §6.2 SOP「按大纲跑」)。
- **P1-2** §1.2 数字(535/630/3 ignore)全部过时,实测 1499/87 套件/77 ignore。
- **P1-3** 6 类下限中多并发/资源耗尽/i18n/降级 4 类有测试无矩阵行。

### P2(组织 / 措辞)
- **P2-1** §6.1 要求的「白盒/灰盒/黑盒」视角未显式措辞(目前隐含在层级里)。
- **P2-2** `corpus_integration_test.rs` 路径标注 `rust/tests/` 过时(实为 `tests/`)。
- **P2-3** CI 章节(§6)标「规划」,与实际 ci.yml(已含 windows matrix per 附录 #2)不一致。

---

## 7. 建议(补哪节 / 加哪类测试)

> 注:本审计只读登记,不改测试/大纲。以下为控制器决策参考。

1. **重写 §1.2 统计 + §7 成熟度路线**:以 v1.1.0 为基准,填实测 1499 core / 87 集成套件 / 77 ignore(注明 nightly 拆分),补 M9+(ACP)。
2. **决策 FEATURES.md**:要么重建(恢复能力↔测试 SSOT),要么把矩阵内联进 TESTING.md §1.3 并删除全部 FEATURES.md 引用。当前断链状态不可留。
3. **新增 §2.4 ACP / Agent Flow 质量门节**:登记 governor / cost cap / cot_budget / output_cap / chat-flow wire 测试矩阵(场景×输入×期望)+ real-LLM gate F1≥0.85 判据,引 ACP wire 测试文件。
4. **补 6 类下限独立节**:把已有的 concurrent_stress / oom_behavior / i18n(non-utf8 golden)/ degradation(mock)测试组织成 §3.x 矩阵行,显式标 8 类下限对照。
5. **修死引用(P1-1)**:删除或修正 run-final-eval.py / quality-eval 引用为实际存在的 harness(bench-orchestrator.sh 在,Python eval 不在)。
6. **加白盒/灰盒/黑盒显式视角列**(§1.1 表加一列),满足 §6.1 视角划分要求。
7. **代码层无需补测试** —— 8 类下限代码已全覆盖、1499 全绿;gap 纯在文档组织层。建议优先级:文档修复 > 加测试。

---

## 附:审计方法与限制

- 真跑仅覆盖 attune-core lib(deterministic,无 LLM)+ 2 个 ACP wire 集成测试(mock)。
- real-LLM gate(`oss_agent_real_llm_gate.rs` 等)未真跑(需 ollama,§1.3 未授权起 daemon)→ 其 F1 判据未独立验证,仅确认文件存在。
- integration 套件(87 文件)未全量跑(部分 #[ignore] 走 nightly,部分需 corpus 下载);抽样确认编译 + ACP 子集通过。
- 数字均有源:`cargo test` stdout(本会话真跑)+ `grep -c` + `ls | wc -l`。
