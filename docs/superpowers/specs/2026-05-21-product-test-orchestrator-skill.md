# product-test-orchestrator Skill — 设计文档

**Status**: Draft（spec → user review → writing-plans → impl 三阶段，本文档仅完成第 1 阶段）
**Target Release**: skill v1.1（attune v1.0 GA 后，2026-06 上旬启动 impl）
**Owner**: attune main repo（spec 落在这里）+ 全局 `~/.claude/skills/product-test-orchestrator/`（impl）
**Scope**: 一个**项目无关**的测试编排 skill，任何产品级仓库接入即可获得自动化的多阶段测试编排（pre-commit / pre-push / PR / pre-merge / nightly / pre-release）+ 跨仓 integration test 协同

---

## 0. TL;DR

把"每次 commit 跑什么测、每次 PR 跑什么测、每次发版跑什么测、attune ↔ attune-pro 配对发布时怎么 cross-repo verify"这一系列**手动决策 + 易漏项**的工作，统一成一份 `.test-orchestrator.yaml` 加一个 portable runner。

- **零冗余**：项目维护者只填 < 50 行 yaml，其他全自动 derive
- **自动检测项目类型**：看 `Cargo.toml` / `package.json` / `pyproject.toml` / `tauri.conf.json` / `docker-compose.yml` → 知道这是什么栈
- **统一 phase**：6 个标准 phase（pre-commit / pre-push / PR / pre-merge / nightly / pre-release），覆盖所有触发场景
- **跨仓**：attune ↔ attune-pro ↔ attune-cloud 同号发布时自动配对触发 integration test
- **可扩展**：新项目类型 / 新 phase / 新 reporter 都是 plugin 式接入，core 不动

本 skill **本身不写新测试**，只编排现有测试 + 自动 trigger + 结果聚合。

---

## 1. 目标定位

### 1.1 用户原始诉求（2026-05-21）

> "产品级别为什么测试都没办法自动触发呢？claude 或者 agent 或者 skill 能否实现产品级别的测试安排布局（根据项目类型自动完成测试安排，并在合适的时间触发）"

### 1.2 痛点诊断（attune 项目实际踩过的坑）

| 痛点 | 历史事故 |
|------|---------|
| **手动决定跑什么测** | v0.6.3 发版前临时想起来"诶 Web UI 没跑过 Playwright"，补测发现 Wizard step3 LLM 选项白屏 |
| **漏测 UI / 跨切面** | 多次 commit 改了 i18n key 表但漏测英文 locale，用户切英文显示中文 |
| **跨仓 integration 没人管** | attune ↔ attune-pro 边界瘦身后，OSS 仓 build 通过但 pro pack 装载失败，因为没有自动 cross-repo CI |
| **nightly 任务零** | 真 LLM 调用 gate / cold-start / 性能 baseline 从来没有 nightly，全靠记得手跑 |
| **pre-release ceremony 散落** | install 包验签 / cross-platform build / 真实安装试跑分散在 release 文档，每次发版重新拼凑 |

### 1.3 定位

**一个 portable / project-agnostic skill**，提供：

1. 标准化 **6 phase 测试编排**（pre-commit → pre-release）
2. **项目类型自动检测**（无需手填）
3. **多触发机制**（git hook / CI / cron / 手动）
4. **结果聚合 + 报告**（标准 JSON / PR comment / 邮件 / Slack 可选）
5. **跨仓 integration 协议**（attune ↔ attune-pro 同号配对触发 reference 实现）

### 1.4 与 attune 产品定位的对齐

- **隐私**：测试结果不出网，PR comment / 邮件等 reporter 全部为可选；默认本地落盘
- **本地优先**：所有 phase 必须可在本地复现（CI 跑的就是 `pnpm test` / `cargo test` 等本地命令）
- **成本感知**（per CLAUDE.md「成本感知与触发契约」）：phase 与成本绑定（pre-commit 必须零成本；nightly 允许花 LLM token）

### 1.5 适用范围

| 项目 | 类型 | 接入优先级 |
|------|------|-----------|
| attune（OSS 主仓） | Rust workspace + Tauri | P0 — reference 实现 |
| attune-pro | Rust workspace + plugin packs | P0 — 与 OSS 配对 |
| attune-cloud | Rust + Docker compose（B 端 SaaS） | P1 |
| KVM | Go / 其他（待 detector 实装） | P2 |
| lawcontrol | Python + Django + Vue + 19 容器 | P2 — 与 attune 完全独立 |
| rv-* 子项目 | C/C++/Rust + 交叉编译 + bench | P3 — 需要扩展 detector + 远端 SSH 执行器 |

---

## 2. 范围边界

### 2.1 做什么

- **phase 编排** — 把 6 个标准 phase 与触发机制 + 命令矩阵绑定
- **项目类型自动检测** — manifest 文件 → 项目类型 → 推荐命令
- **跨仓 integration 协议** — 同号配对触发（attune `v1.0.0` 触发 attune-pro `v1.0.0` 的 integration test）
- **结果聚合** — 标准 JSON report，可消费 reporter 插件
- **触发机制粘合** — 输出 git hook script / GitHub Actions workflow / cron entry

### 2.2 不做什么

- ❌ **不写新测试本身** — 已有 `cargo test` / `pytest` / `playwright test` 等用户自己写；本 skill 只调用
- ❌ **不替代 CI runner** — GitHub Actions / GitLab CI / Jenkins 等还是它们跑；本 skill 只生成 workflow 文件
- ❌ **不做 deployment** — 部署是 release process 的事，本 skill 只跑 pre-release 测试（包验签 / install 试跑）
- ❌ **不做 monitoring** — 运行时监控（uptime / SLO）超出范围
- ❌ **不做 test 生成 / mock 生成** — 那是 `tdd-guide` / `senior-qa` 等 skill 的事
- ❌ **不直接调远端服务做付费操作** — 真 LLM gate 等花钱场景必须显式 opt-in + 走会员配额

