# Contributing to Attune

> [中文见下文 / Chinese below](#中文)

Thanks for your interest in contributing. This file is a short on-ramp; the
authoritative developer reference is **[DEVELOP.md](DEVELOP.md)** (architecture,
branch model, full build/test commands, release flow). This page does not
duplicate it — it links to it and states the contribution expectations.

## Dev setup

See **[DEVELOP.md → 环境搭建 / 编译命令汇总](DEVELOP.md)** for toolchain, build,
and test commands. In short:

- **Rust 商用线** (`rust/`): `cd rust && cargo build` / `cargo test --workspace`.
- **Python 原型线** (`python/`): venv + `pip install -e .[dev]` + `pytest`.
- **Chrome extension** (`extension/`): `npm install && npm run build`.

Third-party attribution lives in [NOTICE](NOTICE) / [ACKNOWLEDGMENTS.md](ACKNOWLEDGMENTS.md).

## Branch model (GitFlow-Lite)

Full table in [DEVELOP.md → 分支模型](DEVELOP.md). The essentials:

- Two long-lived branches: **`main`** (stable, tags only) and **`develop`** (integration).
- Do your work on a short-lived **`feature/<topic>`** branch off `develop`.
- Open a PR into **`develop`** (never directly into `main`). `develop → main` is
  release-only, via `--no-ff` merge + a `vX.Y.Z` tag on `main`.
- Delete the feature branch (remote + local) once merged.

## Commit conventions

- Conventional-commit style imperative subjects: `feat(scope): …`, `fix(scope): …`,
  `docs(scope): …`, `test(scope): …`, `chore(scope): …`, `refactor(scope): …`.
- **Atomic commits** — one logical change per commit; subject in the imperative
  mood, body explaining the *why* and any user-visible impact. Squash WIP noise
  before opening the PR.
- Subjects/bodies in Chinese or English (technical identifiers stay in English).

## Review & test expectations

Before requesting review / opening a PR:

1. **Tests pass**: `cargo test --workspace` (Rust) and/or `pytest` (Python) green
   for the code you touched. New behaviour ships with new tests — cover the
   happy path, edge cases, and error cases (see [docs/TESTING.md](docs/TESTING.md)).
2. **Lint clean**: `cargo clippy --workspace --all-targets -- -D warnings` (Rust),
   `ruff` (Python). No new warnings.
3. **i18n**: any user-visible UI string goes through `t()` with matching keys in
   both `i18n/zh.ts` and `i18n/en.ts` — no hard-coded literals (see CLAUDE.md i18n rule).
4. Every PR gets at least one review. Reviewers check correctness, edge/error
   handling, security, test coverage, and project conventions. Address all
   findings (or push back with a technical reason) before merge.

## Secrets — never commit them

Never put real API keys, passwords, tokens, JWTs, private keys, or
password-bearing connection strings into code, tests, fixtures, examples, or
commit history. Use instead:

- environment variables (`std::env::var("API_KEY")` / `os.environ.get(...)`),
- placeholders (`your-key-here`, `<API_KEY>`, `${ENV_VAR}`),
- clearly-fake test stubs (`test-pass-not-real`, `fake_key_for_test`).

Test fixtures use literal fake values; tests that call real APIs read keys from
env vars or use mocks. If a secret is ever committed, treat it as leaked: rotate
it immediately and rewrite history — do not just delete the file.

## Reporting bugs / requesting features

Open a GitHub issue with reproduction steps (browser/CLI path, what you expected,
what actually happened) and your environment. For security-sensitive reports,
do not open a public issue — follow the disclosure note in the README / SECURITY
contact instead.

---

<a id="中文"></a>

## 中文

感谢参与贡献。本文件是简短的上手指引；权威开发参考是
**[DEVELOP.md](DEVELOP.md)**（架构、分支模型、完整构建/测试命令、发布流程），本页
不重复其内容，只做指引 + 贡献约定说明。

### 开发环境

工具链、构建、测试命令见 **[DEVELOP.md](DEVELOP.md)**。简言之：

- **Rust 商用线** (`rust/`)：`cd rust && cargo build` / `cargo test --workspace`。
- **Python 原型线** (`python/`)：venv + `pip install -e .[dev]` + `pytest`。
- **Chrome 扩展** (`extension/`)：`npm install && npm run build`。

第三方署名见 [NOTICE](NOTICE) / [ACKNOWLEDGMENTS.md](ACKNOWLEDGMENTS.md)。

### 分支模型（GitFlow-Lite）

完整表格见 [DEVELOP.md → 分支模型](DEVELOP.md)。要点：

- 两条长期分支：**`main`**（稳定，仅 tag）与 **`develop`**（集成）。
- 从 `develop` 切短期 **`feature/<topic>`** 分支开发。
- PR 合入 **`develop`**（**不直接进 `main`**）。`develop → main` 仅用于发布，走
  `--no-ff` merge + 在 `main` 上打 `vX.Y.Z` tag。
- 合并后立即删除 feature 分支（远端 + 本地）。

### Commit 约定

- Conventional-commit 风格祈使句 subject：`feat(scope): …` / `fix(scope): …` /
  `docs(scope): …` / `test(scope): …` / `chore(scope): …` / `refactor(scope): …`。
- **原子 commit** —— 每个 commit 一个逻辑改动；subject 用祈使句，body 说明 *为什么*
  + 用户可见影响。开 PR 前先 squash 掉 WIP 噪声。
- subject/body 中英文均可（技术标识符保留英文）。

### 评审与测试要求

请求评审 / 开 PR 前：

1. **测试通过**：改动相关的 `cargo test --workspace`（Rust）/ `pytest`（Python）全绿。
   新行为必须带新测试 —— 覆盖 happy path、边界、错误场景（见
   [docs/TESTING.md](docs/TESTING.md)）。
2. **Lint 干净**：`cargo clippy --workspace --all-targets -- -D warnings`（Rust）、
   `ruff`（Python），无新增告警。
3. **i18n**：任何用户可见 UI 字符串走 `t()`，且 `i18n/zh.ts` 与 `i18n/en.ts` key 一致，
   零硬编码字面量（见 CLAUDE.md i18n 规范）。
4. 每个 PR 至少一次评审。评审检查正确性、边界/错误处理、安全、测试覆盖、项目约定。
   合并前修齐所有 finding（或给出技术理由反驳）。

### Secrets —— 永不提交

禁止把真实 API key、密码、token、JWT、私钥、含密码的连接串写进代码、测试、fixture、
示例或 commit 历史。请改用：

- 环境变量（`std::env::var("API_KEY")` / `os.environ.get(...)`）；
- 占位符（`your-key-here` / `<API_KEY>` / `${ENV_VAR}`）；
- 明示的假 test stub（`test-pass-not-real` / `fake_key_for_test`）。

测试 fixture 用字面假值；调真实 API 的测试从 env var 读 key 或走 mock。一旦误提交
secret，按泄露处理：立即轮换 + 重写历史，不能只删文件。

### 报告 Bug / 提需求

提 GitHub issue，附复现步骤（浏览器/CLI 路径、期望、实际）+ 环境信息。安全敏感的报告
**不要**开公开 issue，按 README / SECURITY 联系方式私下披露。
