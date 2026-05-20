# W2 RAG Quality Batch 1 (J1 + J3 + J5 + B1 后端) 设计稿

**日期**：2026-04-27
**对应路线图**：12 周战略 v4 Phase 1 W2 F-P0b
**依赖**：H1 governor (commit `2bc558c`)、A1 memory (`71a714f`)
**被依赖**：J6 公开 benchmark (W4)、B1 前端高亮（下次会话）、J2 动态窗口 (W5-6)

---

## 1. 为什么这一批

W1 收尾基础（H1 governor + A1 memory）。W2 是第一波**用户感知 RAG 质量**推进 — 关掉"Demo 跑通"和"产品级好用"之间的差距。按吴师兄文章（References §A）四个最高杠杆点：

| 杠杆 | attune 现状 | 本批次 |
|------|-----------|-------|
| Chunk 路径前缀 | extract_sections 切分但**不附路径** | **J1** |
| 显式召回阈值 | RRF 融合，无 cosine cutoff | **J3** |
| 强约束 prompt + 置信度 | 温和"如果有信息回答"prompt | **J5** |
| Citation 含源坐标 | 引用无 offset | **B1 后端** |

前端（B1 高亮 UI、H2/H3 settings UI、D1 toggle）推迟 — 这些需要 Tauri + i18n 框架，是 W5 工作。

## 2. 模块逐项

### J1 — Chunk 面包屑路径前缀

**文件**：`rust/crates/attune-core/src/chunker.rs`

**当前**：`extract_sections(content: &str) -> Vec<(usize, String)>` 返回 `(section_idx, raw_section_text)`。

**新增**：`extract_sections_with_path(content: &str) -> Vec<SectionWithPath>`：

```rust
pub struct SectionWithPath {
    pub section_idx: usize,
    /// 文档根开始的标题层级，例如 ["标题", "第三章", "3.2 等待期"]
    /// 第一个标题前的内容路径为空 Vec
    pub path: Vec<String>,
    pub content: String,
}
```

原 `extract_sections` **保留** — 老调用方（测试代码等）不破坏。新调用方用 `extract_sections_with_path`。

**标题识别**：Markdown `#`/`##`/`###`（深度 = `#` 数）；代码 `def`/`class`/`fn`/`pub fn`/`impl` 视为同级（深度 1，因为代码结构很少深嵌套）。

**路径栈维护**：跟踪当前深度，push 前 pop 同深或更深条目。

**面包屑注入 chunk 文本**：调用方（如 indexer pipeline）以 `> ` 行前缀：

```
> 标题 > 第三章 > 3.2 等待期

[原始章节内容]
```

`>` 前缀走 Markdown blockquote，让 LLM prompt 读起来自然。模式来自吴师兄 §1。

### J3 — 显式 cosine 阈值

**文件**：`rust/crates/attune-core/src/search.rs`

**当前**：`SearchParams` 有 `top_k`、`vector_weight`、`fulltext_weight` 但**无 vector 结果的 min cosine score**。RRF 融合无条件。

**新增**：`SearchParams` 加 `min_score: Option<f32>`。默认 `Some(0.65)` — 吴师兄 0.65/0.72/0.78 曲线的保守端，平衡召回与精度。低于阈值的 vector 结果在 RRF **之前**过滤。BM25 结果不受影响（BM25 score 未归一化到 [0,1]，过滤需要单独标定）。

Settings 暴露 `cosine_threshold` 字段；默认 0.65；UI 控件在 W5+（本批次仅做后端）。

**快照测试**：插入 3 个 vector 结果，分数 [0.50, 0.70, 0.85]。阈值 0.65 → 期望 2 个（0.70 + 0.85）。0.78 → 1 个。0.55 → 3 个。

### J5 — 强约束 prompt + 置信度 + 二次检索

**文件**：`rust/crates/attune-core/src/chat.rs`

**当前**：`build_rag_system_prompt` 宽松："有知识就答，没知识不要编造"。无防编造规则、无置信度要求、无二次检索。

**新增** — 三子改：

#### J5.a 强约束 prompt（per 吴师兄 §4 + Self-RAG token 概念）

替换宽松开头为显式约束：

```text
你是用户的个人知识助手。请严格基于以下文档回答用户问题。

【硬性规则】
1. 只用文档中的信息，不要补充推理
2. 文档无明确答案 → 回复"知识库中暂无相关信息"
3. 禁用模糊措辞："可能" "大概" "建议咨询" "或许" "应该"
4. 引用必带来源：[文档标题 > 路径]
5. 回答末尾必须输出【置信度: N/5】（5=完全确定，1=高度不确定）

文档内容：
[1] 《标题》(来源: file, 路径: > A > B)
...
```

`> A > B` 路径来自 J1 面包屑。

#### J5.b 置信度解析

LLM 响应后用正则匹配 `【置信度: N/5】`（或英文 fallback `[Confidence: N/5]`）。缺失则默认 3（中性）。从用户最终看到的响应中剥离（作为 `confidence: u8` 字段返回 `ChatResponse`）。

#### J5.c 二次检索（per CRAG §3.2）

若 `confidence < 3`，ChatEngine 触发**一次**二次检索，`min_score` 降到 0.55（更广召回）。用扩大 context 重跑 LLM 一次。响应 mark `secondary_retrieval_used: true`。**硬上限一次重试** — 不进无限循环。

### B1 后端 — Citation char offset + 面包屑

**文件**：`rust/crates/attune-core/src/chat.rs` + `search.rs`

**当前**：`Citation { item_id, title, relevance }`。

**新增**：