### 2.3 后续 vNext

- v2.0：**flaky test detection** — 自动识别经常红的 case，标记 `quarantine` + 通知 owner
- v2.1：**affected-test selection** — 根据 git diff 算 affected 模块，只跑相关测试（per CI 资源优化，类似 Bazel `--affected`）
- v2.2：**test impact analysis** — 跨 phase 推算"如果 X 改了，nightly 哪些会挂"
- v3：**LLM-assisted root cause** — 把失败 stack 喂给 LLM 给修复建议（成本感知契约下，仅 nightly / pre-release 启用）

---

## 3. 架构数据流

### 3.1 高层流图

```
        ┌────────────────────────────────────┐
        │ 项目 repo                          │
        │ ├── .test-orchestrator.yaml        │  ← 项目维护者填 (< 50 行)
        │ ├── Cargo.toml / package.json /    │  ← 自动 detect 项目类型
        │ │   pyproject.toml / tauri.conf.json│
        │ └── tests/                          │
        └────────────────┬───────────────────┘
                         ↓
        ┌────────────────────────────────────┐
        │ orchestrator runner                 │
        │ (Rust binary or Python script)      │
        │  1. parse yaml + schema validate    │
        │  2. detect project type via plugins │
        │  3. resolve test matrix per phase   │
        │  4. inject phase context (env vars) │
        │  5. dispatch to executors           │
        └────────────────┬───────────────────┘
                         ↓
            ┌────────────┴────────────┐
            ↓                         ↓
  ┌──────────────────┐     ┌────────────────────────┐
  │ 触发机制(adapter)│     │ test executor (plugin) │
  │ ─ git hook       │     │ ─ cargo test           │
  │ ─ GH Action      │     │ ─ pytest               │
  │ ─ GitLab CI      │     │ ─ playwright           │
  │ ─ cron / systemd │     │ ─ go test              │
  │ ─ manual CLI     │     │ ─ shell custom         │
  └──────────────────┘     └────────────┬───────────┘
                                        ↓
        ┌───────────────────────────────────────────┐
        │ result aggregator (junit XML → JSON)      │
        │ + standardized phase report               │
        └────────────────┬──────────────────────────┘
                         ↓
            ┌────────────┴────────────┐
            ↓                         ↓
   ┌────────────────────┐    ┌────────────────────┐
   │ reporter plugins   │    │ cross-repo trigger │
   │ ─ stdout / log     │    │ (attune ↔ pro)     │
   │ ─ PR comment       │    │ via release tag    │
   │ ─ Slack webhook    │    │ matching protocol  │
   │ ─ email            │    └────────────────────┘
   │ ─ JSON file        │
   │ ─ dashboard (opt)  │
   └────────────────────┘
```

### 3.2 phase 时序（典型流转）

```
开发者本地:
    git commit        ─→ pre-commit hook  ─→ orchestrator(pre-commit phase)
                                              └→ fmt + lint + quick unit (< 10s)
    git push          ─→ pre-push hook    ─→ orchestrator(pre-push phase)
                                              └→ full unit + clippy (< 5min)

GitHub PR opened/updated:
    PR webhook        ─→ GH Action        ─→ orchestrator(pr phase)
                                              └→ full workspace test
                                              └→ integration test
                                              └→ build verify (3 platforms)
                                              └→ playwright UI smoke
                                              └→ post PR comment

PR ready to merge:
    branch protection ─→ orchestrator(pre-merge phase, ENFORCE mode)
                          └→ pr phase 所有 check 必须 ✅
                          └→ + 跨仓配对 check（如 attune ↔ pro 同号配对）

每天 02:00:
    cron / GH cron    ─→ orchestrator(nightly phase)
                          └→ Playwright UI E2E
                          └→ 真 LLM gate（attune-pro membership token）
                          └→ multi-platform build matrix
                          └→ perf baseline regression
                          └→ result → dashboard + 邮件 only if regression

打 tag v1.0.0:
    git tag           ─→ release workflow ─→ orchestrator(pre-release phase)
                                              └→ full ceremony
                                              └→ install pkg verify (Linux deb / Win MSI)
                                              └→ cold-start E2E
                                              └→ cross-repo integration with attune-pro v1.0.0
                                              └→ generate release notes draft
```

---

## 4. 模块边界

### 4.1 物理位置

| 位置 | 内容 | 谁维护 |
|------|------|--------|
| `~/.claude/skills/product-test-orchestrator/SKILL.md` | skill 入口（Claude 触发） | 全局，跨项目 |
| `~/.claude/skills/product-test-orchestrator/runner/` | core runner 实装（Rust 二进制或 Python） | 全局 |
| `~/.claude/skills/product-test-orchestrator/plugins/detectors/` | 项目类型 detector 插件 | 全局 |
| `~/.claude/skills/product-test-orchestrator/plugins/executors/` | test executor 插件 | 全局 |
| `~/.claude/skills/product-test-orchestrator/plugins/reporters/` | reporter 插件 | 全局 |
| `~/.claude/skills/product-test-orchestrator/schemas/` | yaml JSON schema | 全局 |
| `<project>/.test-orchestrator.yaml` | 项目配置（< 50 行） | 各项目维护者 |
| `<project>/.test-orchestrator/` | 项目本地缓存 / 历史 report（gitignored） | 自动生成 |
| `<project>/.github/workflows/test-orchestrator.yml` | 由 skill 生成的 GH Actions（committed） | 项目维护者 generate 一次 |

### 4.2 与其他 skill 的边界

| skill | 关系 |
|-------|------|
| `tdd-guide` | 生成测试代码 — 上游，本 skill 是下游编排器 |
| `senior-qa` | 测试覆盖率分析 — 平行，本 skill 可调用其作为 nightly executor 之一 |
| `timed-task` | 时长约束型任务 — 正交，本 skill 是事件驱动型不是时长型 |
| `verification-before-completion` | 完工前验证 — 平行，本 skill 是"周期性 + 触发性"，那个是"完工时一次性" |
| `ralph-loop` | 后台循环跑任务 — 不冲突，本 skill 可在 ralph-loop 内被调度 |

