# OSS patent 全栈硬删 (S4a) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 从 OSS attune 物理删除 patent 可执行能力(route + scanner + 孤儿 governor 变体 + 旧仓名注释 + README feature 行),硬删 404 无 alias;**不碰** S4b 范围(registry/flow/case_metadata/agent 测试解耦),`corpus_domain="patent"` 数据标签保留。

**Architecture:** 纯删除 sprint(TDD-for-deletion)。每 task 删一个内聚单元 → `cargo build` 0 dangling + 相关测试绿 + grep gate 0 命中。「failing test」语义:删前 grep/build 反映 patent 存在(RED-equivalent);删后转绿。删除顺序:server route(叶子,无下游)→ core scanner → governor 孤儿变体+F1(同 task,否则 cargo test RED)→ README/注释 → 全量回归。

**Tech Stack:** Rust workspace(`rust/Cargo.toml`)— attune-server (Axum) + attune-core。验证工具:`cargo build --release` / `cargo test --workspace` / `cargo clippy --workspace --all-targets -- -D warnings` / `grep -rn`。

**Source spec:** `docs/superpowers/specs/2026-06-01-oss-patent-migration.md` (S4a, G1 PASS 4.6, 含 F1 补正)。实施完成后**立即删本 plan**(§3.2 实施 plan 生命周期)。

**磁盘前置(每 build/test task 前)：** 跑 `df -h /data`。/data 黄线 188G;`rust/target` 160G 增量保留。本 sprint 是删除,build 增量极小(净减 ~510 LOC)。red 线 < 50G 停手清 `cargo clean` + worktree。

**跨仓注记(本 sprint 不阻塞)：** patent 能力落 `attune-pro/patent-pro` = 外部前置依赖,attune-pro 仓自己的 spec/plan 承接。本仓**不读不写** attune-pro。OSS 删除是仓内自包含改动(spec §10 sequencing),不等 Pro 就绪。

**Scope check（spec §2 严守，超范围即 REJECT）：** 本 plan 仅 patent 硬删。**不含** agents.registry.toml / agent_flows.toml / case_metadata.rs / defamation 测试解耦(=S4b);**不含** CI grep gate 行业词守卫(S4b);**保留** `corpus_domain="patent"` 数据标签(`ingest/connector.rs:72` / `store/dirs.rs:19` / `search.rs:140-143`)+ `patent_pro` plugin-id(`tests/generic_plugins_test.rs`)。所有 task 自检无超 §2 scope。

---

## File Structure

删/改 8 个文件(spec §4 模块边界表),无新建文件:

| crate | 文件 | 改动 | task |
|-------|------|------|------|
| attune-server | `rust/crates/attune-server/src/routes/patent.rs` | **删整文件**(147 行) | T1 |
| attune-server | `rust/crates/attune-server/src/routes/mod.rs:28` | 删 `pub mod patent;` | T1 |
| attune-server | `rust/crates/attune-server/src/lib.rs:178-179` | 删 2 条 `.route(...)` | T1 |
| attune-core | `rust/crates/attune-core/src/scanner_patent.rs` | **删整文件**(含内联 5 test) | T2 |
| attune-core | `rust/crates/attune-core/src/lib.rs:156` | 删 `pub mod scanner_patent;` | T2 |
| attune-core | `rust/crates/attune-core/src/resource_governor/profiles.rs` | 删 variant + Display arm + budget 路由 + 2 内联测试 + **F1**(:384 断言/测试名/:336-337 注释) | T3 |
| attune-core | `rust/crates/attune-core/tests/governor_integration.rs:152` | 删 `TaskKind::PatentScanner,` 数组元素(**N3** 自洽) | T3 |
| (doc) | `rust/README.md` / `rust/README.zh.md` | 删 patent **feature** 行(保留 pro-plugin 引用行) | T4 |

依赖顺序(全串行 — 同一文件树删除,无安全并行机会):T1 → T2 → T3 → T4 → T5(全量回归)。
**parallelizable_groups: 无**(纯删除单 worktree,T1-T4 都改 build 状态,顺序删确保每步 build 可验;T5 依赖全部前置)。

---

### Task 1: 删 patent route(server 端叶子,无下游消费者)

**model_tier: sonnet**(删 route 需核 `routes/mod.rs` + `lib.rs` 注册点 dangling — 删错会编译红;非纯机械)

