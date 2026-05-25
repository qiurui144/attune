# W3 Batch C 设计稿（K2 Parse Golden Set Baseline）

**日期**：2026-04-27
**对应路线图**：12-week 战略 v4 Phase 1 W3 F-P0c
**依赖**：W3 batch A + B (commit `674cf55`)

---

## 1. 为什么这一批

per memory `reference_attune_open_source_landscape.md` K2：**Readwise 工程实践 — 200 篇真实页面 parsing benchmark + CI 回归 < 95% 准确率不准发版**。

attune 现有 golden set 测的是检索质量，但 chunker / parser 链路本身没有质量门槛。chunker 改坏了不会被现有测试发现。

W3 batch C = **K2 Parse Golden Set MVP**：5 篇 inline HTML fixture + manifest + CI 回归 harness。完整 200 篇语料 + GitHub Actions CI 留 W4。

## 2. 设计

### Corpus 结构

```
rust/crates/attune-core/tests/fixtures/parse_corpus/
├── manifest.yaml
├── 001-rust-ownership-doc.html       # rust-lang/book ch4
├── 002-china-civil-code.html         # 民法典节选
├── 003-tech-blog-post.html
├── 004-news-article.html
└── 005-academic-paper-section.html
```

每 fixture 是版本固定的真实 HTML 截取。manifest YAML 描述 expected：title 含 / 最小字符 / 必含短语 / 章节数 / 章节路径。

### Test harness

`parse_golden_set_regression.rs`：
1. 读 manifest
2. 每 fixture 跑 attune parser/chunker
3. 对照 expected 断言
4. 任一 fail → CI 红

### Regression gate

`MIN_PASS_RATE = 0.95`（per Readwise）。5 篇 baseline 必须全过，未来扩 200 时容许 5% 边界 fail。

## 3. 不做（推到 W4 + W5-6）

- ❌ 200 篇真实页面采集
- ❌ GitHub Actions CI 集成
- ❌ Per-language fixture 矩阵
- ❌ PDF parsing fixture
- ❌ Readability.js content extraction（阻塞 G3）

## 4. 致谢

per `ACKNOWLEDGMENTS.md`:
- Readwise Reader engineering blog: 200 篇 + CI 95% 阈值方法论
- Fixture content: rust-lang/book (MIT/Apache-2.0)、民法典（公开）等

## 5. 验收

- [ ] manifest.yaml 含 5 fixtures
- [ ] `tests/fixtures/parse_corpus/*.html` 5 文件
- [ ] `parse_golden_set_regression.rs` 5 tests
- [ ] 全过 → MIN_PASS_RATE 100%
- [ ] docs/TESTING.md K2 章节
- [ ] R1 review + ACKNOWLEDGMENTS + commit