### 4.3 跨仓配对

attune 与 attune-pro 同号发布时（v1.0.0 ↔ v1.0.0），本 skill 需要：

1. **检测配对**：attune `pre-release` phase 跑到 `cross-repo-integration` step 时
2. **查 attune-pro 仓**是否存在同号 tag
3. **触发 attune-pro 的 `pre-release` phase**（API call 或 git fetch + 本地 clone 跑）
4. **聚合两边结果** → 单一 release readiness 报告

详见 §5.4 跨仓协议。

---

## 5. API 契约

### 5.1 配置 schema（`.test-orchestrator.yaml`）

**JSON Schema 强校验**（位置 `schemas/v1.json`）。完整字段：

```yaml
schema_version: "1"                # 必填，用于未来 migration

project:
  name: attune                     # 必填
  type: auto                       # auto | rust-workspace | tauri-app | python-fastapi | node-app | go-monorepo | mixed
  primary_language: rust           # 仅当 type=mixed 时需要
  monorepo_roots: []               # 子项目目录，type=mixed 时可指多个

defaults:
  timeout_sec: 1800                # 默认 phase 超时
  fail_fast: true                  # 任一 executor 失败立刻 abort phase
  cwd: .                           # 默认工作目录
  env:                             # 全 phase 共享 env
    RUST_BACKTRACE: "1"

phases:
  pre-commit:                      # < 10s
    enabled: true
    timeout_sec: 10
    executors:
      - name: fmt
        cmd: cargo fmt --check
      - name: lint-changed
        cmd: cargo clippy --workspace -- -D warnings
        when: changed_files_match('**/*.rs')   # 仅当有 rust 文件改动时跑
  
  pre-push:                        # < 5min
    enabled: true
    timeout_sec: 300
    executors:
      - name: unit
        cmd: cargo test --workspace --lib
      - name: clippy-full
        cmd: cargo clippy --workspace --all-targets -- -D warnings
  
  pr:                              # GH Actions 触发，< 30min
    enabled: true
    timeout_sec: 1800
    parallelism: 4
    executors:
      - name: full-test
        cmd: cargo test --workspace --all-features
      - name: build-verify
        matrix:
          target:
            - x86_64-unknown-linux-gnu
            - x86_64-pc-windows-msvc
            - aarch64-unknown-linux-gnu
        cmd: cargo build --target ${MATRIX_TARGET} --release
      - name: playwright-smoke
        cmd: pnpm --filter ui test:e2e -- --grep '@smoke'
        when: changed_files_match('**/ui/**')
  
  pre-merge:                       # ENFORCE mode
    enabled: true
    enforce: true                  # 任何 missing/skipped check 都阻止 merge
    requires:                      # 必须通过的 phase
      - pr
    cross_repo:                    # 跨仓配对 check
      - repo: ../attune-pro
        phase: pr
        when: branch_eq('main') and tag_match('v*')
  
  nightly:                         # cron 02:00
    enabled: true
    timeout_sec: 7200
    schedule: "0 2 * * *"          # cron 表达式
    executors:
      - name: playwright-full
        cmd: pnpm --filter ui test:e2e
      - name: real-llm-gate
        cmd: cargo test --test real_llm_gate -- --ignored
        env:
          ATTUNE_PRO_TOKEN: ${ATTUNE_PRO_TOKEN}    # 从 secret store 注入
        cost_tier: token                          # 标记花钱 — 由 reporter 汇总成本
      - name: perf-baseline
        cmd: cargo bench --bench memory_island -- --save-baseline nightly-${DATE}
        cost_tier: local-gpu
    on_failure:
      reporters: [email, slack]    # 仅 nightly 失败时通知
  
  pre-release:                     # tag 推送时
    enabled: true
    timeout_sec: 14400
    triggered_by: tag_match('v*')
    executors:
      - name: clean-build
        cmd: cargo clean && cargo build --release
      - name: install-pkg-verify
        cmd: bash packaging/verify-install.sh
        matrix:
          os: [ubuntu-latest, windows-latest]
      - name: cold-start-e2e
        cmd: pnpm --filter ui test:e2e -- --grep '@cold-start'
      - name: cross-repo-integration
        cmd: ./scripts/cross-repo-test.sh
        cross_repo:
          - repo: ../attune-pro
            tag: same                            # 配对相同 tag
          - repo: ../attune-cloud
            tag: same
            optional: true                       # 不强制

reporters:
  - type: stdout                                  # 默认
  - type: json
    path: .test-orchestrator/report.json
  - type: pr_comment                              # 仅 GH Actions 环境生效
    only_phases: [pr]
  - type: slack                                   # 可选，secret 必填
    webhook: ${SLACK_WEBHOOK}
    only_failure: true
    only_phases: [nightly, pre-release]
  - type: email
    to: [dev-team@engi-stack.com]
    only_failure: true
    only_phases: [pre-release]
```

**字段约束**：
- `phases.<name>.executors[].cmd` 必填，`when` / `matrix` / `cost_tier` 可选
- `phases.<name>.cross_repo[].repo` 接受 absolute path / git URL / org/repo（GH 自动解析）
- `reporters[].only_phases` / `only_failure` 控制触发条件，避免 spam

### 5.2 CLI 契约

```
attune-test-orchestrator <subcommand> [options]

subcommands:
  run <phase>              在当前 cwd 运行指定 phase
    --dry-run              只 plan 不 execute
    --executor <name>      只跑某个 executor
    --no-fail-fast         覆盖 yaml
    --report-json <path>   覆盖 report 路径
    --project <path>       项目根目录（默认 cwd）

  detect                   打印检测到的项目类型 + 推荐 yaml
    --write                把推荐 yaml 写到 .test-orchestrator.yaml（不覆盖已存在）

  generate <target>        生成集成文件
    target ∈ {git-hooks, gh-actions, gitlab-ci, cron}
    --force                覆盖已存在

  validate                 校验 .test-orchestrator.yaml 是否符合 schema
    --strict               额外检查 cmd 是否存在可执行

  list-phases              列出已配置 phase

  cross-repo-status <tag>  查 attune ↔ attune-pro 同号 tag 是否齐全 + 各 phase 结果
```