**Files:**
- Delete: `rust/crates/attune-server/src/routes/patent.rs`(整文件 147 行,spec A.1;含 `patent.rs:1` npu-vault 旧仓名 V6 注释,随文件删消失)
- Modify: `rust/crates/attune-server/src/routes/mod.rs:28`(删 `pub mod patent;`)
- Modify: `rust/crates/attune-server/src/lib.rs:178-179`(删 2 条 `.route(...)`)

- [ ] **Step 1: 「失败测试」= 删前确认 patent route 存在(RED-equivalent)**

```bash
df -h /data   # 磁盘前置: 确认 > 50G
cd /data/company/project/attune
grep -rn "routes::patent\|/api/v1/patent\|pub mod patent" \
  rust/crates/attune-server/src/lib.rs \
  rust/crates/attune-server/src/routes/mod.rs
```
Expected(删前 = RED,patent 仍存在):
```
rust/crates/attune-server/src/lib.rs:178:        .route("/api/v1/patent/search", post(routes::patent::search))
rust/crates/attune-server/src/lib.rs:179:        .route("/api/v1/patent/databases", get(routes::patent::databases))
rust/crates/attune-server/src/routes/mod.rs:28:pub mod patent;
```

- [ ] **Step 2: 执行删除(整文件 + 模块声明 + 2 route)**

```bash
cd /data/company/project/attune
git rm rust/crates/attune-server/src/routes/patent.rs
```
然后用 Edit 工具删 `routes/mod.rs:28` 整行 `pub mod patent;`，
再用 Edit 工具删 `lib.rs:178-179` 这两行(精确匹配，删后上下行衔接不留空 route)：
```rust
        .route("/api/v1/patent/search", post(routes::patent::search))
        .route("/api/v1/patent/databases", get(routes::patent::databases))
```
**删后立即 §4.2.3 三步验证：**
```bash
grep -n "^<<<<<<<\|^=======$\|^>>>>>>>" \
  rust/crates/attune-server/src/lib.rs rust/crates/attune-server/src/routes/mod.rs   # 应空
git diff -- rust/crates/attune-server/src/lib.rs rust/crates/attune-server/src/routes/mod.rs | head -40
```

- [ ] **Step 3: 「最小实现」= 删后 grep 转绿 + server crate 编译**

```bash
df -h /data
cd /data/company/project/attune
# grep 转绿: patent route 引用 0 命中
grep -rn "routes::patent\|/api/v1/patent\|pub mod patent" rust/crates/attune-server/src/ ; echo "exit=$?"
```
Expected: 无输出，`exit=1`(grep 0 命中)。

- [ ] **Step 4: 验证 server crate 0 dangling 编译**

Run:
```bash
cd /data/company/project/attune/rust
cargo build -p attune-server --release
```
Expected: PASS(0 error)。**注意**:此时 `scanner_patent` 仍存在于 attune-core(T2 才删),但 patent.rs 是其唯一消费者(spec A.1),删 patent.rs 后 scanner_patent 变 unused(`dead_code` warning 可接受,T2 清除;**不要**为消除 warning 此 task 提前删 scanner_patent — 那破坏 task 边界)。若 `cargo build` 因 unused 报错(项目 `-D warnings` 仅在 clippy 阶段，build 不 deny)→ 正常,T5 clippy 前 T2 已删 scanner_patent。

- [ ] **Step 5: Commit(one-commit per task)**

```bash
cd /data/company/project/attune
git add rust/crates/attune-server/src/routes/mod.rs rust/crates/attune-server/src/lib.rs
git commit -m "$(cat <<'EOF'
refactor(server): remove patent route from OSS — boundary realignment

删 routes/patent.rs(整文件 147 行)+ routes/mod.rs:28 模块声明 + lib.rs:178-179
两条 /api/v1/patent/* route 注册。patent 是行业能力,按 OSS 北极星(零行业绑定)
移出 OSS;能力去向 attune-pro/patent-pro(cross-repo,本仓不实现)。硬删 404 无
alias(grep 确认 OSS 内 0 caller,spec §5 决策)。

scanner_patent 在 T2 删(本 task 后变 unused warning,不在此处理以守 task 边界)。

Spec: docs/superpowers/specs/2026-06-01-oss-patent-migration.md (§2.1 / §4 / A.1)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

**acceptance_judges(机器可验):**
- `grep -rn "routes::patent\|/api/v1/patent\|pub mod patent" rust/crates/attune-server/src/` → 0 命中(exit 1)
- `cargo build -p attune-server --release` → 0 error
- `git show --stat HEAD` 含 patent.rs 删除(`-147` 行级)+ mod.rs + lib.rs，不含任何 S4b 文件(无 agents.registry.toml / agent_flows.toml / case_metadata.rs)
- `ls rust/crates/attune-server/src/routes/patent.rs` → No such file

---

### Task 2: 删 scanner_patent 模块(core 端,patent.rs 已是唯一消费者)

**model_tier: sonnet**(删 core 模块需核 lib.rs 声明 + 确认无第三方 import dangling;内联 5 test 随文件删需确认无外部引用)

**Files:**
- Delete: `rust/crates/attune-core/src/scanner_patent.rs`(整文件 ~380 行,含 `:302 #[cfg(test)] mod tests` 5 个 test，spec A.2;含 `:1` npu-vault 旧仓名 V6 注释，随文件删消失)
- Modify: `rust/crates/attune-core/src/lib.rs:156`(删 `pub mod scanner_patent;`)