```rust
pub struct Citation {
    pub item_id: String,
    pub title: String,
    pub relevance: f32,
    /// 字符级 offset 到源 item content（含 start，不含 end）
    /// web 搜索结果为 None（无源 item）
    pub chunk_offset_start: Option<usize>,
    pub chunk_offset_end: Option<usize>,
    /// 来自 J1 面包屑路径；无章节切分的源（如纯笔记）为空
    pub breadcrumb: Vec<String>,
}
```

`SearchResult` 已有 `item_id` 和 `chunk_idx`；扩展 `VectorMeta` 加 `(offset_start, offset_end)`，让 chat 不重新 tokenize 即可算 citations。

**前端不在本批次**：Reader 模态高亮 + 滚动到 offset 是单独的 Tauri/Preact PR。

## 3. 测试计划

per CLAUDE.md：确定输入、真 Store + tempfile、无 random。

### J1 单元（chunker.rs）
- Markdown 嵌套标题 → 路径 `[H1, H2, H3]` 正确
- 代码 `fn` 同级 → 全部路径长度 1
- 空内容 → 空 Vec
- 路径栈 dedent 时 pop：`# A\n## B\n# C` → C 的路径是 `[C]` 不是 `[A, B, C]`

### J3 单元（search.rs）
- min_score 0.65 过滤 [0.50, 0.70, 0.85] → 2 结果
- min_score 0.78 → 1 结果
- min_score None → 不过滤（向后兼容）

### J5 单元（chat.rs）
- prompt 含 "禁用模糊措辞"
- 置信度正则匹配 "【置信度: 4/5】" → 4
- 置信度正则匹配 "[Confidence: 2/5]" → 2（英文 fallback）
- 缺失置信度 → 默认 3
- mock LLM 返回置信度 2 → 二次检索触发
- mock LLM 返回置信度 4 → 不二次检索
- 二次检索失败（LLM error）→ 返回原响应，`secondary_retrieval_used = true` 但 `confidence_after = confidence_before`

### B1 后端单元（chat.rs）
- SearchResult 有已知 offset 时 Citation 有 Some(start) Some(end)
- offset 满足 `start < end <= content.len()`
- web 搜索结果有 None offset
- breadcrumb 从 VectorMeta path 透传

### 集成
单一集成测试：跑完整 chat 周期对接内存 store（3 文档、混合置信度 mock LLM），验证：
- 强约束 prompt 已发出
- 置信度被解析
- 周期内至少触发一次二次检索分支
- Citations 含 offset + 面包屑

## 4. 向后兼容

| 改动 | 风险 | 缓解 |
|------|------|------|
| `extract_sections` → 保留 + 加新函数 | 无 | 老函数不动 |
| `SearchParams.min_score: Option<f32>` | 默认 `Some(0.65)` 可能过滤掉之前能浮现的结果 | Settings UI 暴露；用户可设 None 恢复老行为；集成测试验证 golden recall 不掉 > 5% |
| `Citation` struct 加 3 新字段 | API 消费方（server routes）需重编译 | 全是 `Option` 或默认 `Vec`；serde 友好 |
| 强约束 prompt | 现有 chat 会话中途改 system prompt？ | system prompt 按消息发，不持久化；安全 |

## 5. 致谢（References）

per attune `ACKNOWLEDGMENTS.md` 政策。本批次来源：

- **吴师兄. 《鹅厂面试官追问：你的 RAG 能跑通 Demo？那让它在 5000 份文档里稳定答对，试试看》**, 公众号"吴师兄学大模型", 2026-04-27. https://mp.weixin.qq.com/s/YNcfSN0uv1c1LsLPzgB0jw — 整个 J 系列起源
- **CRAG paper**, Yan et al., 2024. arXiv:2401.15884 — J5.c 三分类门控 + 降阈值二次检索
- **Self-RAG paper**, Asai et al., 2023. arXiv:2310.11511 — J5.b 置信度作为 token 级信号（我们用 1-5 分简化版）
- **explodinggradients/ragas** (Apache-2.0). https://github.com/explodinggradients/ragas — J6（下批次）metric 命名约定

代码中每个新函数/struct 带 `// per <来源> §<节>` inline 注释，让后续维护者能追溯设计意图。

## 6. 验收清单

- [ ] J1: `cargo test -p attune-core chunker::tests::extract_sections_with_path*` 全绿
- [ ] J3: `cargo test -p attune-core search::tests::min_score*` 全绿
- [ ] J5: `cargo test -p attune-core chat::tests::strict_prompt*` + `confidence_parse*` + `secondary_retrieval*` 全绿
- [ ] B1 后端: `cargo test -p attune-core chat::tests::citation_offsets*` 全绿
- [ ] 集成: `cargo test -p attune-core --test rag_w2_batch1_integration` 全绿
- [ ] 全 lib 回归: `cargo test --workspace --lib` ≥ 之前 397 通过、0 失败
- [ ] `ACKNOWLEDGMENTS.md` J1/J3/J5/B1 条目（已在框架内，会精化）
- [ ] `rust/RELEASE.md` 改动条目带 cite
- [ ] `rust/DEVELOP.md` J 系列章节
- [ ] `tests/MANUAL_TEST_CHECKLIST.md` W2 batch 1 验证块
- [ ] git commit 含 `Inspired-by:` 行 + push develop

## 7. 不做（明确）

- ❌ B1 前端（Reader 模态滚动/高亮）— 下次会话
- ❌ H2 settings 档位 UI — 需 Tauri i18n 框架（W5）
- ❌ H3 顶栏 Pause 按钮前端 — 同上
- ❌ D1 No-telemetry toggle UI — 同上
- ❌ J6 公开 benchmark 数字 — W4，待 J1/J3/J5 稳定
- ❌ J2 动态窗口（返回 ±1 chunk）— W5-6（依赖 chunk_idx 邻接查询；要改 indexer）
- ❌ J4 query 意图 ML 路由 — W5-6
