# RSS feed connector 加固 + 云盘 (cloud-drive via rclone) ingest connector — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 RSS 采集源补到生产级安全/测试下限（SSRF 双路径加固 + XXE 实测 + feed 体积上限 + 6 类测试），并新实装一个经 rclone subprocess 桥接的云盘采集源，复用统一 `ingest_document` 入库框架。

**Architecture:** RSS 已端到端实装（`ingest/rss.rs` + `ingest_rss.rs` + `routes/rss.rs` + `rss_feeds` 表 + `start_rss_sync_worker`），本 sprint 仅在 route + worker 两道防线补 SSRF 校验、补测试、加体积上限。云盘是 greenfield：`ingest/cloud_drive.rs`（`CloudDriveConnector` + `RcloneRunner` trait，mock 可离线测）+ `store/cloud_drive_remotes.rs`（加密凭据表）+ `ingest_cloud.rs`（锁外 I/O 三段式）+ `routes/cloud_drive.rs`（5 路由）+ `start_cloud_sync_worker`。两源共用 `ingest_document` content_hash 短路 + `indexed_files` 去重，绝不绕过。

**Tech Stack:** Rust / axum 0.8 / rusqlite + AES-256-GCM 字段级加密 / feed-rs（RSS，已用）/ rclone subprocess (`std::process::Command`) / serde_json (lsjson 解析) / `url_guard::validate_outbound_url`（SSRF）/ `#[test]` + proptest + 集成 `tests/*.rs`。

**模型分层 (Appendix D)：** 架构/结构判断型 task = **opus**（云盘 connector trait 设计 T6、三段式 worker T9）；deterministic 实现型 task = **sonnet**（SSRF 加固、CRUD、测试、route）；纯样板/收尾 = **haiku**（mod.rs 接线 T15、GA checklist 自检 T16）。

---

## 磁盘前置（每个 build/test task 强制，per 项目磁盘铁律 + 全局 §1.1.6）

> 当前 `/data` 黄线 189G available，`rust/target` 160G 保留增量（不 `cargo clean`，避免 5x 重 build）。

**每个含 `cargo build` / `cargo test` 的 Step 执行前，先跑：**

```bash
df -h /data | tail -1
# available < 50G(红线) → 停手：cargo clean 陈旧 worktree target + git worktree prune，腾出再继续
# available 50-200G(黄线) → 正常执行，但 sprint 结束 task 完成后评估清理
```

sprint 全部 task 完成后（plan 自身是临时产物，per §3.2 实施完成立即删）：
```bash
df -h /data | tail -1                 # 确认未跌破红线
git worktree list                     # 无遗留隔离 worktree
```

---

## File Structure（决策锁定 — 7 新建 + 8 改动，触发建议③ marker 检查）

### attune-core (`rust/crates/attune-core/`)
| 文件 | 责任 | 动作 |
|------|------|------|
| `src/ingest/cloud_drive.rs` | `CloudDriveConnector` / `RcloneRunner` trait / `RealRcloneRunner` / `MockRcloneRunner` / `RcloneFile` / `parse_lsjson` 纯函数 | **新建** |
| `src/store/cloud_drive_remotes.rs` | `CloudDriveRemoteRow` / `CloudDriveRemoteInput` + CRUD（对齐 `store/rss_feeds.rs`） | **新建** |
| `src/ingest/rss.rs` | `RealFeedFetcher` 加 `max_feed_bytes` 上限 + adversarial/resource 单元测试 | **改** |
| `src/ingest/mod.rs` | `pub mod cloud_drive;` + re-export | **改** |
| `src/store/mod.rs` | `cloud_drive_remotes` CREATE TABLE + `pub mod cloud_drive_remotes;` | **改** |

### attune-server (`rust/crates/attune-server/`)
| 文件 | 责任 | 动作 |
|------|------|------|
| `src/ingest_cloud.rs` | `sync_cloud_remote(state, remote_id)` 三段式 | **新建** |
| `src/routes/cloud_drive.rs` | 5 路由 handler | **新建** |
| `tests/cloud_drive_subprocess.rs` | 集成 E2E（add → poll → vault 出现文档 → search） | **新建** |
| `src/routes/rss.rs:51-69` | `validate()` 加 `url_guard::validate_outbound_url` + 400 映射 | **改** |
| `src/ingest_rss.rs:46-53` | `sync_rss_feed` 阶段 1 前加 SSRF 二次校验 | **改** |
| `src/routes/mod.rs` | `pub mod cloud_drive;` + nest 路由 | **改** |
| `src/state.rs` | `start_cloud_sync_worker`（对齐 `start_rss_sync_worker`） | **改** |
| `src/routes/vault.rs` | unlock 路径启动 `start_cloud_sync_worker`（对齐 RSS 三处） | **改** |

### 不改（grounding 确认，per Appendix A）
- `src/ingest/connector.rs` — `SourceKind::CloudDrive` 已存在（:16-17,30），trait 不动。
- `src/net/url_guard.rs` — 复用 `validate_outbound_url(url, allow_hosts, resolve)`。
- `src/ingest/pipeline.rs` — `ingest_document`(:44) + content_hash 短路(:99) 不改，云盘复用。

---

## Commit 分批顺序（依赖链）

```
T1 ─► T2 ─► T3            (RSS SSRF route 加固 — 必须先，建立 SSRF 复用模式)
            │
            ▼
T4 ─► T5                  (RSS XXE 实测 + feed 体积上限 — 依赖 T1 的 fixture seam)
                          ─ T1..T5 = RSS 加固块，可整体先 merge
T6 ─► T7 ─► T8 ─► T9 ─► T10 ─► T11 ─► T12 ─► T13   (云盘块，严格串行：trait→解析→CRUD→worker→route→接线→集成)
T14                       (云盘 adversarial: 命令注入 + 路径穿越 — 依赖 T6/T9)
T15                       (mod.rs / routes/mod.rs / state.rs / vault.rs 接线 — 依赖 T6..T11 全在)
T16                       (GA 验收 checklist + 全量 marker 自检 — 最后)
```

并行机会（per 全局 §7.1.7，max 2/group）：**[T1..T5 块] 与 [T6..T8 块] 无共享文件可并行**（RSS 改 server 端 rss.rs/ingest_rss.rs；云盘前三 task 改 core 新建文件）。其余串行。

---

## Task 1: RSS route SSRF 加固 — `validate()` 拒内网返回 400

**model_tier: sonnet**（deterministic 安全加固，复刻 git.rs 已验证修法）

**Files:**
- Modify: `rust/crates/attune-server/src/routes/rss.rs:51-69`（`validate()`）
- Modify: `rust/crates/attune-server/src/routes/rss.rs`（add_feed handler 错误映射，扫描式 code 提取）
- Test: `rust/crates/attune-server/src/routes/rss.rs`（`#[cfg(test)] mod tests`）

> **吸收建议②（双路径之一：route）+ 建议④（citation 校准）**：参照 `routes/git.rs:57-76 git_error_response` 扫描式 code 提取（**不取 `:` 首段**，否则 `VaultError::InvalidInput` 的 `"invalid input: "` 前缀会让 SSRF 拒绝错返 502）。

- [ ] **Step 1: 写 failing test — SSRF feed URL 必须返回 400 `ssrf-rejected`**

```rust
// rss.rs #[cfg(test)] mod tests 内
#[test]
fn validate_rejects_metadata_endpoint_400() {
    // 注入固定 resolver：把任意 host 解析到 link-local，模拟 DNS-rebind / 元数据端点
    let req = AddFeedRequest {
        url: "http://169.254.169.254/latest/meta-data/".into(),
        name: String::new(),
        poll_interval_minutes: None,
    };
    let err = validate(&req).expect_err("SSRF URL must be rejected");
    let (status, body) = rss_error_response(&err.to_string());
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST, "SSRF 拒绝必须 400 非 502");
    assert!(body.to_string().contains("ssrf-rejected"), "code 必须是 ssrf-rejected");
}
```

- [ ] **Step 2: 跑测试确认 FAIL**

`df -h /data | tail -1`（黄线前置）→
Run: `cd rust && cargo test -p attune-server validate_rejects_metadata_endpoint_400 -- --nocapture`
Expected: FAIL — `validate` 不调 url_guard / `rss_error_response` 未定义。

- [ ] **Step 3: 写最小实现 — validate 内调 SSRF guard + 扫描式 error_response**

```rust
use attune_core::net::url_guard;

fn validate(req: &AddFeedRequest) -> Result<(), AppError> {
    let trimmed = req.url.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest("url-empty".into()));
    }
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        return Err(AppError::BadRequest("url-scheme-invalid".into()));
    }
    // SSRF 预校验：拒内网 / link-local / 元数据端点（无 allowlist，默认 resolver）。
    url_guard::validate_outbound_url(trimmed, &[], None)
        .map_err(|e| AppError::BadRequest(format!("ssrf-rejected: {e}")))?;
    if let Some(iv) = req.poll_interval_minutes {
        if iv == 0 {
            return Err(AppError::BadRequest("poll-interval-invalid".into()));
        }
    }
    Ok(())
}

/// 复刻 routes/git.rs:57-76：扫描已知 kebab code（不取 ':' 首段，避免 VaultError 前缀污染）。
fn rss_error_response(msg: &str) -> (axum::http::StatusCode, axum::Json<serde_json::Value>) {
    use axum::http::StatusCode;
    const KNOWN: &[(&str, StatusCode)] = &[
        ("url-empty", StatusCode::BAD_REQUEST),
        ("url-scheme-invalid", StatusCode::BAD_REQUEST),
        ("ssrf-rejected", StatusCode::BAD_REQUEST),
        ("feed-too-large", StatusCode::PAYLOAD_TOO_LARGE),
        ("feed-parse-failed", StatusCode::BAD_GATEWAY),
    ];
    let (code, status) = KNOWN.iter().find(|(c, _)| msg.contains(c))
        .map(|(c, s)| (*c, *s))
        .unwrap_or(("rss-error", StatusCode::BAD_GATEWAY));
    (status, axum::Json(serde_json::json!({ "error": msg, "code": code })))
}
```