**exit code 约定**：

| code | 含义 |
|------|------|
| 0 | 全部 executor 通过 |
| 1 | 至少一个 executor failed（业务失败） |
| 2 | 配置 / schema 错误 |
| 3 | 项目类型 detect 失败 |
| 4 | 跨仓配对 missing |
| 130 | SIGINT（用户取消） |

### 5.3 GitHub Action

发布为 marketplace action：

```yaml
- uses: attune-ai/test-orchestrator@v1
  with:
    phase: pr                       # pr | pre-merge | nightly | pre-release
    project: .                      # 项目根目录
    config: .test-orchestrator.yaml # 默认值
    fail-on-warning: false
  env:
    ATTUNE_PRO_TOKEN: ${{ secrets.ATTUNE_PRO_TOKEN }}
```

### 5.4 跨仓协议

#### 5.4.1 配对触发模式

attune 在 `pre-release` 跑到 `cross-repo-integration` 时：

1. **本地 path 模式**（开发期）：
   ```yaml
   cross_repo:
     - repo: ../attune-pro    # 相对路径
       tag: same              # 同号
   ```
   runner 在 `../attune-pro` clone 中 `git checkout <tag>` 然后跑 `attune-test-orchestrator run pre-release`

2. **远端 fetch 模式**（CI）：
   ```yaml
   cross_repo:
     - repo: attune-ai/attune-pro
       tag: same
       auth: github_token
   ```
   runner `git clone --depth 1 --branch <tag>` 到 tempdir 后跑

3. **API 模式**（CI 强解耦）：
   ```yaml
   cross_repo:
     - repo: attune-ai/attune-pro
       trigger: api
       endpoint: https://api.github.com/repos/.../dispatches
       event_type: pre-release-paired
       wait_for_completion: true
   ```
   runner 用 `repository_dispatch` 触发对方 workflow，poll 结果

#### 5.4.2 配对失败语义

| 场景 | 结果 |
|------|------|
| attune `v1.0.0` 已 tag，attune-pro `v1.0.0` 未 tag | 阻止 attune release（exit 4） |
| 两方都 tagged，attune-pro `pre-release` 失败 | 阻止 attune release（聚合 report 标 X） |
| `optional: true` 的 repo 缺失 | warning，不阻止 |

#### 5.4.3 同号 tag 强制

在 `pre-merge` phase 触发 release branch 时：
- skill 检查 `attune-pro` 的 `develop` 是否已有对应 `vX.Y.Z-rc.N` tag
- 没有 → 提示用户："attune-pro 还没打 vX.Y.Z-rc.N，要不要先去 pro 仓打 tag？"

---

## 6. 扩展点 / 插件接口

### 6.1 ProjectDetector 插件

```rust
// runner/src/plugins/detector.rs
pub trait ProjectDetector {
    /// 检测当前 cwd 是否匹配
    fn detect(&self, cwd: &Path) -> Option<ProjectType>;

    /// 项目类型 → 推荐默认 yaml
    fn recommend_config(&self, project_type: &ProjectType) -> ConfigTemplate;

    /// 优先级（多 detector 冲突时用）
    fn priority(&self) -> u32 { 100 }
}
```

**内置 detector**：

| detector | manifest 文件 | 推荐 executor |
|----------|--------------|--------------|
| RustWorkspaceDetector | `Cargo.toml` with `[workspace]` | `cargo fmt`/`clippy`/`test` |
| TauriAppDetector | `tauri.conf.json` | `pnpm tauri build` + `playwright` |
| PythonFastApiDetector | `pyproject.toml` + `from fastapi` import grep | `ruff` + `pytest` + `httpx` API smoke |
| NodeAppDetector | `package.json` + `scripts.test` | 看 framework：`vitest` / `jest` / `playwright` |
| GoMonorepoDetector | `go.mod` + `go.work` | `go test ./...` + `golangci-lint` |
| DockerComposeDetector | `docker-compose.yml` | `docker compose up --build --abort-on-container-exit` |
| MixedRepoDetector | 多个 manifest 共存 | 子项目分别 detect + 聚合 |

**注册**：扔进 `~/.claude/skills/product-test-orchestrator/plugins/detectors/<name>.rs`，runner 启动时 dyn-load。

### 6.2 TestExecutor 插件

```rust
pub trait TestExecutor {
    fn name(&self) -> &str;

    /// 执行命令，返回标准化结果
    fn execute(&self, cmd: &str, ctx: &PhaseContext) -> ExecutorResult;

    /// 解析输出为标准 junit XML（如果工具支持的话）
    fn parse_output(&self, raw: &str) -> Option<JunitReport>;
}
```

**内置 executor**：

| executor | 支持工具 |
|----------|---------|
| `shell` | 任意命令，默认 fallback |
| `cargo` | `cargo test/build/clippy/bench` — 解析 cargo JSON output |
| `pytest` | 解析 `--junitxml=` 输出 |
| `playwright` | 解析 `--reporter=json` 输出 |
| `go-test` | 解析 `go test -json` |
| `vitest` / `jest` | 解析 reporter JSON |
| `gh-action` | 触发远端 GitHub workflow（cross-repo API 模式专用） |

### 6.3 Reporter 插件

```rust
pub trait Reporter {
    fn name(&self) -> &str;

    fn supports(&self, phase: &str) -> bool;

    fn report(&self, phase_report: &PhaseReport) -> Result<()>;
}
```

**内置 reporter**：

