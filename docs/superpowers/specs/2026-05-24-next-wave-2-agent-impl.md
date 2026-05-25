# Spec: next-wave 2 agent v1.1 impl

**文档状态**: GA — impl 已完成, 全套测试通过
**作者**: AI (Opus 4.7) - 本 spec 是 implementation closure 记录
**日期**: 2026-05-24
**仓库**: `attune` (OSS) + `attune-pro` (private)
**版本目标**: v1.1 (post-GA wave)

---

## 1. 目标定位

完成 #47 task 列出的两个 next-wave agent v1.1 实装:

1. **`document_classifier`** (OSS attune) — 现有 agent 加测试覆盖到 Agent 验证铁律 6 类下限
2. **`tech-pro code-reviewer`** (attune-pro) — 从 scaffold 进入 GA, 提供 diff 自动审查

---

## 2. 范围边界

### 做什么 (v1.1)

**document_classifier (OSS)**:
- 现有 impl (`rust/crates/attune-core/src/agents/document_classifier.rs`) 保留, 新增 golden + property + integration + error case 测试到 6 类下限
- 修复 reality-test-found bug: anchor-first 排序避免 borrowing 关键词压过 judgment

**tech-pro code-reviewer (attune-pro)**:
- Layer 0: diff parser (unified diff → Vec<DiffHunk>, ≤ 50k lines)
- Layer 1 (deterministic): pattern_checker - 8 builtin 规则
- Layer 2 (team-rule): YAML 加载 + deterministic regex 应用
- Merger + dedup + 排序 + summary
- subprocess binary protocol stub (留 v1.2 实装)

### 不做 (留 v1.2)

- LLM judgement 层 (trait scaffold 在位, 真实 attune-core::llm 接入留 v1.2)
- subprocess binary 入口
- GitHub PR webhook bot
- IDE LSP server

---

## 3. 架构数据流

### document_classifier (已有, 加 anchor-first 修复)

```
Vec<DocumentInput>
  ↓
classify_chunk_kind::classify (anchor-first ranking)  # ←  本次新增
  ↓
extract_entities::extract
  ↓
ClassificationOutput { classified, kind_summary }
  ↓
AgentOutput<ClassificationOutput>
```

**关键修复**: `KIND_ANCHORS` 高优先级标识表. 命中 anchor (如"判决书") 的 kind 必须胜出,
避免内嵌借款条款的判决书被误判为 borrowing_doc.

### tech-pro code-reviewer (新建)

```
CodeReviewInput { diff_text, language_hint, team_rules }
  ↓
diff_parser::parse_diff → Vec<DiffHunk> {file, lines, skip}
  ↓
pattern_checker::check_hunks (builtin rules per language)
  ↓
team_rules::apply_team_rules (user YAML regex + require_also_present)
  ↓
merger (dedup by (file, line, rule_id) + sort by severity)
  ↓
CodeReviewOutput { findings, summary, confidence=1.0, cost=0 }
```

---

## 4. 模块边界

### attune (OSS) 新增

- `rust/crates/attune-core/tests/golden/document_classifier/` — 11 golden + 3 error YAML
- `rust/crates/attune-core/tests/document_classifier_agent_golden_gate.rs` — golden runner
- `rust/crates/attune-core/tests/document_classifier_agent_proptests.rs` — 3 property tests
- `rust/crates/attune-core/tests/document_classifier_agent_integration.rs` — 2 E2E
- `rust/crates/attune-core/src/skills/classify_chunk_kind.rs` — bug fix (anchor-first)

### attune-pro 新增

- `plugins/tech-pro/src/code_reviewer/{mod,types,diff_parser,pattern_checker,team_rules}.rs`
- `plugins/tech-pro/tests/code_reviewer_{golden_gate,proptests,integration}.rs`
- `plugins/tech-pro/tests/golden/01..11-*.yaml` + `error/e01..e03-*.yaml`
- `plugins/tech-pro/Cargo.toml` — 加 `regex` + dev `proptest`
- `plugins/tech-pro/plugin.yaml` — `status: alpha`, `agents: [code-reviewer]`
- `plugins/tech-pro/README.md` — 全文重写

---

## 5. API 契约

### document_classifier (无变化)

```rust
pub fn run(inputs: &[DocumentInput<'_>]) -> AgentOutput<ClassificationOutput>;
```

### tech-pro code-reviewer

```rust
pub fn review(input: &CodeReviewInput) -> CodeReviewOutput;
```

types per `attune-pro/docs/superpowers/specs/2026-05-21-tech-pro-code-reviewer-agent.md` §5.4.

---

## 6. 扩展点 / 插件接口

### document_classifier

- 新 ChunkKind 加 `KIND_KEYWORDS` 行 + 可选 `KIND_ANCHORS` 行 (anchor 为强信号)
- LLM 二次分类作为 followup 留 v1.2 (per agent impl 已留 `low_confidence_count` followup)