- [ ] **Step 1: 「失败测试」= 删前确认 scanner_patent 存在 + 无第三方消费者(T1 后)**

```bash
df -h /data
cd /data/company/project/attune
# 删前: scanner_patent 模块声明仍在(RED-equivalent)
grep -n "pub mod scanner_patent" rust/crates/attune-core/src/lib.rs
# 确认 T1 后无第三方消费者(应仅 scanner_patent.rs 自身 — patent.rs 已删)
grep -rn "scanner_patent\|search_patents\|ingest_patent_records\|PatentQuery\|PatentDatabase" \
  rust/ --include="*.rs" | grep -v "scanner_patent.rs"
```
Expected:
- `lib.rs:156:pub mod scanner_patent;` 命中
- 第二条 grep 0 命中(T1 已删 patent.rs 唯一消费者;若有命中说明遗漏，停手排查)

- [ ] **Step 2: 执行删除(整文件 + 模块声明)**

```bash
cd /data/company/project/attune
git rm rust/crates/attune-core/src/scanner_patent.rs
```
用 Edit 工具删 `lib.rs:156` 整行 `pub mod scanner_patent;`。
**删后立即 §4.2.3 验证:**
```bash
grep -n "^<<<<<<<\|^=======$\|^>>>>>>>" rust/crates/attune-core/src/lib.rs   # 应空
git diff -- rust/crates/attune-core/src/lib.rs
```

- [ ] **Step 3: 「最小实现」= 删后 grep 转绿**

```bash
cd /data/company/project/attune
grep -rn "scanner_patent\|search_patents\|ingest_patent_records\|USPTO" \
  rust/ --include="*.rs" | grep -v "reports/" ; echo "exit=$?"
```
Expected: 无输出，`exit=1`(scanner_patent/search_patents/ingest_patent_records/USPTO 全 0 命中)。

- [ ] **Step 4: 验证 core crate 编译 + scanner_patent 内联 test 消失**

```bash
df -h /data
cd /data/company/project/attune/rust
cargo build -p attune-core --release
cargo test -p attune-core scanner_patent 2>&1 | tail -5
```
Expected:
- build PASS(0 error;此时 `PatentScanner` governor 变体仍在 T3 才删,与 scanner_patent 无 import 关系，不阻塞 build)
- `cargo test ... scanner_patent` → `0 tests run`(内联 5 test 随文件删消失,无残留引用)

- [ ] **Step 5: Commit**