| reporter | 触发场景 |
|----------|---------|
| `stdout` | 总是，本地开发期 |
| `json` | 写 `.test-orchestrator/report.json`，给其他工具消费 |
| `pr_comment` | GH Actions PR phase，调 GH API 发评论 |
| `slack` | 失败 + 配置 webhook |
| `email` | pre-release 失败 |
| `dashboard` | optional，POST 到自建 dashboard（attune-cloud 可选托管） |

### 6.4 新增插件流程

1. 实现对应 trait
2. 扔进 `plugins/<kind>/<name>.{rs|py}`
3. 注册在 `plugins/<kind>/REGISTRY.toml`（手填一行）
4. 写 1 个 golden test（fixture 项目 + 期望输出）
5. PR 到 `product-test-orchestrator` skill 仓

---

## 7. 错误处理 + 边界 case

### 7.1 配置错误

| 错误 | 处理 |
|------|------|
| yaml 不存在 | 报"未找到 .test-orchestrator.yaml，要不要 `attune-test-orchestrator detect --write` 生成？" |
| schema 不符（如缺 `schema_version`） | 报具体 line 号 + 期望字段 + 离最近 valid yaml 的 diff |
| `phases.X.executors[].cmd` 为空 | 拒绝运行（exit 2） |
| `schema_version` 不识别（未来版本） | 报"本 runner 版本 vN 不支持 schema vM，请升级 runner" |

### 7.2 执行错误

| 错误 | 处理 |
|------|------|
| 命令找不到（`cargo: command not found`） | 报 executor 名 + 推荐安装命令（per project type） |
| executor 超时 | SIGTERM → 5s 后 SIGKILL，report status=timeout |
| executor segfault | report status=crashed + signal 名 |
| executor stdout 不是合法 junit | report status=passed/failed by exit code，warning："无 junit 输出，仅以 exit code 判断" |
| 跨仓 repo 找不到 | exit 4 + 提示"如果是 optional repo，加 `optional: true`" |

### 7.3 边界 case

#### 7.3.1 monorepo 含多语言

例如一个 repo 同时有 `Cargo.toml` + `package.json` + `pyproject.toml`：

- `project.type: auto` → 多个 detector 都命中
- yaml 必须显式 `type: mixed` + 列 `monorepo_roots`，否则 exit 2
- 每个 root 跑各自 phase，结果聚合

#### 7.3.2 CI runner 资源不足

- `pr` phase 默认 `parallelism: 4`，runner 自动降级到 CPU 核数
- nightly 跑不完 → kill + report 部分结果 + 下次继续（断点能力 vNext）

#### 7.3.3 git hook 与 CI 重复跑

- `pre-push` 已在本地跑过；CI `pr` 又跑一遍 → 默认允许重复（信任本地不可靠）
- 可选 `phases.pre-push.skip_in_ci: true` 跳过

#### 7.3.4 没有网络的环境

- nightly / pre-release 含 `cross_repo: trigger: api` 时需要网络
- detect 不通 → 自动降级到 `cross_repo: trigger: local-path` if 配置允许，否则 exit 4

#### 7.3.5 用户取消（Ctrl+C）

- runner 捕获 SIGINT → 给所有子进程 SIGTERM → 5s 后 SIGKILL
- 写 partial report 到 `.test-orchestrator/report.json`
- exit 130

### 7.4 attune / KVM / 其他项目接入示例

#### 7.4.1 attune `.test-orchestrator.yaml`（reference 实现）

```yaml
schema_version: "1"

project:
  name: attune
  type: tauri-app                  # Rust workspace + Tauri 前端

defaults:
  timeout_sec: 1800
  env:
    RUST_BACKTRACE: "1"
    CARGO_TERM_COLOR: always

phases:
  pre-commit:
    enabled: true
    executors:
      - name: fmt
        cmd: cargo fmt --check
      - name: i18n-guard
        cmd: bash scripts/i18n-grep-guard.sh    # 防中英混杂（per CLAUDE.md）

  pre-push:
    enabled: true
    timeout_sec: 600
    executors:
      - name: unit
        cmd: cargo test --workspace --lib
      - name: clippy
        cmd: cargo clippy --workspace --all-targets -- -D warnings

  pr:
    enabled: true
    executors:
      - name: full-test
        cmd: cargo test --workspace --all-features
      - name: ui-smoke
        cmd: pnpm --filter ui test:e2e -- --grep '@smoke'
      - name: build-verify
        matrix:
          target: [x86_64-unknown-linux-gnu, x86_64-pc-windows-msvc]
        cmd: cargo build --target ${MATRIX_TARGET} --release

  pre-merge:
    enabled: true
    enforce: true
    requires: [pr]

  nightly:
    enabled: true
    schedule: "0 2 * * *"
    executors:
      - name: playwright-full
        cmd: pnpm --filter ui test:e2e
      - name: real-llm-gate
        cmd: cargo test --test real_llm_gate -- --ignored
        cost_tier: token
      - name: perf-baseline
        cmd: cargo bench --bench memory_island
        cost_tier: local-gpu

  pre-release:
    enabled: true
    triggered_by: tag_match('v*')
    executors:
      - name: install-verify
        cmd: bash packaging/verify-install.sh
        matrix:
          os: [ubuntu-latest, windows-latest]
      - name: cold-start
        cmd: pnpm --filter ui test:e2e -- --grep '@cold-start'
      - name: cross-repo-pro
        cross_repo:
          - repo: attune-ai/attune-pro
            tag: same

reporters:
  - type: stdout
  - type: json
    path: .test-orchestrator/report.json
  - type: pr_comment
    only_phases: [pr]
```

行数：约 49 行（满足 < 50 行约束）。

#### 7.4.2 attune-pro `.test-orchestrator.yaml`