### tech-pro code-reviewer

- 新 builtin 规则: 在 `pattern_checker.rs` 加 `check_<lang>(hunk, &mut out)` 函数
- 新语言: 加 match 分支到 `check_hunks`
- LLM provider: 复用 attune_core::llm::LlmProvider trait (留 v1.2 接入点)

---

## 7. 错误处理 + 边界 case

| 场景 | document_classifier | code-reviewer |
|------|--------------------|----|
| 空输入 | `classified=[], confidence=0.0` | `findings=[], confidence=1.0` |
| 文本为空 | `kind=other, conf<0.3` | hunk skip, no findings |
| 异常文本 (数字/标点) | `kind=other, conf<0.3` | hunk skip if not parseable |
| 超大输入 | n/a (chunked upstream) | exit 1: `diff-too-large` if >50k lines |
| Invalid team YAML | n/a | regex compile fail → silent skip rule |

---

## 8. 成本契约

Both agents 都在 🆓 **零成本** 层 (零 LLM 调用). UI / chat 不需要显式触发 — 自动跑 OK.

LLM judgement layer (留 v1.2) 走 ⚡ 本地算力 或 💰 时间金钱 — 用户显式触发.

---

## 9. 测试矩阵

### document_classifier

| 类别 | 实测 | 下限 |
|------|------|------|
| Golden (`tests/golden/document_classifier/`) | 11 + 1 sentinel | ≥ 10 + 1 |
| Property | 3 invariants | ≥ 3 |
| Boundary `#[test]` | 6 in `document_classifier.rs` mod + 10 in `classify_chunk_kind.rs` mod | ≥ 5 |
| Error (`golden/document_classifier/error/`) | 3 | ≥ 3 |
| Integration E2E | 2 | ≥ 1 |
| Regression | `11-sentinel-regression.yaml` | ≥ 1 |

### code-reviewer

| 类别 | 实测 | 下限 |
|------|------|------|
| Golden (`tests/golden/`) | 11 + 1 sentinel | ≥ 10 + 1 |
| Property | 3 invariants | ≥ 3 |
| Boundary `#[test]` | 6 in `boundary_tests` mod + 9 in submodule mods | ≥ 5 |
| Error (`golden/error/`) | 3 | ≥ 3 |
| Integration E2E | 2 | ≥ 1 |
| Regression | `11-sentinel-mixed-multi-rule.yaml` | ≥ 1 |

---

## 10. 向后兼容

- `document_classifier`: 公开 API 无变化, 仅内部排序逻辑加 anchor-first; 原有调用代码不受影响
- `code-reviewer`: 新 module, 不破任何现有 import; `tech-pro::placeholder` 保留 `#[deprecated]` 标记供 scaffold_sanity 兼容
- Schema: `CodeReviewOutput.schema_version = "1.0"`; 未来新字段用 `Option<T>` 形式扩展

---

## 11. 风险登记

| ID | 风险 | 概率 | 影响 | 缓解 |
|----|------|------|------|------|
| R1 | anchor-first 排序对未覆盖文档类型可能误降级 | 低 | 低 | golden case 11 sentinel + 现有 10 单元测试覆盖 |
| R2 | code-reviewer pattern false-positive (如 secret 检测误捕 env example) | 中 | 中 | placeholder 关键词过滤 + golden case 09 验证 |
| R3 | team rule regex ReDoS | 低 | 中 | 用 `regex` crate (已带 size limit); 失败的 rule 静默跳过 |
| R4 | 用户 diff 含中文/Unicode 文件名时 parse 失败 | 低 | 低 | 已用 `str::lines()` 标准 split, 不依赖字节边界 |
| R5 | LLM judgement layer 推迟到 v1.2 — 用户期望可能落空 | 中 | 低 | README + plugin.yaml 明确标 "v1.2+" |

---

## 12. 实施结果

### document_classifier (OSS)

- 19 个相关测试全过 (10 unit + 6 agent unit + 11 golden + 3 error + 3 prop + 2 e2e - 16 重复)
- 实测发现并修复 1 个 reality bug (judgment 误判为 borrowing_doc)
- `agent_golden_gate` 等价 harness: `document_classifier_agent_golden_gate.rs` 1.00 pass rate

### tech-pro code-reviewer (attune-pro)

- **37 个测试全过**: 27 lib + 2 golden + 2 integration + 3 prop + 3 scaffold
- 8 builtin 规则全覆盖 (each rule ≥1 trigger + ≥1 non-trigger case)
- 实测发现并修复 1 个 reality bug (secret pattern 不识别 env-style `KEY=value`)
- Build: `cargo test -p tech-pro` 全过 + clippy clean (待最后跑)

### 已 commit + push

- attune `develop` — document_classifier 测试套件 + classify_chunk_kind anchor 修复
- attune-pro `develop` — tech-pro code_reviewer 实装 + plugin.yaml `status: alpha`
