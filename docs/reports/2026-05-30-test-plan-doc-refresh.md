# 测试大纲文档债清理报告（attune + attune-pro）

> 日期：2026-05-30   分支：两仓均 develop   类型：纯文档（无新测试代码）
> 依据：`docs/reports/2026-05-30-attune-test-plan-audit.md` + `attune-pro/docs/reports/2026-05-30-attune-pro-test-plan-audit.md`
> 触发：全局 §6.1 测试方案规范 + §1.1.4 文档随 tag 同步

---

## 1. 真验统计数字（本机 cargo test 真跑，不照抄）

| 套件 | 真跑结果 | 命令 |
|------|---------|------|
| attune-core lib | **1499 passed / 0 failed / 1 ignored**（4.14s） | `cargo test -p attune-core --lib` |
| attune ACP-4 governor wire | 2 passed / 0 failed（0.09s） | `cargo test -p attune-server --test acp4_governor_wire_test` |
| attune ACP-5 chat-flow wire | 3 passed / 0 failed（0.02s） | `cargo test -p attune-server --test acp5_chat_flow_wire_test` |
| attune `#[ignore]` 总数 | **76**（src+tests grep） | `grep -rn '#[ignore' rust/crates` |
| attune 集成套件 | **87**（core 53 + server 34） | `ls tests/*.rs \| wc -l` |
| attune-pro law-pro lib | **336 passed / 0 failed / 0 ignored** | `cargo test -p law-pro --lib` |
| attune-pro law-pro golden gate | **28 passed / 0 failed / 3 ignored** | `cargo test -p law-pro --test agent_golden_gate` |
| attune-pro tech-pro lib | **27 passed / 0 failed** | `cargo test -p tech-pro --lib` |

历史大纲过时数字（attune 535/587/630/3 ignore；attune-pro「只有 22 tests」误读）全部作废，以本次真跑为准。

---

## 2. attune `docs/TESTING.md` 刷新项

大纲此前停在 v0.6.3，落后 v1.1.0（~5 minor + 整个 ACP）。本次改动：

1. **统计数字刷新**：§1.2 改为 v1.1.0 真跑（1499 core lib / 87 集成套件 / 76 ignore），标注「不照抄历史」。§5.1 / §7 同步。
2. **ACP（v1.1.0）入大纲**：新增 §2.4「ACP / Agent Flow 质量门」矩阵（Cost Governor / telemetry / registry/flow/feedback/scheduler + ACP-4/5 wire），含场景×输入×期望 + 真跑列 + F1≥0.85 判据。
3. **6 类下限矩阵补行**：新增 §2.5，把已存在的 concurrent_stress / oom_behavior / non-utf8 i18n / mock 降级 4 类组织成显式矩阵行（测试早已存在，只补文档行）。
4. **死引用修复**：
   - `scripts/run-final-eval.py`（不存在）→ 删除/改注，eval 逻辑已内聚进 `bench-orchestrator.sh`。§2.3.2 / §5.6 / §6 CI YAML 三处修。
   - `cargo run --bin quality-eval`（bin 不存在，仅 headless.rs）→ 改指向 `parse_golden_set_regression` + `bench-orchestrator.sh`。§2.3.3 / §5.6 修。
   - `docs/e2e-test-report.md`（不存在）→ §1.4 改为截图归 `docs/screenshots/<release>-verification/` + bug 入 RELEASE.md。
5. **FEATURES.md 断链处理**：经 git log 确认 FEATURES.md 已于 `86e3833` **主动合并进 README**（非误删）。按 §3.2「不重建已合并的双副本」原则，**不重建 FEATURES.md**，改为：① header 指向 README 作能力清单 SSOT；② §1.3 内联 18 条 F-nn 能力 ID 表（从 86e3833^ 提炼）；③ §4 命名规范、附录 C 关系节去除 FEATURES.md 外链。诚实判断：重建会逆转一次刻意的文档收敛。
6. **白/灰/黑盒视角**：§1.1 加显式视角划分表（满足 §6.1）。
7. **v 历史 trace**：§7.1 补 v0.6→v1.1.0 全版本测试增量；§7 成熟度路线 M1-M8 状态校正 + 新增 M9（ACP）。
8. **CI 章节**：§6 标注实装 `ci.yml`（已含 windows matrix）+ `rust-release.yml`，去除「规划」误导。

**结论**：代码层 8 类下限全覆盖、1499 全绿，gap 纯在文档组织层；本次无新增测试，仅修文档。

---

## 3. attune-pro `docs/TESTING.md` 创建（薄层聚合 index）

attune-pro 此前无仓库根级 TESTING.md（测试大纲只活在 `plugins/law-pro/tests/`，tech-pro 无对应段）。本次**创建薄层导航**（不重写 framework，避免 §3.2 双副本）：

- §0 header 指向已有 SSOT：agent-reliability-framework / methodology / law-pro TESTING / thresholds.yaml。
- §1 全景表：law-pro（生产）/ tech-pro（生产）/ patent·presales（scaffold exclude）+ 视角划分 + 语料/GT 来源（含 ENGINEERING_FIXTURE 上线门标注）。
- §2.2 **补 tech-pro 测试段**（此前无）：5 文件矩阵（unit/golden_gate/proptests/integration/scaffold）。
- §3 真跑命令汇总 + 本机真跑结果表。
- §4 新 plugin/agent 测试要求（仿 law-pro/tech-pro 闭环）。

---

## 4. 硬约束遵守

- 纯文档：attune 改 1 文件（TESTING.md），attune-pro 新建 1 文件（docs/TESTING.md）。无测试代码改动。
- 真验统计：所有数字 cargo test 真跑取数（§1）。
- 磁盘：`/` 33G（紧），cargo test 产物在 /data target/（复用既有缓存，未新增大量产物，无需 cargo clean）。未起 ollama（real-LLM lane 保持 ignored）。
- 测试运行时意外重写的 `plugins/law-pro/tests/perf-baseline/v0.9.5.json`（timestamp/p99 抖动）已 `git checkout` 还原，保证 commit doc-only。

---

## 5. Commit SHA

- attune develop：`ef239f7`（push d21ae25..ef239f7 → github.com/qiurui144/attune）
- attune-pro develop：`29703ae`（push 33adf74..29703ae → github.com/qiurui144/attune-pro）