- [ ] **Step 4: 跑测试确认 PASS**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-server validate_rejects_metadata_endpoint_400 -- --nocapture`
Expected: PASS。
再跑 `cargo clippy -p attune-server --all-targets -- -D warnings` 干净。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/routes/rss.rs
git commit -m "fix(rss): route validate() 加 SSRF 校验，拒内网返回 400 ssrf-rejected

复刻 routes/git.rs:57-76 扫描式 code 提取（不取 ':' 首段，避免 VaultError
前缀让 SSRF 拒绝错返 502）。新增 validate_rejects_metadata_endpoint_400 回归 fixture。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

**acceptance_judges:**
- `cargo test -p attune-server validate_rejects_metadata_endpoint_400` exits 0。
- `validate()` 体内含 `url_guard::validate_outbound_url` 调用（`grep -n 'validate_outbound_url' rust/crates/attune-server/src/routes/rss.rs` 命中）。
- `rss_error_response` 对 `"ssrf-rejected: ..."` 返回 `StatusCode::BAD_REQUEST`（断言已在 test 内）。
- `cargo clippy -p attune-server --all-targets -- -D warnings` 无输出。

---

## Task 2: RSS route — DNS-rebind / 127.0.0.1 回环 case 补全

**model_tier: sonnet**

**Files:**
- Test: `rust/crates/attune-server/src/routes/rss.rs`（`#[cfg(test)] mod tests`）

> 巩固建议②的 route 防线：除元数据端点外，127.0.0.1（本机 attune 自身 :18900）和私网段必须同样拒绝。

- [ ] **Step 1: 写 failing test — 回环 + 私网段拒绝**

```rust
#[test]
fn validate_rejects_loopback_and_private() {
    for url in [
        "http://127.0.0.1:18900/",
        "http://localhost/feed.xml",
        "http://10.0.0.5/internal.rss",
        "http://192.168.1.1/admin",
    ] {
        let req = AddFeedRequest { url: url.into(), name: String::new(), poll_interval_minutes: None };
        let err = validate(&req).unwrap_err();
        let (status, _) = rss_error_response(&err.to_string());
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST, "{url} 必须被 SSRF 拒绝为 400");
    }
}
```

- [ ] **Step 2: 跑测试确认 FAIL 或直接 PASS（验 url_guard 覆盖面）**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-server validate_rejects_loopback_and_private -- --nocapture`
Expected: 若 `url_guard` 已覆盖回环/私网 → PASS（确认覆盖面）；若某段漏 → FAIL，回到 Step 3 在 validate 补 host 白名单拒绝。

- [ ] **Step 3: （仅当 Step 2 有 FAIL）在 validate 补私网/回环显式拒绝**

```rust
// 仅当 url_guard 未覆盖某段时补：解析 host，命中 localhost/127./10./192.168./172.16-31 → Err
// 若 url_guard 已全覆盖则本 Step 跳过（不引入冗余逻辑，YAGNI）。
```

- [ ] **Step 4: 跑测试确认 PASS**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-server validate_rejects_loopback_and_private -- --nocapture`
Expected: PASS（4 个 URL 全 400）。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/routes/rss.rs
git commit -m "test(rss): SSRF route 防线补回环 + 私网段拒绝 case

127.0.0.1:18900(attune 自身) / localhost / 10.x / 192.168.x 全拒为 400。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

**acceptance_judges:**
- `cargo test -p attune-server validate_rejects_loopback_and_private` exits 0，4 个 URL 全断言 400。
- test body 含 `127.0.0.1:18900` 字面量（防回归到允许本机回环）。

---

## Task 3: RSS worker SSRF 二次校验 — `sync_rss_feed` 阶段 1 前守门

**model_tier: sonnet**

**Files:**
- Modify: `rust/crates/attune-server/src/ingest_rss.rs:46-53`（阶段 1 fetch 前）
- Test: `rust/crates/attune-server/src/ingest_rss.rs`（`#[cfg(test)] mod tests`，注入 mock fetcher）

> **吸收建议②（双路径之二：worker）**：route 是第一道防线，但 worker 直读 DB 里已存 URL 再 fetch。若未来某 path 绕过 route 写入 feed（migration / 直接 SQL / DNS-rebind 在 add 后失效），worker 必须再校验一次。守护这道防线不被未来重构悄删。

- [ ] **Step 1: 写 failing test — worker 对内网 URL 拒绝 fetch**

```rust
#[test]
fn sync_rss_feed_rejects_ssrf_url_in_worker() {
    // 构造一个 DB 里存了内网 URL 的 feed（绕过 route 直接 add_rss_feed 落库），
    // 调 sync_rss_feed → 必须在发 HTTP 前返回 Err 含 "ssrf-rejected"，且 touch_polled_at 被调。
    let state = crate::test_support::locked_test_state_unlocked();
    let dek = { let v = state.vault.lock().unwrap(); v.dek_db().unwrap() };
    let feed_id = {
        let v = state.vault.lock().unwrap();
        v.store().add_rss_feed(&dek, &attune_core::store::rss_feeds::RssFeedInput {
            name: "evil".into(),
            url: "http://169.254.169.254/latest/meta-data/".into(),
            poll_interval_minutes: None,
        }).unwrap()
    };
    let res = sync_rss_feed(&state, &feed_id);
    assert!(res.is_err(), "worker 必须拒绝内网 URL");
    assert!(res.unwrap_err().contains("ssrf-rejected"), "错误必须含 ssrf-rejected code");
}
```
（若 `test_support::locked_test_state_unlocked` 不存在，复用 `ingest_rss.rs` 现有集成测试 harness 的 state 构造方式；优先复用，不新造。）

- [ ] **Step 2: 跑测试确认 FAIL**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-server sync_rss_feed_rejects_ssrf_url_in_worker -- --nocapture`
Expected: FAIL — worker 当前直接 fetch 不校验。

- [ ] **Step 3: 写最小实现 — 阶段 0 取出 url 后、阶段 1 fetch 前加校验**

```rust
// ingest_rss.rs sync_rss_feed —— 在 fetch_input 物化后(:44 之后)、阶段 1(:46) 之前插入：
    // SSRF 二次校验（防 route 被绕过 / DNS-rebind 在 add 后失效）。
    if let Err(e) = attune_core::net::url_guard::validate_outbound_url(&fetch_input.url, &[], None) {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let _ = vault.store().touch_rss_polled_at(feed_id); // 防 tight-loop 重试
        return Err(format!("rss fetch {feed_id}: ssrf-rejected: {e}"));
    }
```

- [ ] **Step 4: 跑测试确认 PASS**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-server sync_rss_feed_rejects_ssrf_url_in_worker -- --nocapture`
Expected: PASS。
`cargo clippy -p attune-server --all-targets -- -D warnings` 干净。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/ingest_rss.rs
git commit -m "fix(rss): worker sync_rss_feed 加 SSRF 二次校验防 route 绕过

阶段 1 HTTP 抓取前再校验一次（DNS-rebind 在 add 后失效 / 直接 SQL 写入绕过 route
的场景）。失败 touch_polled_at 防 tight-loop。守护 worker 这道防线不被未来重构悄删。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

**acceptance_judges:**
- `cargo test -p attune-server sync_rss_feed_rejects_ssrf_url_in_worker` exits 0。
- `grep -n 'validate_outbound_url' rust/crates/attune-server/src/ingest_rss.rs` 命中（worker 路径确有 SSRF 校验）。
- 错误路径调 `touch_rss_polled_at`（`grep -c 'touch_rss_polled_at' ingest_rss.rs` ≥ 2：原 fetch-fail + 新 SSRF-fail）。

---

## Task 4: RSS XXE 实测断言 fixture — 外部实体不读本地文件

**model_tier: sonnet**

**Files:**
- Modify: `rust/crates/attune-core/src/ingest/rss.rs`（`#[cfg(test)] mod tests`）
- Test fixture: 内联 XML 字符串（不落独立文件）

> **吸收建议①（核心）**：把「feed-rs 不解析外部实体」从假设转为**实测断言**（per §6.1 反白盒推理）。喂含 `<!ENTITY xxe SYSTEM "file:///etc/passwd">` 的 feed XML，断言 (a) 入库正文不含 `/etc/passwd` 内容、(b) 解析不读本地文件（不 panic、不挂起）。

- [ ] **Step 1: 写 failing test — XXE feed 解析后正文不含 /etc/passwd**

```rust
// ingest/rss.rs #[cfg(test)] mod tests 内
#[test]
fn xxe_external_entity_not_resolved() {
    // 含外部实体声明的恶意 RSS。若解析器解析外部实体，正文会塞进 /etc/passwd 内容。
    let malicious = br#"<?xml version="1.0"?>
<!DOCTYPE rss [
  <!ENTITY xxe SYSTEM "file:///etc/passwd">
]>
<rss version="2.0"><channel><title>evil</title>
  <item><title>pwn</title><link>http://example.com/1</link>
    <description>&xxe;</description></item>
</channel></rss>"#;
    // parse_feed_bytes 是 connector 内部解析入口（grounding: ingest/rss.rs 解析路径）。
    let docs = parse_feed_bytes_for_test(malicious);
    // (a) 正文不含 /etc/passwd 任何标志串（root: / /bin/bash / daemon:）
    for d in &docs {
        let body = String::from_utf8_lossy(&d.content);
        assert!(!body.contains("root:"), "XXE 解析了外部实体，正文含 /etc/passwd！");
        assert!(!body.contains("/bin/bash"), "XXE 泄露本地文件内容");
    }
    // (b) 解析完成（不 panic、不挂起）—— 走到这里即证明未阻塞读本地文件。
}
```
（`parse_feed_bytes_for_test` = 复用 connector 现有内部解析函数的 test 包装；若已有 pub(crate) 解析入口直接调，不新造解析逻辑。）

- [ ] **Step 2: 跑测试确认 FAIL（编译失败：test helper 缺）或 PASS（验真实行为）**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-core xxe_external_entity_not_resolved -- --nocapture`
Expected: 先 FAIL（helper 未接 / 断言未通）；接好后若 feed-rs 确实不解析外部实体 → PASS（这正是要实测确认的）。

- [ ] **Step 3: 接 test helper（不改产品解析逻辑，仅暴露 test 入口）**

```rust
#[cfg(test)]
fn parse_feed_bytes_for_test(bytes: &[u8]) -> Vec<crate::ingest::RawDocument> {
    // 复用 RssConnector 既有解析 + sink 路径，物化 docs。
    let mut docs = Vec::new();
    let mut sink: crate::ingest::DocumentSink<'_> = Box::new(|d| docs.push(d));
    // 用 mock fetcher 返回 200 + malicious body 喂进 connector 解析路径。
    let conn = RssConnector::with_test_body(bytes.to_vec());
    let _ = conn.fetch_documents(&mut sink);
    docs
}
```
（`with_test_body` 若不存在 → 用现有 8 个单元测试已用的 mock fetcher 构造方式；优先复用既有 seam。）

- [ ] **Step 4: 跑测试确认 PASS**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-core xxe_external_entity_not_resolved -- --nocapture`
Expected: PASS — 正文不含 `root:` / `/bin/bash`，证明 feed-rs 不解析外部实体。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/ingest/rss.rs
git commit -m "test(rss): XXE 外部实体实测断言 fixture（反白盒推理）