```yaml
schema_version: "1"

project:
  name: attune-pro
  type: rust-workspace             # 6 个 plugin pack

defaults:
  timeout_sec: 900

phases:
  pre-commit:
    executors:
      - name: fmt
        cmd: cargo fmt --check

  pre-push:
    executors:
      - name: per-pack-test
        matrix:
          pack: [law-pro, sales-pro, tech-pro, patent-pro, medical-pro, academic-pro]
        cmd: cargo test -p ${MATRIX_PACK}

  pr:
    executors:
      - name: full-workspace
        cmd: cargo test --workspace
      - name: oss-compat
        cmd: bash scripts/test-against-oss.sh    # 用 attune OSS 仓 develop 当依赖跑
        cross_repo:
          - repo: ../attune
            tag: develop                          # 跟踪 OSS develop

  pre-merge:
    enforce: true
    requires: [pr]

  nightly:
    schedule: "0 3 * * *"                         # 比 attune 晚 1h，等 attune nightly 结果
    executors:
      - name: paired-with-oss
        cross_repo:
          - repo: ../attune
            phase: nightly
            wait_for_completion: true

  pre-release:
    triggered_by: tag_match('v*')
    executors:
      - name: paired-with-oss
        cross_repo:
          - repo: attune-ai/attune
            tag: same                             # v1.0.0 ↔ v1.0.0 强配对

reporters:
  - type: stdout
  - type: json
  - type: pr_comment
    only_phases: [pr]
```

行数：约 42 行。

#### 7.4.3 KVM `.test-orchestrator.yaml`（假设 Go）

```yaml
schema_version: "1"

project:
  name: KVM
  type: go-monorepo

defaults:
  timeout_sec: 1200

phases:
  pre-commit:
    executors:
      - name: gofmt
        cmd: gofmt -l . | grep . && exit 1 || exit 0
      - name: govet
        cmd: go vet ./...

  pre-push:
    executors:
      - name: unit
        cmd: go test -short ./...
      - name: lint
        cmd: golangci-lint run

  pr:
    executors:
      - name: full-test
        cmd: go test -race ./...
      - name: integration
        cmd: go test -tags=integration ./tests/integration/...

  pre-merge:
    enforce: true
    requires: [pr]

  nightly:
    schedule: "0 4 * * *"
    executors:
      - name: load-test
        cmd: go test -tags=load ./tests/load/...
        cost_tier: local-cpu

  pre-release:
    triggered_by: tag_match('v*')
    executors:
      - name: cross-platform-build
        matrix:
          os: [linux, darwin, windows]
          arch: [amd64, arm64]
        cmd: GOOS=${MATRIX_OS} GOARCH=${MATRIX_ARCH} go build ./cmd/...

reporters:
  - type: stdout
  - type: json
```

行数：约 40 行。

---

## 8. 成本契约

per CLAUDE.md「成本感知与触发契约」三层模型，每个 phase 必须声明所属成本档。

### 8.1 phase × 成本档对照

| phase | 默认成本档 | 触发频率 | 用户感知 |
|-------|----------|---------|---------|
| pre-commit | 🆓 零成本（CPU） | 每次 commit | 应 < 10s，无感 |
| pre-push | 🆓 零成本（CPU） | 每次 push | 应 < 5min，能容忍 |
| pr | 🆓 零成本（runner CPU） | 每个 PR open/update | < 30min，CI runner 算力 |
| pre-merge | 🆓 零成本（聚合已跑结果） | merge 前一次 | < 1s（只聚合，不重跑） |
| nightly | ⚡ 本地算力 + 💰 token | 每天 1 次 | 真 LLM gate 算 token；perf bench 算 GPU |
| pre-release | ⚡ 本地算力 + 💰 token | 每个 release | 多平台 build + cross-repo + 真实安装试跑 |

### 8.2 cost_tier 字段语义

每个 executor 必须（隐式或显式）标注 `cost_tier`：

```yaml
executors:
  - name: real-llm-gate
    cmd: cargo test --test real_llm_gate -- --ignored
    cost_tier: token                # 显式声明
  - name: perf-baseline
    cost_tier: local-gpu
  - name: unit
    # 不写就是 local-cpu（默认）
```

合法值：`local-cpu` / `local-gpu` / `local-npu` / `token` / `external-api`。

### 8.3 reporter 汇总成本

`json` reporter 输出每个 phase 的成本汇总：

```json
{
  "phase": "nightly",
  "duration_sec": 4823,
  "cost_summary": {
    "local-cpu-min": 80,
    "local-gpu-min": 12,
    "tokens": {
      "provider": "attune-pro-gateway",
      "input": 14523,
      "output": 8721,
      "estimated_usd": 0.038
    }
  }
}
```

### 8.4 成本预算守卫

yaml 可配 budget：

```yaml
phases:
  nightly:
    budget:
      tokens_usd_max: 1.00       # 单次 nightly 最多 $1
      duration_sec_max: 7200     # 最多 2h
    on_budget_exceeded: warn     # warn | abort
```

---

## 9. 测试矩阵

本 skill 自己也要符合自己的标准。

### 9.1 自测层级

| 层级 | 描述 | 数量目标 |
|------|------|---------|
| L1 单元测试 | 配置 parse / schema validate / cmd 解析 | ≥ 30 case |
| L2 detector 单元 | 每个 builtin detector 一组 fixture 项目 | ≥ 7 detector × 1 fixture |
| L3 executor 单元 | mock 子进程输出，验证 result 解析 | ≥ 6 executor × 1 case |
| L4 phase integration | fake project 跑完整 phase，验证 report | ≥ 6 phase × 1 |
| L5 跨仓 E2E | attune ↔ attune-pro fixture 仓，跑 pre-release 配对 | ≥ 3 case（成功 / 缺 tag / pro phase 失败） |
| L6 golden | 每个 builtin 项目类型 + 每个 phase 组合的 expected JSON | ≥ 12 golden |
| L7 prop test | yaml fuzzing → schema validate | ≥ 3 proptest × 100 cases |
| L8 boundary | 超时 / 资源耗尽 / 用户取消 / 配置缺失 / cmd 不存在 | ≥ 5 case |
| L9 error injection | executor 故意 crash / segfault / timeout / 非法 junit | ≥ 3 case |