```bash
cd /data/company/project/attune
git add rust/crates/attune-core/src/lib.rs
git commit -m "$(cat <<'EOF'
refactor(core): remove scanner_patent module from OSS — boundary realignment

删 scanner_patent.rs(整文件 ~380 行,含内联 5 test + USPTO_BASE 直连 + 独立
reqwest::blocking::Client)+ lib.rs:156 模块声明。patent.rs(T1 已删)是其唯一
消费者,删除后无 dangling。入库走通用 store.insert_item(.., "patent", ..),无
patent 专属 schema/表/migration,已入库 patent item 保留为普通 item(spec §10)。

Spec: docs/superpowers/specs/2026-06-01-oss-patent-migration.md (§2.4-2.5 / §4 / A.2)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

**acceptance_judges(机器可验):**
- `grep -rn "scanner_patent\|search_patents\|ingest_patent_records\|USPTO" rust/ --include="*.rs" | grep -v reports/` → 0 命中(exit 1)
- `cargo build -p attune-core --release` → 0 error
- `cargo test -p attune-core scanner_patent` → 0 tests run(无残留 test 引用)
- `ls rust/crates/attune-core/src/scanner_patent.rs` → No such file
- `git show --stat HEAD` 仅含 lib.rs(scanner_patent.rs 删除已 git rm),无 S4b 文件

---

### Task 3: 删 TaskKind::PatentScanner 孤儿变体 + wiring + 2 内联 test + F1 计数断言 + N3 集成测试(必须同一 task)

**model_tier: sonnet**(改通用 `resource_governor` 子系统,spec R4;F1 计数耦合 — 删 3 数据行后不同步改 `:384` 断言/测试名/`:336-337` 注释则 `cargo test` runtime RED;match exhaustive 需核;N3 集成测试自洽需核)

**为什么 F1 + 删变体必须同一 task:** spec §2.6(F1) + A.3 — `profiles.rs:359-362` 删 3 个 `PatentScanner` 数据行后,`:384 assert_eq!(cases.len(), 30, ...)` 会 30→27 → **runtime RED**。计数断言、测试名 `all_30_combinations_snapshot`、`:336-337` doc 注释必须与删行同步改,否则 `cargo test` 失败。拆成两个 commit 会留一个 build 绿但 test 红的中间态(违反每 task 测试绿)。

**Files:**
- Modify: `rust/crates/attune-core/src/resource_governor/profiles.rs`(精确 6 处 + F1 4 处，下列逐一)
- Modify: `rust/crates/attune-core/tests/governor_integration.rs:152`(删 `TaskKind::PatentScanner,` 数组元素 — N3)

**profiles.rs 精确删/改清单(实测行号已核对,spec A.3 + 本 plan grounding):**

1. `:26` — 删 enum variant 整行 `    PatentScanner,`
2. `:45` — 删 Display arm 整行 `            Self::PatentScanner => "patent_scanner",`
3. `:158-159` — 删 budget 路由(含注释行):
   ```rust
            // PatentScanner — 与 FileScanner 同档
            (p, PatentScanner) => p.budget_for(FileScanner),
   ```
4. `:320-329` — 删整个内联测试 `fn patent_scanner_inherits_file_scanner`(含 `#[test]` 属性行 + 前置空行):
   ```rust
       #[test]
       fn patent_scanner_inherits_file_scanner() {
           // Spec §6 中 PatentScanner 与 FileScanner 同档 — 验证不漂移
           for profile in [Profile::Conservative, Profile::Balanced, Profile::Aggressive] {
               let p = profile.budget_for(TaskKind::PatentScanner);
               let f = profile.budget_for(TaskKind::FileScanner);
               assert_eq!(p.cpu_pct_max, f.cpu_pct_max);
               assert_eq!(p.ram_bytes_max, f.ram_bytes_max);
           }
       }
   ```
5. `:359-362` — 删 3 个表驱动数据行(含 `// PatentScanner` 注释行):
   ```rust
               // PatentScanner — 与 FileScanner 同档（继承）
               (Profile::Conservative, TaskKind::PatentScanner, 10.0, 256, 1000, None),
               (Profile::Balanced, TaskKind::PatentScanner, 20.0, 512, 500, None),
               (Profile::Aggressive, TaskKind::PatentScanner, 50.0, 1024, 100, None),
   ```
6. **(F1-a) `:384`** — 改计数断言 `30` → `27`:
   ```rust
   // 改前
           assert_eq!(cases.len(), 30, "must cover 3 profiles × 10 kinds");
   // 改后
           assert_eq!(cases.len(), 27, "must cover 3 profiles × 9 kinds");
   ```
7. **(F1-b) `:339`** — 改测试名 `all_30_combinations_snapshot` → `all_27_combinations_snapshot`:
   ```rust
   // 改前
       fn all_30_combinations_snapshot() {
   // 改后
       fn all_27_combinations_snapshot() {
   ```
8. **(F1-c) `:336-337`** — 改 doc 注释 `30→27` / `10→9`:
   ```rust
   // 改前
       /// 全 30 组合 (3 profiles × 10 task kinds) snapshot — 防漂移。
       /// 修改任何预设值都需要同步更新此表 + spec §6 + docs/system-impact.md。
   // 改后
       /// 全 27 组合 (3 profiles × 9 task kinds) snapshot — 防漂移。
       /// 修改任何预设值都需要同步更新此表 + spec §6 + docs/system-impact.md。
   ```

- [ ] **Step 1: 「失败测试」= 删前确认 PatentScanner 全引用点 + F1 计数耦合存在**