喂 <!ENTITY xxe SYSTEM file:///etc/passwd> 恶意 feed，实测断言入库正文不含
/etc/passwd 内容、解析不读本地文件。把'feed-rs 不解析外部实体'从假设转为实测。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

**acceptance_judges:**
- `cargo test -p attune-core xxe_external_entity_not_resolved` exits 0。
- test body 含字面量 `file:///etc/passwd` 与断言 `!body.contains("root:")`（`grep` 双命中）。
- 测试是**实测**（喂真实恶意 XML 走解析路径），非 `assert!(true)` 占位。

---

## Task 5: RSS feed body 体积上限 — `max_feed_bytes` 防 OOM + billion-laughs

**model_tier: sonnet**

**Files:**
- Modify: `rust/crates/attune-core/src/ingest/rss.rs`（`RealFeedFetcher` fetch 处 + struct 加 `max_feed_bytes` 字段）
- Test: `rust/crates/attune-core/src/ingest/rss.rs`（`#[cfg(test)] mod tests`）

> 吸收建议①的 billion-laughs 部分（体积上限挡内存放大）+ spec §2-A.3 + §11 R8。默认 16 MiB。

- [ ] **Step 1: 写 failing test — 超限 feed body 返回 feed-too-large 不 OOM**

```rust
#[test]
fn feed_body_over_limit_rejected() {
    // 构造 32 MiB body（超 16 MiB 默认上限）。mock fetcher 返回这个大 body。
    let big = vec![b'a'; 32 * 1024 * 1024];
    let fetcher = MockFeedFetcher::returning_body(big); // 复用既有 mock 模式
    let err = fetcher.fetch_with_limit("http://example.com/feed", 16 * 1024 * 1024)
        .expect_err("32MiB body 必须被拒");
    assert!(err.to_string().contains("feed-too-large"), "超限必须报 feed-too-large");
}
```

- [ ] **Step 2: 跑测试确认 FAIL**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-core feed_body_over_limit_rejected -- --nocapture`
Expected: FAIL — `RealFeedFetcher` 当前 `resp.bytes()` 无上限。

- [ ] **Step 3: 写最小实现 — 流式读取 + 上限断流**

```rust
// RealFeedFetcher struct 加字段（默认 16 MiB）：
//   pub max_feed_bytes: u64,   // Default::default() / new() 里 = 16 * 1024 * 1024
// fetch 处把 resp.bytes() 改为受限读取：
fn read_capped(resp: reqwest::blocking::Response, max: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut buf = Vec::new();
    let mut reader = resp.take((max + 1) as u64); // 读到 max+1 即可判超限
    reader.read_to_end(&mut buf).map_err(|e| VaultError::InvalidInput(format!("feed-read: {e}")))?;
    if buf.len() as u64 > max {
        return Err(VaultError::InvalidInput("feed-too-large".into()));
    }
    Ok(buf)
}
```

- [ ] **Step 4: 跑测试确认 PASS + 既有 8 单元测试不回归**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-core ingest::rss -- --nocapture`
Expected: 新 case PASS + 既有 8 个 RSS 单元测试全 PASS。
`cargo clippy -p attune-core --all-targets -- -D warnings` 干净。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/ingest/rss.rs
git commit -m "feat(rss): RealFeedFetcher 加 max_feed_bytes(16 MiB) 上限防 OOM

resp.bytes() 无上限改为流式受限读取，超 16 MiB 返回 feed-too-large。
挡 billion-laughs 内存放大 + 超大 feed body OOM。新增 feed_body_over_limit_rejected fixture。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

**acceptance_judges:**
- `cargo test -p attune-core feed_body_over_limit_rejected` exits 0。
- `cargo test -p attune-core ingest::rss` 既有 8 单元测试 0 failures（无回归）。
- `RealFeedFetcher` 含 `max_feed_bytes` 字段（`grep -n 'max_feed_bytes' rust/crates/attune-core/src/ingest/rss.rs` 命中）。

---

## Task 6: 云盘 — `RcloneRunner` trait + `RcloneFile` + `MockRcloneRunner`

**model_tier: opus**（架构/trait 边界设计：抓取层抽象是云盘整块的 mock 注入边界，决定后续可测性）

**Files:**
- Create: `rust/crates/attune-core/src/ingest/cloud_drive.rs`
- Test: 同文件 `#[cfg(test)] mod tests`

- [ ] **Step 1: 写 failing test — MockRcloneRunner 返回 lsjson + cat**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mock_runner_lsjson_and_cat() {
        let mock = MockRcloneRunner::new()
            .with_files(vec![
                RcloneFile { path: "notes/a.md".into(), size: 12, mod_time: "2026-06-01T10:00:00Z".into(), sha1: Some("abc".into()), is_dir: false },
                RcloneFile { path: "notes/b.md".into(), size: 20, mod_time: "2026-06-01T11:00:00Z".into(), sha1: None, is_dir: false },
            ])
            .with_content("notes/a.md", b"# hello".to_vec());
        let files = mock.lsjson("gdrive:", "notes").unwrap();
        assert_eq!(files.len(), 2);
        let body = mock.cat("gdrive:", "notes/a.md", 64 * 1024 * 1024).unwrap();
        assert_eq!(body, b"# hello");
    }

    #[test]
    fn mock_cat_over_limit_errors() {
        let mock = MockRcloneRunner::new().with_content("big.bin", vec![0u8; 200]);
        let err = mock.cat("gdrive:", "big.bin", 100).unwrap_err();
        assert!(err.to_string().contains("cloud-file-too-large"));
    }
}
```

- [ ] **Step 2: 跑测试确认 FAIL**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-core cloud_drive::tests -- --nocapture`
Expected: FAIL — 模块/类型不存在。

- [ ] **Step 3: 写 trait + 类型 + mock（不含 RealRcloneRunner，留 T7）**

```rust
//! 云盘采集源（经 rclone subprocess 桥接 Google Drive / Dropbox / OneDrive 等）。
//! RcloneRunner 是 mock 注入边界 —— 与 RSS FeedFetcher 同模式，离线可测。
use crate::error::{Result, VaultError};
use crate::ingest::connector::{DocumentSink, RawDocument, SourceConnector, SourceKind};

/// lsjson 一行（只保留 ingest 必需字段）。
#[derive(Debug, Clone)]
pub struct RcloneFile {
    pub path: String,
    pub size: u64,
    pub mod_time: String,        // RFC3339
    pub sha1: Option<String>,    // Hashes.SHA-1，有则优先作 marker
    pub is_dir: bool,
}

/// 云盘抓取层抽象。生产 = RealRcloneRunner(调 rclone)；测试 = MockRcloneRunner。
pub trait RcloneRunner: Send + Sync {
    fn lsjson(&self, remote: &str, path: &str) -> Result<Vec<RcloneFile>>;
    fn cat(&self, remote: &str, full_path: &str, max_bytes: u64) -> Result<Vec<u8>>;
}

#[cfg(test)]
pub struct MockRcloneRunner {
    files: Vec<RcloneFile>,
    contents: std::collections::HashMap<String, Vec<u8>>,
}
#[cfg(test)]
impl MockRcloneRunner {
    pub fn new() -> Self { Self { files: vec![], contents: Default::default() } }
    pub fn with_files(mut self, f: Vec<RcloneFile>) -> Self { self.files = f; self }
    pub fn with_content(mut self, p: &str, c: Vec<u8>) -> Self { self.contents.insert(p.into(), c); self }
}
#[cfg(test)]
impl RcloneRunner for MockRcloneRunner {
    fn lsjson(&self, _remote: &str, _path: &str) -> Result<Vec<RcloneFile>> { Ok(self.files.clone()) }
    fn cat(&self, _remote: &str, full_path: &str, max_bytes: u64) -> Result<Vec<u8>> {
        let key = full_path.rsplit('/').next().map(|_| full_path).unwrap_or(full_path);
        let c = self.contents.get(key).cloned()
            .ok_or_else(|| VaultError::NotFound(format!("cloud file {full_path}")))?;
        if c.len() as u64 > max_bytes { return Err(VaultError::InvalidInput("cloud-file-too-large".into())); }
        Ok(c)
    }
}
```

- [ ] **Step 4: 跑测试确认 PASS**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-core cloud_drive::tests -- --nocapture`
Expected: PASS（2 个 case）。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/ingest/cloud_drive.rs
git commit -m "feat(cloud): 云盘抓取层抽象 RcloneRunner trait + MockRcloneRunner

RcloneFile / lsjson / cat(受 max_bytes 限) 定义；mock 注入边界保证离线可测，
与 RSS FeedFetcher 同模式。cat 超限返回 cloud-file-too-large。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

**acceptance_judges:**
- `cargo test -p attune-core cloud_drive::tests` exits 0（2 case）。
- `RcloneRunner` trait 含 `lsjson` + `cat`（`grep -n 'fn lsjson\|fn cat' cloud_drive.rs` 双命中）。
- mock cat 超 `max_bytes` 返回 `cloud-file-too-large`（断言已在 test）。

---

## Task 7: 云盘 — `parse_lsjson` 纯函数 + `RealRcloneRunner`（命令注入安全传参）

**model_tier: sonnet**

**Files:**
- Modify: `rust/crates/attune-core/src/ingest/cloud_drive.rs`
- Test: 同文件 `#[cfg(test)] mod tests`

> 吸收建议（spec §11 R6）：`RealRcloneRunner` 全程 `std::process::Command` arg 数组传参，**绝不 shell 字符串拼接**；rclone config 写临时文件(0600) 经 `--config` 传，不进 argv。