### 9.2 golden case 设计

每个 golden 包含：
- fixture project 目录（最小可跑示例）
- 期望的 `.test-orchestrator/report.json`
- 验证 script（diff or jq query）

12 个 golden：

| # | fixture | phase | 目标 |
|---|---------|-------|------|
| 1 | rust-workspace | pre-commit | fmt + clippy 通过 |
| 2 | rust-workspace | pre-push | unit test 失败应 fail fast |
| 3 | tauri-app | pr | playwright + cargo + build matrix |
| 4 | python-fastapi | pre-push | pytest + ruff |
| 5 | node-app | pr | vitest + playwright |
| 6 | go-monorepo | nightly | load test + budget warn |
| 7 | mixed-repo | pr | 多 root 聚合 |
| 8 | attune fixture | pre-release | cross-repo 跨 attune-pro 同号成功 |
| 9 | attune fixture | pre-release | cross-repo attune-pro tag 缺失 → exit 4 |
| 10 | attune fixture | pre-release | cross-repo attune-pro phase 失败 → 阻止 release |
| 11 | rust-workspace | pre-commit | yaml schema 错误 → exit 2 |
| 12 | rust-workspace | nightly | budget exceeded → abort |

### 9.3 跨仓 E2E（reference）

用 attune 真实仓 + attune-pro 真实仓做集成：

1. 在 attune fixture 仓 `git tag v9.9.9-test`（不 push）
2. 跑 `attune-test-orchestrator run pre-release`
3. 应触发 attune-pro 的 `pre-release`（本地 path 模式）
4. 聚合 report → 验证 cross_repo section

### 9.4 性能 SLO

| metric | SLO |
|--------|-----|
| `pre-commit` 总时长 | p95 < 10s |
| `pre-push` 总时长 | p95 < 5min |
| `pr` 总时长 | p95 < 30min |
| runner 自身启动 + 配置解析 overhead | < 500ms |
| cross-repo API trigger 等待 | < 30s 启动响应 |

每个 SLO 在 nightly perf bench 中跑。

### 9.5 真实使用 dogfood

attune 仓自己接 v1.1 第一个版本，作为 reference 跑 1 周：

- 收集每个 phase 的 wall-clock + 失败率
- 收集 yaml 维护成本（diff 行数 / 周）
- 收集"missed by orchestrator"事件（人工发现但 phase 没抓到的 bug）

dogfood 1 周后才能扩展到 attune-pro / cloud / 其他。

---

## 10. 向后兼容

### 10.1 yaml schema versioning

```yaml
schema_version: "1"     # 字符串而非数字，未来可能 "1.1" / "2"
```

策略：

| 变化 | 处理 |
|------|------|
| 新增可选字段 | minor bump（"1" → "1.1"），老 runner 忽略新字段 |
| 改字段语义 | major bump（"1" → "2"），自动 migrate tool（`attune-test-orchestrator migrate`） |
| 删字段 | major bump + 至少 1 个 release 周期的 deprecation warning |

### 10.2 runner 自身版本

`attune-test-orchestrator --version` 报告：

```
attune-test-orchestrator 1.1.0
supports schema versions: 1, 1.1
```

不兼容时报清晰错误：
```
ERROR: this .test-orchestrator.yaml requires schema 2.0
       but this runner only supports up to 1.1.
       please upgrade: cargo install attune-test-orchestrator
```

### 10.3 老项目接入路径

无 yaml 的项目接入：

1. `attune-test-orchestrator detect` → 推荐 yaml
2. `attune-test-orchestrator detect --write` → 落地默认 yaml
3. 用户改字段
4. `attune-test-orchestrator validate` → schema 校验
5. `attune-test-orchestrator generate git-hooks --force` → 安装 hooks
6. `attune-test-orchestrator generate gh-actions --force` → 落 workflow yaml

### 10.4 与现有 CI 共存

| 场景 | 处理 |
|------|------|
| 项目已有 `.github/workflows/ci.yml` | 不覆盖，本 skill 落 `test-orchestrator.yml` 独立 workflow |
| 项目已有 pre-commit hook | `generate git-hooks` 询问是否 append / 替换 / 跳过 |
| 老 CI 跑 `cargo test`、新 phase 又跑一遍 | 默认允许重复（信任老 CI 可能跑了不同 subset）；提供 `--check-overlap` 工具诊断 |

### 10.5 attune-pro / cloud 配套版本绑定

attune-pro 的 `.test-orchestrator.yaml` 必须声明 OSS attune 版本兼容范围：

```yaml
project:
  name: attune-pro
  oss_compat:
    min: "v1.0.0"
    max: "v1.x"           # semver caret
```

skill 在 `cross_repo` check 时验证版本兼容，不在范围内 exit 4。

---

## 11. 风险登记

### 11.1 高风险（必须在 v1.0 解决）

| 风险 | 缓解 |
|------|------|
| **项目类型 detector 误判 monorepo** | (1) 多 detector 命中时 exit 2 提示用户显式 `type: mixed`；(2) golden 覆盖所有混合 case |
| **触发链失误**（pre-commit → pre-push → PR → merge 漏跑某 phase） | (1) `pre-merge enforce: true` 强校验 `requires` 链；(2) report aggregator 显式标"phase X was skipped" |
| **跨仓配对死锁**（attune nightly 等 attune-pro nightly，pro 又等 OSS） | (1) yaml schema 验证时检测循环依赖；(2) `wait_for_completion: true` 必须有 timeout |
| **token 浪费**（nightly 跑爆 attune-pro 会员 quota） | (1) `cost_tier: token` + budget guard；(2) `--dry-run` 输出预估 token |

### 11.2 中风险（v1.1 / v1.2 接续）