```bash
df -h /data
cd /data/company/project/attune
grep -n "PatentScanner\|patent_scanner\|all_30_combinations\|cases.len(), 30\|3 profiles × 10" \
  rust/crates/attune-core/src/resource_governor/profiles.rs
grep -n "PatentScanner" rust/crates/attune-core/tests/governor_integration.rs
```
Expected(删前 = RED,引用全存在): profiles.rs 命中 `:26 :45 :159 :324 :336 :337 :339 :360 :361 :362 :384`(及 test 体内 `PatentScanner`)；governor_integration.rs 命中 `:152`。

- [ ] **Step 2: 执行删除(上列 profiles.rs 8 项 + governor_integration.rs:152，全部用 Edit 工具)**

按上「精确删/改清单」逐项 Edit。governor_integration.rs:152 删整行 `    TaskKind::PatentScanner,`。
**N3 自洽核对(spec §2.6 / non_blocking_recs N3):** governor_integration.rs:163 是 `assert_eq!(snap.len(), kinds.len())` — 删数组元素后 `kinds.len()` 随之 9,断言两边同减,**自洽,无需改 `:163`**。删后确认:
```bash
cd /data/company/project/attune
sed -n '150,165p' rust/crates/attune-core/tests/governor_integration.rs   # 核 :163 仍 snap.len()==kinds.len()
grep -n "^<<<<<<<\|^=======$\|^>>>>>>>" \
  rust/crates/attune-core/src/resource_governor/profiles.rs \
  rust/crates/attune-core/tests/governor_integration.rs   # 应空
```

- [ ] **Step 3: 「最小实现」= 删后 grep 转绿 + match exhaustive 确认**

```bash
cd /data/company/project/attune
grep -rn "PatentScanner\|patent_scanner\|all_30_combinations\|cases.len(), 30\|3 profiles × 10" \
  rust/crates/attune-core/ ; echo "exit=$?"
```
Expected: 无输出，`exit=1`(全 0 命中)。
**match exhaustive 安全(spec G14):** 删 variant + 删 Display arm + 删 budget tuple arm 三者同步,`match self`(as_str)和 `match (self, task)`(budget_for)枚举仍穷尽,编译期保证。

- [ ] **Step 4: 验证编译 + governor 测试全绿(F1 计数 27 + N3 自洽 + R4 其余变体不漂移)**

```bash
df -h /data
cd /data/company/project/attune/rust
cargo build -p attune-core --release
cargo test -p attune-core --test governor_integration 2>&1 | tail -8
cargo test -p attune-core resource_governor 2>&1 | tail -15
```
Expected:
- build PASS(0 error)
- `governor_integration` 全绿(N3 `:163 snap.len()==kinds.len()` 自洽通过)
- `resource_governor` 模块测试全绿,**含 `all_27_combinations_snapshot` PASS**(F1:`cases.len()==27` 断言通过 — 删 3 行 + 改断言同步,无 runtime RED)；`default_profile_is_balanced` 等其余 test 不受影响(R4:FileScanner/WebDavSync/BrowserSearch/AiAnnotator budget 不漂移)

- [ ] **Step 5: Commit**