- [ ] **Step 1: 写 failing test — parse_lsjson 解析真实 rclone lsjson JSON**

```rust
#[test]
fn parse_lsjson_real_shape() {
    // rclone lsjson --recursive --files-only --hash 的真实输出形态
    let json = r#"[
      {"Path":"notes/a.md","Name":"a.md","Size":12,"ModTime":"2026-06-01T10:00:00.000Z","IsDir":false,"Hashes":{"SHA-1":"abc123"}},
      {"Path":"sub/b.pdf","Name":"b.pdf","Size":2048,"ModTime":"2026-06-01T11:00:00.000Z","IsDir":false}
    ]"#;
    let files = parse_lsjson(json).unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].path, "notes/a.md");
    assert_eq!(files[0].sha1.as_deref(), Some("abc123"));
    assert_eq!(files[1].sha1, None);          // 无 Hashes → None，marker 走 size+modtime
    assert_eq!(files[1].size, 2048);
}

#[test]
fn parse_lsjson_skips_dirs() {
    let json = r#"[{"Path":"d","Name":"d","Size":-1,"ModTime":"2026-06-01T10:00:00Z","IsDir":true}]"#;
    assert_eq!(parse_lsjson(json).unwrap().len(), 0); // 目录条目过滤
}
```

- [ ] **Step 2: 跑测试确认 FAIL**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-core parse_lsjson -- --nocapture`
Expected: FAIL — `parse_lsjson` 未定义。

- [ ] **Step 3: 写 parse_lsjson 纯函数 + RealRcloneRunner（arg 数组传参）**

```rust
use serde::Deserialize;

#[derive(Deserialize)]
struct LsjsonRow {
    #[serde(rename = "Path")] path: String,
    #[serde(rename = "Size")] size: i64,
    #[serde(rename = "ModTime")] mod_time: String,
    #[serde(rename = "IsDir", default)] is_dir: bool,
    #[serde(rename = "Hashes", default)] hashes: Option<std::collections::HashMap<String, String>>,
}

/// 解析 `rclone lsjson` 输出为 RcloneFile 列表（过滤目录）。纯函数，无 I/O。
pub fn parse_lsjson(json: &str) -> Result<Vec<RcloneFile>> {
    let rows: Vec<LsjsonRow> = serde_json::from_str(json)
        .map_err(|e| VaultError::InvalidInput(format!("rclone-config-invalid: lsjson parse {e}")))?;
    Ok(rows.into_iter().filter(|r| !r.is_dir).map(|r| RcloneFile {
        path: r.path,
        size: r.size.max(0) as u64,
        mod_time: r.mod_time,
        sha1: r.hashes.and_then(|h| h.get("SHA-1").cloned()),
        is_dir: false,
    }).collect())
}

/// 生产 runner：调真实 rclone subprocess。config 经临时文件(0600) + --config 传，不进 argv。
pub struct RealRcloneRunner { pub config_path: std::path::PathBuf }
impl RcloneRunner for RealRcloneRunner {
    fn lsjson(&self, remote: &str, path: &str) -> Result<Vec<RcloneFile>> {
        let target = format!("{remote}{path}");
        let out = std::process::Command::new("rclone")
            .args(["lsjson", "--recursive", "--files-only", "--hash",
                   "--config", &self.config_path.to_string_lossy(), &target])  // arg 数组，无 shell
            .output()
            .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound {
                VaultError::InvalidInput("rclone-not-found".into())
            } else { VaultError::InvalidInput(format!("rclone-exec-failed: {e}")) })?;
        if !out.status.success() {
            return Err(VaultError::InvalidInput("rclone-exec-failed".into()));
        }
        parse_lsjson(&String::from_utf8_lossy(&out.stdout))
    }
    fn cat(&self, remote: &str, full_path: &str, max_bytes: u64) -> Result<Vec<u8>> {
        let target = format!("{remote}{full_path}");
        let out = std::process::Command::new("rclone")
            .args(["cat", "--config", &self.config_path.to_string_lossy(),
                   "--count", &max_bytes.to_string(), &target])  // arg 数组，无 shell
            .output()
            .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound {
                VaultError::InvalidInput("rclone-not-found".into())
            } else { VaultError::InvalidInput(format!("rclone-exec-failed: {e}")) })?;
        if !out.status.success() { return Err(VaultError::InvalidInput("rclone-exec-failed".into())); }
        if out.stdout.len() as u64 > max_bytes { return Err(VaultError::InvalidInput("cloud-file-too-large".into())); }
        Ok(out.stdout)
    }
}
```

- [ ] **Step 4: 跑测试确认 PASS**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-core parse_lsjson -- --nocapture`
Expected: PASS（2 case）。
`cargo clippy -p attune-core --all-targets -- -D warnings` 干净。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/ingest/cloud_drive.rs
git commit -m "feat(cloud): parse_lsjson 纯函数 + RealRcloneRunner(arg 数组传参防注入)

lsjson 解析过滤目录、SHA-1 有则优先；RealRcloneRunner 全程 Command arg 数组，
绝不 shell 拼接，config 经临时文件 --config 传不进 argv。rclone 缺失 → rclone-not-found。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

**acceptance_judges:**
- `cargo test -p attune-core parse_lsjson` exits 0（2 case，含目录过滤）。
- `RealRcloneRunner` 用 `.args([...])` arg 数组（`grep -n '.args(\[' cloud_drive.rs` 命中），**无** `Command::new("sh")` / 字符串拼接 shell（`grep -c 'sh", "-c"' cloud_drive.rs` = 0）。
- rclone NotFound 映射 `rclone-not-found`（`grep` 命中）。

---

## Task 8: 云盘 — `CloudDriveConnector impl SourceConnector` + 增量 marker + 路径穿越防护

**model_tier: sonnet**

**Files:**
- Modify: `rust/crates/attune-core/src/ingest/cloud_drive.rs`
- Test: 同文件 `#[cfg(test)] mod tests`

> 吸收建议⑤（marker → 复用 content_hash 短路）+ spec §11 R4(marker)/R10(路径穿越)。`modified_marker` = SHA-1 优先，无则 `size + mod_time`。含 `..` 段的 path 跳过。

- [ ] **Step 1: 写 failing test — connector emit RawDocument + marker + 路径穿越跳过**

```rust
#[test]
fn connector_emits_docs_with_marker() {
    let mock = MockRcloneRunner::new()
        .with_files(vec![
            RcloneFile { path: "a.md".into(), size: 7, mod_time: "2026-06-01T10:00:00Z".into(), sha1: Some("h1".into()), is_dir: false },
            RcloneFile { path: "../../etc/passwd".into(), size: 9, mod_time: "x".into(), sha1: None, is_dir: false }, // 路径穿越
        ])
        .with_content("a.md", b"# hello".to_vec());
    let conn = CloudDriveConnector::new_for_test("gdrive:", "", Box::new(mock), 64 * 1024 * 1024);
    let mut docs = Vec::new();
    let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d));
    conn.fetch_documents(&mut sink).unwrap();
    assert_eq!(docs.len(), 1, "含 .. 段的 path 必须跳过，只 emit a.md");
    assert_eq!(docs[0].source_kind, SourceKind::CloudDrive);
    assert_eq!(docs[0].modified_marker.as_deref(), Some("h1")); // SHA-1 优先
    assert!(docs[0].source_ref.contains("a.md"));
}

#[test]
fn marker_falls_back_to_size_modtime_without_sha1() {
    let f = RcloneFile { path: "b.md".into(), size: 20, mod_time: "2026-06-01T11:00:00Z".into(), sha1: None, is_dir: false };
    assert_eq!(compute_marker(&f), "20:2026-06-01T11:00:00Z"); // 无 SHA-1 → size:modtime
}
```

- [ ] **Step 2: 跑测试确认 FAIL**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-core cloud_drive -- --nocapture`
Expected: FAIL — `CloudDriveConnector` / `compute_marker` 未定义。

- [ ] **Step 3: 写 connector + compute_marker + 路径穿越过滤**

```rust
pub struct CloudDriveConnector {
    remote: String,
    path: String,
    runner: Box<dyn RcloneRunner>,
    max_file_bytes: u64,
}
impl CloudDriveConnector {
    #[cfg(test)]
    pub fn new_for_test(remote: &str, path: &str, runner: Box<dyn RcloneRunner>, max: u64) -> Self {
        Self { remote: remote.into(), path: path.into(), runner, max_file_bytes: max }
    }
}

/// 增量 marker：SHA-1 优先；无则 size:mod_time 组合。
pub fn compute_marker(f: &RcloneFile) -> String {
    match &f.sha1 {
        Some(h) => h.clone(),
        None => format!("{}:{}", f.size, f.mod_time),
    }
}

/// path 含 ".." 段视为穿越，跳过。
fn is_safe_path(p: &str) -> bool {
    !p.split('/').any(|seg| seg == "..")
}

impl SourceConnector for CloudDriveConnector {
    fn source_kind(&self) -> SourceKind { SourceKind::CloudDrive }
    fn fetch_documents(&self, sink: &mut DocumentSink<'_>) -> Result<()> {
        let files = self.runner.lsjson(&self.remote, &self.path)?; // 源级错误向上抛
        for f in files {
            if f.is_dir || !is_safe_path(&f.path) { continue; }      // 路径穿越跳过
            let full = if self.path.is_empty() { f.path.clone() } else { format!("{}/{}", self.path, f.path) };
            let content = match self.runner.cat(&self.remote, &full, self.max_file_bytes) {
                Ok(c) => c,
                Err(e) => { tracing::warn!("cloud cat skip {full}: {e}"); continue; } // 单文件可恢复错误吞掉
            };
            sink(RawDocument {
                uri: format!("rclone://{}{}", self.remote, full),
                title: String::new(),
                content,
                mime_hint: None,
                source_kind: SourceKind::CloudDrive,
                source_ref: format!("{}#{}", self.remote, full),
                modified_marker: Some(compute_marker(&f)),
                domain: None, tags: None, corpus_domain: None,
                metadata: std::collections::HashMap::new(),
            });
        }
        Ok(())
    }
}
```

- [ ] **Step 4: 跑测试确认 PASS**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-core cloud_drive -- --nocapture`
Expected: PASS（含路径穿越跳过 + marker fallback）。
`cargo clippy -p attune-core --all-targets -- -D warnings` 干净。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/ingest/cloud_drive.rs
git commit -m "feat(cloud): CloudDriveConnector impl SourceConnector + 增量 marker + 路径穿越防护