| 风险 | 缓解 |
|------|------|
| **flaky test 反复挂 nightly** | v2.0 加 flaky detection + auto-quarantine |
| **跨平台 runner 差异**（Windows path / Linux shell） | runner 用 Rust 实现 + 跨平台 test matrix（per CLAUDE.md 跨平台规范）；shell 命令明确声明 `shell: bash` / `shell: powershell` |
| **secret 泄露**（webhook / token 写进 yaml） | yaml 只能引用 `${VAR}`，不能写明文；runner 检测明文 secret 模式（`xoxb-` / `ghp_` 等）拒绝运行 |
| **与现有 CI 冲突** | (1) 落独立 workflow 文件；(2) `--check-overlap` 工具诊断 |

### 11.3 低风险 / 可接受

| 风险 | 缓解 |
|------|------|
| 老项目接入摩擦 | `detect --write` 一键生成默认 yaml |
| 不同 git hosting 平台支持（GitLab / Gitea） | v1.0 仅 GitHub；v1.2 加 GitLab CI；Gitea 看需求 |
| reporter 数据敏感 | 默认仅 stdout / json file，其他全部 opt-in |

### 11.4 已知妥协

- v1.0 runner 用 **Rust** 写（与 attune 主线一致），单二进制分发；放弃 Python（依赖管理麻烦）+ Bash（跨平台坑多）
- runner 二进制 ~ 8 MB（含 schema validator + yaml parser + reqwest），可接受
- 项目维护者需要装 `cargo install attune-test-orchestrator`，加一行 dependency 而已；或用 `npx`-like 形式（vNext）

---

## 12. 设计点待用户 review

以下 **4 个设计决策点**需要用户确认，标记 `[TBD by user review]`：

### TBD-1: runner 实现语言

**选项**：
- A. **Rust**（推荐）：单二进制，跨平台干净，与 attune 主线技术栈一致；但启动比 Python 慢一点
- B. Python：用户已有解释器；但依赖管理跨平台坑（pip / venv）
- C. Bash：零依赖；但 Windows 支持靠 WSL，跨平台坑大

**默认推荐**：A。

### TBD-2: cross-repo 默认触发模式

**选项**：
- A. **本地 path 模式**（推荐 v1.0）：开发期简单，CI 时用 `actions/checkout` 拉两份；问题是 CI 环境耦合
- B. API trigger 模式：解耦干净，但 GitHub `repository_dispatch` 配置复杂
- C. 两者都支持，yaml 显式选

**默认推荐**：C，v1.0 先实现 A，v1.1 加 B。

### TBD-3: skill 与 attune 仓的物理位置

**选项**：
- A. **全局 skill 仓 `~/.claude/skills/product-test-orchestrator/`**（推荐）：跨项目共享，符合 timed-task / ort-pr 模式
- B. attune monorepo 内 `tools/test-orchestrator/`：与主代码同步演进，但其他项目 import 麻烦
- C. 独立 GitHub repo `attune-ai/test-orchestrator`：完全独立发布，但维护开销大

**默认推荐**：A 起步，dogfood 1 周稳定后考虑 C（独立 repo + cargo install 分发）。

### TBD-4: reporter 默认开启列表

**选项**：
- A. **仅 stdout + json**（推荐）：隐私优先，其他全部 opt-in
- B. stdout + json + pr_comment（CI 环境）：体验最好，但 PR comment 可能 spam
- C. 全开：最便利但隐私风险

**默认推荐**：A。pr_comment 通过 yaml 显式打开。

---

## 13. 实施路径（impl plan 占位，正式 plan 进 writing-plans）

### Phase 0：v1.0 GA 后启动（2026-05-26 起）

- v1.0 attune GA 优先级最高，本 skill 不抢占
- 5/26 启动 spec review + 4 个 TBD 拍板

### Phase 1：v1.1 MVP（2026-06 上旬，2 周）

- [ ] runner core（Rust）：yaml parse + schema + cmd dispatch
- [ ] 3 个 detector：rust-workspace / tauri-app / mixed
- [ ] 3 个 executor：shell / cargo / playwright
- [ ] 2 个 reporter：stdout / json
- [ ] CLI：`run` / `detect` / `validate` / `generate git-hooks`
- [ ] attune 仓接入 `.test-orchestrator.yaml`，dogfood 1 周
- [ ] L1-L4 测试 + 6 个 golden

### Phase 2：v1.1.1 跨仓 + 更多 detector（2026-06 下旬，2 周）

- [ ] cross-repo 协议（本地 path 模式）
- [ ] attune-pro / attune-cloud 接入
- [ ] 加 detector：python-fastapi / node-app / go-monorepo
- [ ] 加 executor：pytest / vitest / go-test
- [ ] 加 reporter：pr_comment
- [ ] L5 跨仓 E2E + 12 个 golden 补齐

### Phase 3：v1.2 平台扩展（2026-07，2 周）

- [ ] cross-repo API trigger 模式
- [ ] generate gh-actions / gitlab-ci workflow
- [ ] reporter：slack / email
- [ ] KVM / rv-* 接入（需要远端 SSH executor）
- [ ] L7-L9 测试补全

### Phase 4：v2.x（2026-Q3 起）

- [ ] flaky test detection
- [ ] affected-test selection
- [ ] LLM-assisted root cause（成本契约下）

---

## 14. 参考资料

- attune 项目 CLAUDE.md「成本感知与触发契约」「跨平台兼容规范」「i18n 规范」
- attune-pro 边界规则（`docs/oss-pro-strategy.md` v2）
- `~/.claude/skills/timed-task/SKILL.md` — skill 物理结构参考
- `docs/superpowers/specs/2026-04-17-product-positioning-design.md` — 产品定位
- `docs/superpowers/specs/2026-05-20-office-helper-design.md` — 最新 spec 范式
- GitHub Actions `repository_dispatch` 文档 — cross-repo API 模式实装参考
- Bazel `--affected` / Nx `affected` — vNext affected-test 设计参考

---

**End of spec.**

下一步：用户 review 4 个 `[TBD by user review]` 决策点，确认后进 `writing-plans` 阶段做 impl plan。