```bash
cd /data/company/project/attune
git add rust/crates/attune-core/src/resource_governor/profiles.rs \
        rust/crates/attune-core/tests/governor_integration.rs
git commit -m "$(cat <<'EOF'
refactor(governor): remove orphan TaskKind::PatentScanner variant + F1 count sync

删 resource_governor 孤儿变体 PatentScanner(全栈硬删 patent 语义):enum variant
(:26) + Display arm(:45) + budget 路由(:158-159) + 内联 test
patent_scanner_inherits_file_scanner(:320-329) + 表驱动 3 数据行(:359-362) +
governor_integration.rs:152 数组元素。

F1(同 task 强制): 删 3 数据行后同步改 cases.len() 断言 30→27 + 测试名
all_30→all_27_combinations_snapshot + doc 注释 10→9 kinds,否则 cargo test runtime
RED。N3: governor_integration.rs:163 snap.len()==kinds.len() 删后自洽(kinds→9)。

该变体是 dead label — 真实 patent route 走 spawn_blocking 不经 governor(spec A.3);
但触碰通用 governor 子系统,R4 已 §9 T7 验证其余变体不漂移。

Spec: docs/superpowers/specs/2026-06-01-oss-patent-migration.md (§2.6 F1 / A.3)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

**acceptance_judges(机器可验):**
- `grep -rn "PatentScanner\|patent_scanner\|all_30_combinations\|cases.len(), 30\|3 profiles × 10" rust/crates/attune-core/` → 0 命中(exit 1)
- `cargo build -p attune-core --release` → 0 error
- `cargo test -p attune-core resource_governor` → 全绿,含 `all_27_combinations_snapshot` PASS(F1 验证)
- `cargo test -p attune-core --test governor_integration` → 全绿(N3 自洽)
- `git show HEAD -- rust/crates/attune-core/src/resource_governor/profiles.rs` 含 `30→27` / `all_27` / `× 9` 三处 F1 改动(实际 diff 看,不靠 commit msg claim — §4.2.3 R2)

---

### Task 4: 删 README patent feature 行(双语,N1 精确区分 feature vs pro-plugin 引用)

**model_tier: haiku**(纯文档行删除,无 dangling 风险;但 N1 区分 feature 行 vs pro-plugin 引用行需精确 grep 定位)

**N1 关键区分(已 plan-time grep 实测,spec A.4 + non_blocking_recs N1):**
- **删(OSS 可执行 feature 行):**
  - `rust/README.md:56` — `- Real-time USPTO patent search (\`POST /api/v1/patent/search\`)`
  - `rust/README.zh.md:41` — `- USPTO 专利实时检索（\`POST /api/v1/patent/search\`）`
- **保留(attune-pro vertical 引用,正确 framing,**不动**):**
  - `rust/README.md:7`(intro「patent agents」load pro plugin)、`:55`(`Industry plugins (patent / law / tech / presales ...)`)、`:160`(table「Lawyers / Patent agents」)
  - `rust/README.zh.md:40`(`领域插件（patent / law / tech / presales ...)`)
- **判别原则:** patent 作为 OSS **可执行 endpoint feature** 的行 → 删;patent 作为 **attune-pro 行业插件** 的引用行 → 保留(符合 spec §1「能力去向 attune-pro/patent-pro」framing,反而是正确文档)。

**Files:**
- Modify: `rust/README.md:56`(删 USPTO patent search feature 行)
- Modify: `rust/README.zh.md:41`(删对应中文 feature 行)

- [ ] **Step 1: 「失败测试」= 删前 grep 确认 feature 行 + pro-plugin 引用行并存(RED-equivalent)**

```bash
cd /data/company/project/attune
grep -ni "patent\|uspto" rust/README.md rust/README.zh.md
```
Expected(删前): README.md 命中 `:7 :55 :56 :160`；README.zh.md 命中 `:40 :41`。确认 `:56`(en)/`:41`(zh)是 USPTO feature 行,`:55`/`:40` 是 pro-plugin 引用行(保留)。

- [ ] **Step 2: 执行删除(仅 feature 行,双语)**

用 Edit 工具删 `rust/README.md` 这一行:
```
- Real-time USPTO patent search (`POST /api/v1/patent/search`)
```
用 Edit 工具删 `rust/README.zh.md` 这一行:
```
- USPTO 专利实时检索（`POST /api/v1/patent/search`）
```
**§4.2.3 验证:**
```bash
cd /data/company/project/attune
grep -n "^<<<<<<<\|^=======$\|^>>>>>>>" rust/README.md rust/README.zh.md   # 应空
git diff -- rust/README.md rust/README.zh.md
```

- [ ] **Step 3: 「最小实现」= 删后 grep 转绿(feature 行消失,pro 引用保留)**

```bash
cd /data/company/project/attune
grep -ni "USPTO\|/api/v1/patent" rust/README.md rust/README.zh.md ; echo "exit=$?"
grep -ni "patent" rust/README.md rust/README.zh.md
```
Expected:
- 第一条(USPTO / endpoint feature): 0 命中，`exit=1`
- 第二条(patent 泛指): 仅剩 pro-plugin 引用行(README.md `:7 :55 :160`，README.zh.md `:40` — 行号可能因删行上移,内容为「patent agents / Industry plugins (patent...)」即对)

- [ ] **Step 4: 验证双语 README 一致(§1.1.3 不漂移)**

```bash
cd /data/company/project/attune
# 双语 patent 行计数应一致(各保留同数量 pro-plugin 引用,均删 1 feature 行)
echo "en pro-refs:"; grep -ci "patent" rust/README.md
echo "zh pro-refs:"; grep -ci "patent" rust/README.zh.md
```
Expected: en 保留 3 行(`:7 :55 :160`)、zh 保留 1 行(`:40` — 中文 README 本就更精简,intro/table 无独立 patent 行)。**判据:USPTO/endpoint 行双语均 0;pro-plugin 引用行双语均保留(无一侧漏删 feature 行或误删 pro 引用)。** 若 en 仍有 USPTO 行或 zh 仍有 `专利实时检索` → 漏删,回 Step 2。

- [ ] **Step 5: Commit**

```bash
cd /data/company/project/attune
git add rust/README.md rust/README.zh.md
git commit -m "$(cat <<'EOF'
docs(readme): drop USPTO patent search feature line — patent moved to pro

删 rust/README.md / rust/README.zh.md 中 OSS 可执行 USPTO patent search feature 行
(防 §7.2 Gate1 doc 漂移)。保留「patent agents 装 attune-pro 插件」「Industry
plugins (patent/law/...)」等 pro-vertical 引用行 — 那是正确 framing(patent 能力去向
attune-pro/patent-pro)。双语两份同步(§1.1.3)。

Spec: docs/superpowers/specs/2026-06-01-oss-patent-migration.md (§2.9 / A.4 / N1)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>
EOF
)"
```

**acceptance_judges(机器可验):**
- `grep -ni "USPTO\|/api/v1/patent" rust/README.md rust/README.zh.md` → 0 命中(exit 1)
- `grep -ci "patent" rust/README.md` → 3(pro-plugin 引用行保留);`grep -ci "patent" rust/README.zh.md` → 1(pro-plugin 引用行保留)
- `git show --stat HEAD` 仅含 README.md + README.zh.md，各 `-1` 行
- `git diff HEAD~1 HEAD -- rust/README.md | grep '^-'` 仅删 USPTO feature 行,不删 `Industry plugins` / `Patent agents` 行(实看 diff，§4.2.3 R2)

---

### Task 5: 全量回归 + grep gate + clippy(GA gate 收口,无代码改动)

**model_tier: sonnet**(全量 workspace build/test/clippy 结果判读 + grep gate 人工区分 corpus_domain/plugin-id 保留命中 — 需理解 spec §8 命令 1 判据,非纯机械)

**Files:** 无改动(纯验证收口 task)。若发现遗漏 → 回对应 T1-T4 task 修(不在本 task 改代码)。

- [ ] **Step 1: 磁盘前置 + 全量编译**

```bash
df -h /data   # 黄线 188G;build 增量极小
cd /data/company/project/attune/rust
cargo build --release --workspace
```
Expected: PASS(0 error;spec §9 T2)。

- [ ] **Step 2: 全量回归测试(spec §9 T10)**

Run:
```bash
cd /data/company/project/attune/rust
cargo test --workspace 2>&1 | tail -30
```
Expected: 全绿(0 fail);`#[ignore]` 个数不突增(§7.2 Gate2,删除 sprint 不应新增 ignore)。**重点核**:`all_27_combinations_snapshot` PASS(F1)、`governor_integration` PASS(N3)、通用 endpoint 测试 PASS(spec §9 T1)、已入库 `source="patent"` item 走通用 search 不报错(§9 T3,若有相关集成测试)。

- [ ] **Step 3: clippy 干净(spec §9 T9,删 governor 变体后无 dead_code/unreachable)**

Run:
```bash
cd /data/company/project/attune/rust
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -20
```
Expected: 0 warning(无 dead_code / unused_import 残留;删 PatentScanner variant + Display arm + budget arm 同步,无 unreachable;删 scanner_patent 后 attune-server 无 unused import)。

- [ ] **Step 4: grep gate — patent 可执行残留 0 命中(spec §8 命令 1 / §9 T11)**

Run:
```bash
cd /data/company/project/attune
grep -rn "scanner_patent\|routes::patent\|/api/v1/patent\|PatentQuery\|search_patents\|PatentScanner\|USPTO" \
  rust/ --include="*.rs" --include="*.toml" | grep -v reports/
```
Expected: **0 patent 可执行命中**。允许保留(spec §2 灰区 + S4b,**人工区分**,不算残留):
- `corpus_domain="patent"` 数据标签:`ingest/connector.rs:72`(注释「legal/tech/medical/patent/general」)、`store/dirs.rs:19`、`search.rs:140-143`(cross-domain penalty 'patent')
- `patent_pro` plugin-id:`tests/generic_plugins_test.rs:74,103,125,193`(S4b 范围)

判据:无 `scanner_patent` / `routes::patent` / `/api/v1/patent` mount / `search_patents` / `PatentScanner` / `USPTO` 命中。上述 grep pattern 不含 `corpus_domain`/`patent_pro`,故理论 0 输出;若 `patent` 子串意外命中 corpus_domain 行(因 pattern 无 `corpus_domain` 但含其他)→ 人工确认是数据标签即放行。

- [ ] **Step 5: README doc 漂移检查(spec §8 命令 3 / §9 T12)+ commit gate 报告**

Run:
```bash
cd /data/company/project/attune
grep -ni "patent\|uspto" rust/README.md rust/README.zh.md
```
Expected: 仅剩 pro-plugin 引用行(无 USPTO / endpoint feature 行)。

本 task 无代码改动 → **不产生 commit**(若 Step 1-5 全绿)。若任一 step fail → 回对应 T1-T4 修复后重跑本 task。全绿即 sprint 实施完成,**handoff 给 release/RELEASE.md 写入要点**(spec §10:Highlights / Breaking `/api/v1/patent/*` removed / Migration / Known Limitations patent 需 patent-pro)。

**acceptance_judges(机器可验):**
- `cargo build --release --workspace` → 0 error(§9 T2)
- `cargo test --workspace` → 0 fail，含 `all_27_combinations_snapshot` + `governor_integration` PASS;`#[ignore]` 不突增(§9 T10)
- `cargo clippy --workspace --all-targets -- -D warnings` → 0 warning(§9 T9)
- `grep -rn "scanner_patent\|routes::patent\|/api/v1/patent\|search_patents\|PatentScanner\|USPTO" rust/ --include="*.rs" --include="*.toml" | grep -v reports/` → 0 命中(§8 命令 1 / §9 T11)
- `grep -ni "USPTO\|/api/v1/patent" rust/README.md rust/README.zh.md` → 0 命中(§9 T12)

---

## Risk Register Carry-Over（spec §11 → task 映射）

| spec 风险 | affected tasks | 本 plan 监控/缓解 |
|-----------|---------------|------------------|
| **R1** 硬删 breaking,外部脚本 curl 收 404 | T1 | T1 删 route 后即 404;RELEASE.md Breaking+Migration(T5 handoff)。grep 确认 OSS 0 caller(T1 Step 1) |
| **R2** 跨仓能力 gap(patent-pro 未接住) | (T5 handoff) | 本仓不可验证 pro 状态;RELEASE.md Known Limitations(T5 Step 5 handoff);**非 OSS 删除技术阻塞** |
| **R3** dangling ref 遗漏 → build 红 | T1/T2/T3 | 每 task Step 3 grep 转绿 + Step 4 crate build;T5 全量 build(§9 T2/T10) |
| **R4** 改通用 governor 漂移 | **T3** | T3 精确限定 6+F1 处;T3 Step 4 跑 `resource_governor` 全测(其余变体 budget 不漂移);T5 全量回归 |
| **R5** 删测试遗漏 → 编译红/引用悬空 | T2/T3 | T2 内联 5 test 随文件删(Step 4 核 0 tests run);T3 2 内联 test + governor_integration:152 手删 + F1 计数同步 |
| **R6** 删 scanner blocking client 影响其他 client | T2 | scanner_patent 自建独立 client(spec A.2 :193);T2 Step 4 core build 验证无共享 |
| **R7** 多文件删除 conflict-marker | T1/T2/T3/T4 | 每 task Step 2 后强制 §4.2.3 三步(`grep <<<<<<<` + `git diff` + 逐文件核);优先 git rm/Edit 不用 stash+reset |
| **R8** doc 漂移(README 未删) | **T4** | T4 删 feature 行;T5 Step 5 审计命令 3;双语同步(T4 Step 4) |
| **R9** clippy dead_code/unreachable | T3 | T3 同步删 enum/arm/budget/test 无半删;T5 Step 3 clippy `-D warnings` |
| **R10** grep gate 误判 corpus_domain/plugin-id | **T5** | T5 Step 4 注释明示人工区分;判据只针对 route/scanner/governor/USPTO,corpus_domain/patent_pro 是 §2 灰区保留 |

---

## Commit 分批顺序（one-commit per task）

1. **T1** `refactor(server): remove patent route from OSS` — server route 叶子先删
2. **T2** `refactor(core): remove scanner_patent module from OSS` — core scanner(patent.rs 唯一消费者已删)
3. **T3** `refactor(governor): remove orphan TaskKind::PatentScanner variant + F1 count sync` — governor 孤儿变体 + F1 + N3 同 commit
4. **T4** `docs(readme): drop USPTO patent search feature line — patent moved to pro` — 双语 README feature 行
5. **T5** 无 commit(全量回归 gate;全绿 handoff RELEASE.md;fail 则回 T1-T4)

跨仓注记:patent-pro 落 attune-pro 是外部前置,本 sprint 不阻塞、不产生本仓 commit。