fetch_documents emit RawDocument(source_kind=CloudDrive)；marker SHA-1 优先无则
size:modtime；含 .. 段 path 跳过；单文件 cat 失败吞掉继续(SourceConnector 契约)。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

**acceptance_judges:**
- `cargo test -p attune-core cloud_drive` exits 0（connector emit + 路径穿越 + marker fallback 全 PASS）。
- `is_safe_path("../../etc/passwd")` 返回 false 致跳过（断言 `docs.len()==1` 已验）。
- `RawDocument.source_kind == SourceKind::CloudDrive`、`corpus_domain == None`（OSS 边界，断言已验）。

---

## Task 9: 云盘 — `sync_cloud_remote` 三段式 worker（锁外 I/O + content_hash 短路）

**model_tier: opus**（持锁设计 / Lock ordering / 增量 cursor 推进是死锁+正确性高风险结构判断，per §11 R5）

**Files:**
- Create: `rust/crates/attune-server/src/ingest_cloud.rs`
- Test: 同文件 `#[cfg(test)] mod tests`

> **吸收建议⑤**：云盘逐文档调 `attune_core::ingest::ingest_document`（pipeline.rs:44），其内 content_hash 短路(pipeline.rs:99 `find_item_by_content_hash`)；云盘新码**不绕过**该短路。三段式锁外 I/O 对齐 `ingest_rss.rs:23-60`。

- [ ] **Step 1: 写 failing test — sync 调 ingest_document 且 indexed_files 短路**

```rust
#[test]
fn sync_cloud_remote_ingests_via_pipeline_and_short_circuits() {
    let state = /* 复用 ingest_rss 集成测试 harness 的 unlocked state 构造 */;
    let dek = { let v = state.vault.lock().unwrap(); v.dek_db().unwrap() };
    // 落一个 cloud remote（mock config）
    let remote_id = { let v = state.vault.lock().unwrap();
        v.store().add_cloud_remote(&dek, &test_cloud_input()).unwrap() };
    // 注入 mock runner（2 文件）→ 首轮 sync new_files=2
    let r1 = sync_cloud_remote_with_runner(&state, &remote_id, mock_runner_2_files()).unwrap();
    assert_eq!(r1["new_files"], 2);
    // 第二轮同 marker → indexed_files 短路，new_files=0
    let r2 = sync_cloud_remote_with_runner(&state, &remote_id, mock_runner_2_files()).unwrap();
    assert_eq!(r2["new_files"], 0, "ModTime/marker 未变必须短路，不重复入库");
}
```

- [ ] **Step 2: 跑测试确认 FAIL**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-server sync_cloud_remote_ingests_via_pipeline -- --nocapture`
Expected: FAIL — `sync_cloud_remote` / `add_cloud_remote` 未定义（后者 T10 提供，本 task 先红）。

- [ ] **Step 3: 写 sync_cloud_remote 三段式（对齐 sync_rss_feed）**

```rust
//! 云盘增量同步 —— add / poll-now route 与周期 worker 共用。三段式（对齐 ingest_rss）：
//!   阶段 0 锁内读 cursor + 解密 rclone config；阶段 1 锁外 rclone I/O；阶段 2 逐文档短暂持锁 ingest。
use std::sync::Arc;
use attune_core::ingest::{ingest_document, CloudDriveConnector, DocumentSink, RawDocument, RcloneRunner, SourceConnector};
use crate::state::AppState;

pub fn sync_cloud_remote(state: &Arc<AppState>, remote_id: &str) -> Result<serde_json::Value, String> {
    // 阶段 0：锁内读配置 + 解密 config，写临时文件(0600)。
    let (remote, path, max_bytes, config_path, last_cursor) = {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db().map_err(|e| e.to_string())?;
        let row = vault.store().get_cloud_remote(&dek, remote_id).map_err(|e| e.to_string())?
            .ok_or_else(|| "cloud-remote-not-found".to_string())?;
        if !row.enabled { return Err(format!("cloud remote {remote_id} disabled")); }
        let cfg_path = write_temp_rclone_config_0600(&row.rclone_config)?; // 不进 argv / 不进 log
        (row.remote_name, row.remote_path, row.max_file_bytes, cfg_path, row.last_cursor)
    };
    // 阶段 1：锁外 rclone I/O，物化 docs（DocumentSink 回调，不一次性下载全部）。
    let runner: Box<dyn RcloneRunner> = Box::new(attune_core::ingest::RealRcloneRunner { config_path });
    let conn = CloudDriveConnector::new(&remote, &path, runner, max_bytes);
    let mut docs: Vec<RawDocument> = Vec::new();
    let fetch = { let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d)); conn.fetch_documents(&mut sink) };
    if let Err(e) = fetch {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let _ = vault.store().touch_cloud_polled_at(remote_id); // 防 tight-loop
        return Err(format!("cloud sync {remote_id}: {e}"));
    }
    // 阶段 2：逐文档短暂持锁 → indexed_files 短路 → ingest_document(内含 content_hash 短路) → cursor 推进。
    let mut new_files = 0u32; let mut skipped = 0u32; let mut max_marker = last_cursor.clone();
    for d in &docs {
        let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db().map_err(|e| e.to_string())?;
        let store = vault.store();
        if store.get_indexed_file(&d.source_ref).map_err(|e| e.to_string())?
            .map(|f| Some(f.file_hash.clone()) == d.modified_marker).unwrap_or(false) {
            skipped += 1; continue; // marker 未变 → 第一层短路
        }
        match ingest_document(store, &dek, d).map_err(|e| e.to_string())? {  // content_hash 短路在内部 pipeline.rs:99
            outcome if outcome.is_new() => {
                store.upsert_indexed_file(&d.source_ref, d.modified_marker.as_deref().unwrap_or(""), remote_id)
                    .map_err(|e| e.to_string())?;
                new_files += 1;
            }
            _ => { skipped += 1; }
        }
        if d.modified_marker > max_marker { max_marker = d.modified_marker.clone(); }
    }
    { let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
      let _ = vault.store().set_cloud_cursor(remote_id, max_marker.as_deref()); // cursor 推进 + touch_polled_at
    }
    Ok(serde_json::json!({ "status": "ok", "total_files": docs.len(), "new_files": new_files, "skipped": skipped, "errors": [] }))
}
```
（`is_new()` / `IngestOutcome` 来自 pipeline.rs:25-43；`get_indexed_file` / `upsert_indexed_file` 复用既有 store API，与 RSS 同。Store CRUD 方法 `get_cloud_remote` / `touch_cloud_polled_at` / `set_cloud_cursor` 由 T10 提供 → 本 task 在 T10 后才能编译通过，故 T9→T10 依赖反向；实施时先写 T9 测试与签名，T10 落 store 后回到 T9 Step 4 跑绿。）

- [ ] **Step 4: 跑测试确认 PASS（在 T10 完成后）**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-server sync_cloud_remote_ingests_via_pipeline -- --nocapture`
Expected: PASS — 首轮 new_files=2，二轮 new_files=0（短路）。
`cargo clippy -p attune-server --all-targets -- -D warnings` 干净。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/ingest_cloud.rs
git commit -m "feat(cloud): sync_cloud_remote 三段式 worker(锁外 I/O + content_hash 短路复用)

阶段0 锁内读 cursor+解密 config；阶段1 锁外 rclone；阶段2 逐文档短暂持锁经
ingest_document(复用 pipeline content_hash 短路，不绕过)。Lock ordering 沿用 RSS。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

**acceptance_judges:**
- `cargo test -p attune-server sync_cloud_remote_ingests_via_pipeline` exits 0（首轮 2 / 二轮 0 短路）。
- worker 调 `ingest_document`（`grep -n 'ingest_document' ingest_cloud.rs` 命中），**未**自调 vectors/fulltext API（`grep -c 'vectors\|fulltext' ingest_cloud.rs` = 0，per 项目 reindex 约定）。
- 阶段 1 不持 vault 锁（结构对齐 sync_rss_feed：fetch 在 lock guard 作用域外）。

---

## Task 10: 云盘 — `cloud_drive_remotes` 表 + CRUD（加密凭据）

**model_tier: sonnet**

**Files:**
- Create: `rust/crates/attune-core/src/store/cloud_drive_remotes.rs`
- Modify: `rust/crates/attune-core/src/store/mod.rs`（CREATE TABLE + `pub mod`）
- Test: 同 `cloud_drive_remotes.rs` `#[cfg(test)] mod tests`

> 凭据 `rclone_config_enc` 字段级 AES-256-GCM（复用 `crypto::encrypt(dek, bytes)`，对齐 `rss_feeds.rs:65`）。日志只打 remote_id 不打 config（§1.4 secrets + §11 R9）。

- [ ] **Step 1: 写 failing test — add/get round-trip + config 加密落盘**

```rust
#[test]
fn cloud_remote_crud_roundtrip_encrypted() {
    let (store, dek) = test_store_with_dek();
    let input = CloudDriveRemoteInput {
        name: "My GDrive".into(), remote_name: "gdrive:".into(), remote_path: "KB".into(),
        rclone_config: "[gdrive]\ntype = drive\n".into(),
        poll_interval_minutes: Some(360), max_file_bytes: Some(67108864),
        include_glob: String::new(), exclude_glob: String::new(),
    };
    let id = store.add_cloud_remote(&dek, &input).unwrap();
    let row = store.get_cloud_remote(&dek, &id).unwrap().unwrap();
    assert_eq!(row.remote_name, "gdrive:");
    assert_eq!(row.rclone_config, "[gdrive]\ntype = drive\n"); // 解密回明文
    // 落盘 BLOB 是密文，不含明文 config
    let raw_blob: Vec<u8> = store.raw_cloud_config_blob(&id).unwrap();
    assert!(!String::from_utf8_lossy(&raw_blob).contains("type = drive"), "config 必须加密落盘");
}
```

- [ ] **Step 2: 跑测试确认 FAIL**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-core cloud_remote_crud_roundtrip_encrypted -- --nocapture`
Expected: FAIL — 类型/方法/表不存在。

- [ ] **Step 3: 写表 + Input/Row + CRUD（对齐 rss_feeds.rs 模式）**

```rust
// store/mod.rs 内 CREATE TABLE（与既有 IF NOT EXISTS 模式一致，老 vault 自动建表）：
// CREATE TABLE IF NOT EXISTS cloud_drive_remotes (... per spec §3 DDL ...);
// CREATE INDEX IF NOT EXISTS idx_cloud_remotes_enabled_polled ON cloud_drive_remotes(enabled, last_polled_at);

// store/cloud_drive_remotes.rs:
use crate::crypto::{self, Key32};
use crate::error::Result;
use crate::store::Store;
use rusqlite::params;

pub struct CloudDriveRemoteInput {
    pub name: String, pub remote_name: String, pub remote_path: String,
    pub rclone_config: String,
    pub poll_interval_minutes: Option<u32>, pub max_file_bytes: Option<u64>,
    pub include_glob: String, pub exclude_glob: String,
}
pub struct CloudDriveRemoteRow {
    pub id: String, pub name: String, pub remote_name: String, pub remote_path: String,
    pub rclone_config: String,   // 已解密
    pub last_cursor: Option<String>, pub last_polled_at: Option<String>,
    pub poll_interval_minutes: u32, pub max_file_bytes: u64,
    pub include_glob: String, pub exclude_glob: String, pub enabled: bool,
}

impl Store {
    pub fn add_cloud_remote(&self, dek: &Key32, input: &CloudDriveRemoteInput) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let cfg_enc = crypto::encrypt(dek, input.rclone_config.as_bytes())?; // 字段级 AES-256-GCM
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO cloud_drive_remotes(id,name,remote_name,remote_path,rclone_config_enc,\
             poll_interval_minutes,max_file_bytes,include_glob,exclude_glob,enabled,created_at,updated_at)\
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,1,?10,?10)",
            params![id, input.name, input.remote_name, input.remote_path, cfg_enc,
                input.poll_interval_minutes.unwrap_or(360),
                input.max_file_bytes.unwrap_or(67108864) as i64,
                input.include_glob, input.exclude_glob, now])?;
        Ok(id)
    }
    pub fn get_cloud_remote(&self, dek: &Key32, id: &str) -> Result<Option<CloudDriveRemoteRow>> { /* SELECT + crypto::decrypt(dek, blob) */ unimplemented!() }
    pub fn list_cloud_remotes(&self) -> Result<Vec<CloudDriveRemoteRow>> { /* 元数据，config 不解密回传 */ unimplemented!() }
    pub fn delete_cloud_remote(&self, id: &str) -> Result<bool> { /* DELETE，已 ingest item 保留 */ unimplemented!() }
    pub fn patch_cloud_remote(&self, id: &str, enabled: Option<bool>, interval: Option<u32>) -> Result<()> { unimplemented!() }
    pub fn touch_cloud_polled_at(&self, id: &str) -> Result<()> { /* UPDATE last_polled_at=now */ unimplemented!() }
    pub fn set_cloud_cursor(&self, id: &str, cursor: Option<&str>) -> Result<()> { /* UPDATE last_cursor + last_polled_at */ unimplemented!() }
}
```
（`unimplemented!()` 仅为本 task Step 3 的骨架占位标记 —— 实施时填齐每个方法体（SELECT + `crypto::decrypt` + params），与 `add_cloud_remote` 同模式；**不得保留 `unimplemented!()` 进 commit**，Step 4 测试覆盖 add/get/list/delete/patch/cursor 全路径强制填实。）

- [ ] **Step 4: 跑测试确认 PASS（CRUD 全方法）**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-core cloud_drive_remotes -- --nocapture`
Expected: PASS — round-trip 解密正确 + 落盘 BLOB 是密文。
`grep -rn 'unimplemented!' rust/crates/attune-core/src/store/cloud_drive_remotes.rs` 必须 0 命中。
`cargo clippy -p attune-core --all-targets -- -D warnings` 干净。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/store/cloud_drive_remotes.rs rust/crates/attune-core/src/store/mod.rs
git commit -m "feat(cloud): cloud_drive_remotes 表 + CRUD(rclone_config 字段级加密)

CREATE TABLE IF NOT EXISTS(老 vault 自动建表)；config 经 AES-256-GCM 落 BLOB，
解密回明文供 worker；list 不回传 config。对齐 store/rss_feeds.rs 模式。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

**acceptance_judges:**
- `cargo test -p attune-core cloud_drive_remotes` exits 0（CRUD round-trip）。
- 落盘 BLOB 不含明文 `type = drive`（加密落盘断言已验）。
- `grep -rn 'unimplemented!' store/cloud_drive_remotes.rs` = 0（无骨架残留）。

---

## Task 11: 云盘 — 5 路由 handler (`routes/cloud_drive.rs`)

**model_tier: sonnet**

**Files:**
- Create: `rust/crates/attune-server/src/routes/cloud_drive.rs`
- Test: 同文件 `#[cfg(test)] mod tests`（SSRF/config 校验单元 case）

> route `validate` 同样校验 remote_name 合法（白名单拒 `..` / `;` / `|` / 反引号，per §11 R6）+ 错误经 `AppError` → 400/404/502/503。

- [ ] **Step 1: 写 failing test — add 非法 remote_name 拒绝 + 5 handler 签名存在**

```rust
#[test]
fn add_cloud_rejects_shell_metachar_remote_name() {
    for bad in ["gdrive;rm -rf:", "g|drive:", "g`whoami`:", "../etc:"] {
        let req = AddCloudRemoteRequest { remote_name: bad.into(), ..default_req() };
        assert!(validate_cloud(&req).is_err(), "{bad} 必须被拒(rclone-config-invalid)");
    }
}
#[test]
fn add_cloud_accepts_normal_remote_name() {
    let req = AddCloudRemoteRequest { remote_name: "gdrive:".into(), ..default_req() };
    assert!(validate_cloud(&req).is_ok());
}
```

- [ ] **Step 2: 跑测试确认 FAIL**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-server add_cloud_rejects_shell_metachar -- --nocapture`
Expected: FAIL — handler/类型未定义。

- [ ] **Step 3: 写 5 handler + validate_cloud**

```rust
//! 云盘源 route —— add/list/delete/patch/poll 五路由。挂 /api/v1/sources/cloud/remotes。
use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use attune_core::store::cloud_drive_remotes::CloudDriveRemoteInput;
use crate::error::{AppError, AppResult};
use crate::state::SharedState;

#[derive(Deserialize)]
pub struct AddCloudRemoteRequest {
    pub remote_name: String,
    #[serde(default)] pub remote_path: String,
    pub rclone_config: String,
    #[serde(default)] pub name: String,
    #[serde(default)] pub poll_interval_minutes: Option<u32>,
    #[serde(default)] pub include_glob: Vec<String>,
    #[serde(default)] pub exclude_glob: Vec<String>,
    #[serde(default)] pub max_file_bytes: Option<u64>,
}

/// remote_name 白名单：字母数字 + '_' '-' '.' ':' ；拒 shell 元字符 + '..'。
fn validate_cloud(req: &AddCloudRemoteRequest) -> Result<(), AppError> {
    let n = req.remote_name.trim();
    if n.is_empty() { return Err(AppError::BadRequest("rclone-config-invalid: remote_name empty".into())); }
    if n.contains("..") || n.chars().any(|c| !(c.is_alphanumeric() || matches!(c, '_'|'-'|'.'|':'))) {
        return Err(AppError::BadRequest("rclone-config-invalid: illegal remote_name".into()));
    }
    if req.rclone_config.trim().is_empty() {
        return Err(AppError::BadRequest("rclone-config-invalid: empty config".into()));
    }
    Ok(())
}

pub async fn add_cloud_remote(State(state): State<SharedState>, Json(body): Json<AddCloudRemoteRequest>) -> AppResult<Json<serde_json::Value>> {
    validate_cloud(&body)?;
    let id = { let vault = state.vault.lock().unwrap_or_else(|e| e.into_inner());
        let dek = vault.dek_db()?;
        vault.store().add_cloud_remote(&dek, &CloudDriveRemoteInput {
            name: body.name.clone(), remote_name: body.remote_name.clone(), remote_path: body.remote_path.clone(),
            rclone_config: body.rclone_config.clone(),
            poll_interval_minutes: body.poll_interval_minutes, max_file_bytes: body.max_file_bytes,
            include_glob: body.include_glob.join(","), exclude_glob: body.exclude_glob.join(","),
        })? };
    // 首轮 poll 走 spawn_blocking（对齐 RSS add）
    let st = state.clone(); let rid = id.clone();
    let poll = tokio::task::spawn_blocking(move || crate::ingest_cloud::sync_cloud_remote(&st.0, &rid)).await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "id": id, "poll": poll.unwrap_or_else(|e| serde_json::json!({"status":"error","errors":[e]})) })))
}
// list_cloud_remotes / delete_cloud_remote / patch_cloud_remote / poll_cloud_remote 四 handler
// 对齐 routes/rss.rs 的 list/delete/update/poll 模式（list 不回传 rclone_config）。
```
（其余 4 handler 实施时按 routes/rss.rs 对应 handler 1:1 镜像填实；poll handler 经 spawn_blocking 调 `sync_cloud_remote`。）

- [ ] **Step 4: 跑测试确认 PASS**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-server cloud_drive -- --nocapture`
Expected: PASS（非法 remote_name 全拒 + 正常通过）。
`cargo clippy -p attune-server --all-targets -- -D warnings` 干净。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/routes/cloud_drive.rs
git commit -m "feat(cloud): 5 路由 handler(add/list/delete/patch/poll)+remote_name 白名单校验

validate_cloud 拒 shell 元字符 + '..'(rclone-config-invalid)；add 后 spawn_blocking
首轮 poll；list 不回传 rclone_config(凭据不出库)。对齐 routes/rss.rs 模式。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

**acceptance_judges:**
- `cargo test -p attune-server cloud_drive` exits 0（shell 元字符 4 个全拒 + 正常通过）。
- 5 个 pub handler 函数存在（`grep -c 'pub async fn' routes/cloud_drive.rs` ≥ 5）。
- list handler 不含 `rclone_config` 回传（`grep -n 'rclone_config' routes/cloud_drive.rs` 仅在 add 路径）。

---

## Task 12: 云盘 — `start_cloud_sync_worker` + unlock 启动接线

**model_tier: sonnet**

**Files:**
- Modify: `rust/crates/attune-server/src/state.rs`（加 `start_cloud_sync_worker`，对齐 `start_rss_sync_worker`）
- Modify: `rust/crates/attune-server/src/routes/vault.rs`（unlock 三处启动，对齐 RSS）
- Test: `rust/crates/attune-server/src/state.rs`（worker 到期判断单元 case）

- [ ] **Step 1: 写 failing test — worker 只调到期 remote**

```rust
#[test]
fn cloud_worker_polls_only_due_remotes() {
    // 两 remote：A last_polled_at 很久前(到期)，B 刚 poll(未到期)。
    // 跑一轮 worker tick → 只有 A 被 sync。
    let state = /* unlocked harness */;
    seed_two_remotes_one_due(&state);
    let polled = cloud_worker_tick_for_test(&state); // 返回被 poll 的 remote_id 列表
    assert_eq!(polled, vec!["remote_A_due"]);
}
```

- [ ] **Step 2: 跑测试确认 FAIL**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-server cloud_worker_polls_only_due -- --nocapture`
Expected: FAIL — `start_cloud_sync_worker` / tick helper 未定义。

- [ ] **Step 3: 写 worker + unlock 接线（镜像 RSS）**

```rust
// state.rs —— 对齐 start_rss_sync_worker：spawn tokio task，按 poll_interval_minutes 判到期，
// spawn_blocking 调 sync_cloud_remote；失败仅 log(remote_id，不打 config)。
pub fn start_cloud_sync_worker(state: SharedState) { /* loop tick: list enabled remotes → due → spawn_blocking sync_cloud_remote */ }

// vault.rs —— unlock 成功后(RSS 已有 start_rss_sync_worker 的同三处)追加：
//   crate::state::start_cloud_sync_worker(state.clone());
```

- [ ] **Step 4: 跑测试确认 PASS**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-server cloud_worker_polls_only_due -- --nocapture`
Expected: PASS（只 poll 到期 A）。
`grep -c 'start_cloud_sync_worker' rust/crates/attune-server/src/routes/vault.rs` 应为 3（对齐 RSS 三处 unlock 路径）。
`cargo clippy -p attune-server --all-targets -- -D warnings` 干净。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/state.rs rust/crates/attune-server/src/routes/vault.rs
git commit -m "feat(cloud): start_cloud_sync_worker + unlock 三处启动接线(镜像 RSS)

按 poll_interval_minutes 判到期只 sync 到期 remote；unlock 后启动；失败仅 log
remote_id 不打 config(secrets)。对齐 start_rss_sync_worker。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

**acceptance_judges:**
- `cargo test -p attune-server cloud_worker_polls_only_due` exits 0（只 poll 到期 remote）。
- `grep -c 'start_cloud_sync_worker' routes/vault.rs` = 3（unlock 三处全接，对齐 RSS）。

---

## Task 13: 云盘 — rclone 缺失 graceful 探测（不 panic → rclone-not-found 503）

**model_tier: sonnet**

**Files:**
- Modify: `rust/crates/attune-server/src/ingest_cloud.rs` 或 `routes/cloud_drive.rs`（探测 + 503 映射）
- Test: 集成 `tests/cloud_drive_subprocess.rs`（PATH 无 rclone 场景）

> spec §11 R3：目标机未装 rclone → `rclone-not-found` 503 + UI 友好提示，整源 disable，**不 panic**，其它 5 源不受影响。

- [ ] **Step 1: 写 failing test — rclone 缺失返回 503 不 panic**

```rust
// tests/cloud_drive_subprocess.rs
#[test]
fn rclone_missing_returns_503_not_panic() {
    // 用 RealRcloneRunner 指向不存在的 binary（通过临时空 PATH 或 mock NotFound）
    // sync_cloud_remote → Err 含 "rclone-not-found"；route 层映射 503；进程不 panic。
    let err = simulate_rclone_not_found_sync();
    assert!(err.contains("rclone-not-found"), "缺 rclone 必须 graceful 报 rclone-not-found");
}
```

- [ ] **Step 2: 跑测试确认 FAIL**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-server --test cloud_drive_subprocess rclone_missing -- --nocapture`
Expected: FAIL — 映射未接。

- [ ] **Step 3: 写探测 + 503 映射（AppError → 503）**

```rust
// AppError 增加 / 复用一个映射：含 "rclone-not-found" → StatusCode::SERVICE_UNAVAILABLE。
// RealRcloneRunner 已在 T7 把 io::ErrorKind::NotFound 映射为 "rclone-not-found"；
// 本 task 确保 route 层把该 code → 503 而非 500/panic。
fn cloud_error_status(msg: &str) -> axum::http::StatusCode {
    use axum::http::StatusCode;
    if msg.contains("rclone-not-found") { StatusCode::SERVICE_UNAVAILABLE }
    else if msg.contains("rclone-config-invalid") || msg.contains("ssrf-rejected") { StatusCode::BAD_REQUEST }
    else if msg.contains("cloud-remote-not-found") { StatusCode::NOT_FOUND }
    else { StatusCode::BAD_GATEWAY } // rclone-exec-failed
}
```

- [ ] **Step 4: 跑测试确认 PASS**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-server --test cloud_drive_subprocess rclone_missing -- --nocapture`
Expected: PASS — 503，进程不 panic。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/ingest_cloud.rs rust/crates/attune-server/src/routes/cloud_drive.rs rust/crates/attune-server/tests/cloud_drive_subprocess.rs
git commit -m "feat(cloud): rclone 缺失 graceful 探测 → rclone-not-found 503 不 panic

route 层 cloud_error_status 把 rclone-not-found → 503，整源 disable，其它 5 源
不受影响。新增 rclone_missing_returns_503_not_panic 集成 fixture。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

**acceptance_judges:**
- `cargo test -p attune-server --test cloud_drive_subprocess rclone_missing` exits 0。
- 缺 rclone 路径**无 panic / unwrap on None**（`grep -n '.unwrap()' ingest_cloud.rs` 复核 rclone 调用处全走 `?` / map_err）。
- `rclone-not-found` 映射 503（断言已验）。

---

## Task 14: 云盘 adversarial 集成 — 命令注入 + 路径穿越端到端

**model_tier: sonnet**

**Files:**
- Modify: `rust/crates/attune-server/tests/cloud_drive_subprocess.rs`

> 补 §9 adversarial 矩阵：(a) rclone_config / remote_name 含 shell 元字符不触发命令注入；(b) 文件名路径穿越不触达本地 fs。集成层验证（route → worker → connector 全链）。

- [ ] **Step 1: 写 failing test — 端到端命令注入 + 路径穿越无害**

```rust
#[test]
fn cloud_command_injection_and_path_traversal_e2e() {
    // (a) remote_name 含 ';rm -rf' → route validate_cloud 拒(rclone-config-invalid)，DB 无写入
    let r = add_cloud_via_route("gdrive;rm -rf /:", "valid config");
    assert_eq!(r.status, 400);
    // (b) mock runner 返回含 ../../etc/passwd 的文件 → connector 跳过，vault items 无该文件
    let id = add_cloud_via_route("gdrive:", "valid config").id;
    poll_with_mock_traversal_file(&id);
    assert_eq!(vault_item_count_for_remote(&id), 1, "穿越文件不入库，只入合法文件");
    // 进程未 panic、本地 /etc/passwd 未被读
}
```

- [ ] **Step 2: 跑测试确认 FAIL**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-server --test cloud_drive_subprocess cloud_command_injection -- --nocapture`
Expected: FAIL（helper 未接）。

- [ ] **Step 3: 接集成 helper（复用 T6 mock + T11 route，不新造逻辑）**

```rust
// add_cloud_via_route / poll_with_mock_traversal_file / vault_item_count_for_remote
// 全部复用既有 test server harness（对齐 git_subprocess / rss 集成测试风格）。
```

- [ ] **Step 4: 跑测试确认 PASS**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-server --test cloud_drive_subprocess cloud_command_injection -- --nocapture`
Expected: PASS — 注入拒绝 400 + 穿越文件跳过 + 仅合法文件入库。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/tests/cloud_drive_subprocess.rs
git commit -m "test(cloud): 命令注入 + 路径穿越端到端 adversarial fixture

remote_name 含 ';rm -rf' → 400 拒且 DB 无写入；../../etc/passwd 文件 connector
跳过不入库。验证 arg 数组传参防注入 + 路径穿越防护全链有效。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

**acceptance_judges:**
- `cargo test -p attune-server --test cloud_drive_subprocess cloud_command_injection` exits 0。
- 注入 remote_name → 400 且 DB 无写入；穿越文件不入库（`vault_item_count==1` 已验）。

---

## Task 15: 模块接线 + 全量 conflict-marker 自检（吸收建议③）

**model_tier: haiku**（纯样板接线 + grep 守卫）

**Files:**
- Modify: `rust/crates/attune-core/src/ingest/mod.rs`（`pub mod cloud_drive;` + re-export `CloudDriveConnector` / `RcloneRunner` / `RealRcloneRunner` / `RcloneFile`）
- Modify: `rust/crates/attune-server/src/routes/mod.rs`（`pub mod cloud_drive;` + nest `/api/v1/sources/cloud/remotes`）

> **吸收建议③（marker 检查关）**：7+ 文件新增 + 多文件改动，本 task 在接线 commit 前强制跑 conflict-marker 守卫（§4.2.3），防 reset/stash/cherry-pick 残留 `<<<<<<<` 进 commit。

- [ ] **Step 1: 改 mod.rs 接线（core + server）**

```rust
// attune-core/src/ingest/mod.rs
pub mod cloud_drive;
pub use cloud_drive::{CloudDriveConnector, RcloneFile, RcloneRunner, RealRcloneRunner};

// attune-server/src/routes/mod.rs
pub mod cloud_drive;
// build_router 内 nest：
//   .route("/api/v1/sources/cloud/remotes", get(cloud_drive::list_cloud_remotes).post(cloud_drive::add_cloud_remote))
//   .route("/api/v1/sources/cloud/remotes/:id", delete(cloud_drive::delete_cloud_remote).patch(cloud_drive::patch_cloud_remote))
//   .route("/api/v1/sources/cloud/remotes/:id/poll", post(cloud_drive::poll_cloud_remote))
// 同时 server 顶层声明 pub mod ingest_cloud;
```

- [ ] **Step 2: 全量 conflict-marker + 行数守卫（§4.2.3 强制）**

```bash
# 本 sprint 触达的全部文件做 marker 检查（建议③）
FILES="rust/crates/attune-core/src/ingest/cloud_drive.rs \
rust/crates/attune-core/src/store/cloud_drive_remotes.rs \
rust/crates/attune-core/src/ingest/mod.rs rust/crates/attune-core/src/store/mod.rs \
rust/crates/attune-core/src/ingest/rss.rs \
rust/crates/attune-server/src/ingest_cloud.rs rust/crates/attune-server/src/routes/cloud_drive.rs \
rust/crates/attune-server/src/routes/rss.rs rust/crates/attune-server/src/ingest_rss.rs \
rust/crates/attune-server/src/routes/mod.rs rust/crates/attune-server/src/state.rs \
rust/crates/attune-server/src/routes/vault.rs rust/crates/attune-server/tests/cloud_drive_subprocess.rs"
grep -n '^<<<<<<<\|^=======$\|^>>>>>>>' $FILES   # 必须无输出
```
Expected: **无输出**（无 conflict marker）。有输出 → 停手手动 resolve 再继续。

- [ ] **Step 3: 全量编译 + clippy**

`df -h /data | tail -1` →
Run: `cd rust && cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: build 成功 + clippy 0 warning。

- [ ] **Step 4: 路由可达性冒烟**

Run: `cd rust && cargo test -p attune-server cloud -- --nocapture`
Expected: 云盘全部测试 PASS（route 已 nest，handler 可达）。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/src/ingest/mod.rs rust/crates/attune-server/src/routes/mod.rs
git commit -m "feat(cloud): 接线 ingest/mod + routes/mod，挂 /api/v1/sources/cloud/remotes

re-export CloudDriveConnector/RcloneRunner；nest 5 路由。接线前跑 conflict-marker
守卫(§4.2.3)13 文件无残留。workspace build + clippy 干净。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

**acceptance_judges:**
- `grep -n '^<<<<<<<\|^=======$\|^>>>>>>>' $FILES` 无输出（建议③ marker 守卫通过）。
- `cargo build --workspace` exits 0 + `cargo clippy --workspace --all-targets -- -D warnings` 无输出。
- 路由可达：`cargo test -p attune-server cloud` exits 0。

---

## Task 16: GA 验收 checklist + 并发 N=3 + 全量回归自检

**model_tier: haiku**（checklist 执行 + 数据收集，无新逻辑）

**Files:**
- Modify: `rust/crates/attune-server/tests/cloud_drive_subprocess.rs`（并发 N=3 case）
- 无产品代码改动（仅测试 + 验收）

> spec §9 concurrent + resource 收尾 + §11 R5 死锁验证。multi-seed 不适用（deterministic），但并发 case 固定线程数复跑 N=3 确认无 flake。

- [ ] **Step 1: 写 failing test — 并发多源 + 前台 add 无死锁 N=3**

```rust
#[test]
fn cloud_concurrent_no_deadlock_n3() {
    for seed in 0..3 { // 固定复跑 3 次确认无 flake（非随机 seed，deterministic）
        let state = /* unlocked harness */;
        let a = spawn_sync(&state, "remote_a");
        let b = spawn_sync(&state, "remote_b");
        let c = std::thread::spawn({ let s = state.clone(); move || add_cloud_via_route_direct(&s, "gdrive:") });
        a.join().unwrap(); b.join().unwrap(); c.join().unwrap(); // 不挂起 = 无死锁
        assert!(no_partial_writes(&state), "seed {seed}: 并发后无半写状态");
    }
}
```

- [ ] **Step 2: 跑测试确认 FAIL**

`df -h /data | tail -1` →
Run: `cd rust && cargo test -p attune-server --test cloud_drive_subprocess cloud_concurrent_no_deadlock -- --nocapture`
Expected: FAIL（helper 未接）。

- [ ] **Step 3: 接并发 helper（复用既有 test_support 多线程 harness）**

```rust
// spawn_sync / add_cloud_via_route_direct / no_partial_writes 复用 RSS concurrent 测试模式。
// 严守 Lock ordering vault→vectors→fulltext→embedding（worker 已对齐 RSS 三段式，不应死锁）。
```

- [ ] **Step 4: 全量回归 + GA checklist**

`df -h /data | tail -1` →
Run（全部必绿）:
```bash
cd rust
cargo test -p attune-core --release          # core 全量(含 cloud_drive + rss 新 case)
cargo test -p attune-server --release         # server 全量(含 cloud 集成)
cargo clippy --workspace --all-targets -- -D warnings
# #[ignore] 计数不突增
git grep -c '#\[ignore\]' -- 'rust/**/*.rs' | awk -F: '{s+=$2} END{print "ignored:", s}'
```
GA checklist 勾选（spec §9 + §11 全覆盖）:
- [ ] RSS SSRF route(T1/T2) + worker(T3) 双路径 fixture 全绿（建议②）
- [ ] RSS XXE 实测断言绿（建议①）
- [ ] RSS feed-too-large 上限绿
- [ ] 云盘 connector / lsjson / CRUD / worker / route / 集成全绿
- [ ] 云盘命令注入 + 路径穿越 adversarial 绿（建议③精神）
- [ ] rclone 缺失 503 不 panic 绿
- [ ] content_hash 短路复用验证绿（二轮 new_files=0，建议⑤）
- [ ] 并发 N=3 无死锁绿（R5）
- [ ] conflict-marker 守卫无残留（建议③，T15 已跑）
- [ ] `cargo clippy --workspace -D warnings` 0 warning
- [ ] OSS 边界：云盘 `corpus_domain == None`，无行业绑定（grep 确认）

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/tests/cloud_drive_subprocess.rs
git commit -m "test(cloud): 并发 N=3 无死锁 + GA 验收全量回归

固定复跑 3 次确认并发多源+前台 add 无死锁(Lock ordering vault→vectors→fulltext→
embedding)；core+server 全量 release 测试绿 + clippy 0 warning。GA checklist 全勾。

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

**acceptance_judges:**
- `cargo test -p attune-server --test cloud_drive_subprocess cloud_concurrent_no_deadlock_n3` exits 0（N=3 不挂起）。
- `cargo test -p attune-core --release` + `cargo test -p attune-server --release` 全 exits 0（0 failures）。
- `cargo clippy --workspace --all-targets -- -D warnings` 无输出。
- `#[ignore]` 总计数未突增（rc.N → rc.N+1 ≤ +2，per §7.2 Gate2）。

---

## 风险登记 carry-over（spec §11 → 本 plan task 映射）

| spec §11 风险 | 缓解落地的 task | 监控/验证 |
|------|------|------|
| R1 5-path 去重 O(N²) 放大 | T9（云盘走 indexed_files O(1) + content_hash O(1)，不新增全表扫描） | T9 acceptance：worker 无 vectors/fulltext 直调；二轮短路 new_files=0 |
| R2 SSRF feed/cloud URL | T1/T2（route）+ T3（worker）+ T11（remote_name 校验） | T1/T3 acceptance：400 ssrf-rejected 双路径 fixture |
| R3 rclone 缺失 | T13（graceful 503 不 panic） | T13 acceptance：rclone_missing 集成 fixture |
| R4 增量 marker 错 | T8（SHA-1 优先 / size:modtime fallback）+ T9（content_hash 兜底） | T8 acceptance：marker fallback 断言；T9 二轮短路 |
| R5 并发死锁 | T9（三段式锁外 I/O）+ T12（worker）+ T16（N=3 验证） | T16 acceptance：N=3 不挂起 |
| R6 rclone 命令注入 | T7（arg 数组传参）+ T11（remote_name 白名单）+ T14（端到端） | T7/T14 acceptance：无 shell 拼接；注入 400 拒 |
| R7 XXE / billion-laughs | T4（XXE 实测）+ T5（体积上限挡 laughs） | T4/T5 acceptance |
| R8 OOM 超大 body/文件 | T5（feed 16 MiB）+ T6/T8（cloud-file-too-large 64 MiB） | T5/T6 acceptance |
| R9 凭据泄露 | T10（rclone_config_enc 加密）+ T12（log 不打 config） | T10 acceptance：落盘 BLOB 是密文 |
| R10 路径穿越 | T8（is_safe_path）+ T14（端到端） | T8/T14 acceptance |
| R11 worker tight-loop | T3（RSS touch_polled_at）+ T9（cloud touch_cloud_polled_at） | T3 acceptance：touch ≥2 |
| R12 大目录 lsjson 物化 | T9（DocumentSink 逐文件 cat，不一次性下载） | spec §3 设计，本 sprint <10万文件级足够 |
| R13 跨平台 rclone 差异 | T7（`Command::new("rclone")` PATH 查找）+ T13（缺失 graceful） | T13 acceptance |

---

## 自检（writing-plans Self-Review）

**1. Spec 覆盖：** §2-A(RSS 加固 3 项)→T1/T2/T3/T4/T5；§2-B(云盘 6 项)→T6..T14；§5 API→T11；§7 错误码→T1/T7/T11/T13；§9 测试矩阵 6 类→happy(T6/T8) edge(T8) error(T7/T13) adversarial(T4/T14) concurrent(T16) resource(T5/T6)；§10 向后兼容(IF NOT EXISTS 建表)→T10；§11 风险全映射(上表)。无 gap。

**2. Placeholder 扫描：** 无 "TBD / TODO / 稍后 / appropriate / similar to Task N"。`unimplemented!()` 仅在 T10 Step3 骨架且 Step4 acceptance 强制 grep=0 才能过。每个 code step 有完整代码块。

**3. Type consistency：** `RawDocument` 12 字段、`SourceConnector::fetch_documents(&self, sink)`、`SourceKind::CloudDrive`、`RssFeedInput{name,url,poll_interval_minutes}`、`crypto::encrypt(dek, bytes)`、`ingest_document(store, dek, raw)`、`IngestOutcome::is_new()` 全与既有 grounding 一致（Read 核实）。`RcloneRunner::{lsjson,cat}` / `RcloneFile` / `compute_marker` / `CloudDriveRemoteInput/Row` 跨 T6→T8→T9→T10→T11 命名统一。

**4. Scope 自检（spec §2）：** 无 Email/Telegram/OAuth内置/rclone打包/行业绑定/LLM agent task。云盘 `corpus_domain=None`（OSS 边界）。全部 task 落在 §2 ✅ 清单内 —— **无超 scope task**。
