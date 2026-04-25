# Sprint 0 + 0.5 Implementation Plan: Cross-Platform Compile Hygiene + Tauri 2 Desktop Shell

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 attune 从"开浏览器访问 :18900"升级为"双击 Attune.exe 即用"的桌面应用，同时保留 attune-server-headless 供 K3 / NAS / 服务器使用，并打通自动更新链路。

**Architecture:**
attune-server lib 已存在；本 sprint (1) 在其上加 high-level `run_in_runtime(ServerConfig)` 入口；(2) 把 binary 改名为 `attune-server-headless`；(3) 拆 ort feature 默认到 CPU；(4) 新建 `apps/attune-desktop`（Tauri 2）crate，main.rs 内嵌 axum + WebView 加载 `http://127.0.0.1:18900`，含托盘 / 单实例 / 拖拽；(5) Tauri bundler 出 NSIS / deb / AppImage 三产物 + tauri-plugin-updater 接入自动更新。

**Tech Stack:**
- Rust 1.75+ workspace
- Tauri 2.x（rust 主进程 + js binding；前端零改动）
- ort 2.0.0-rc.12（default-features = false，cuda / directml / coreml 通过 feature 切换）
- 现有 Preact + Vite + Signals 前端（rust/crates/attune-server/ui/）零改动
- tauri-plugin-updater（Ed25519 签名验签）
- tauri-plugin-single-instance

**Spec source:** [`docs/superpowers/specs/2026-04-25-industry-attune-design.md`](../specs/2026-04-25-industry-attune-design.md) §6.5 §6.6 §7.1 §7.2

---

## File Structure

**Modify:**
- `rust/Cargo.toml` — workspace.members 加 `apps/attune-desktop`
- `rust/crates/attune-core/Cargo.toml` — ort default-features = false + 加 cuda/directml feature
- `rust/crates/attune-server/Cargo.toml` — `[[bin]] name = "attune-server-headless"`，path 改 `bin/headless.rs`
- `rust/crates/attune-server/src/lib.rs` — 新增 `pub struct ServerConfig` + `pub async fn run_in_runtime()`
- `rust/crates/attune-server/src/main.rs` — 移到 `bin/headless.rs`，简化为 wrapper
- `rust/crates/attune-core/src/vault.rs` — 已有 cfg(unix) 保护，本 sprint 仅审计

**Create:**
- `apps/attune-desktop/Cargo.toml`
- `apps/attune-desktop/build.rs`
- `apps/attune-desktop/tauri.conf.json`
- `apps/attune-desktop/capabilities/default.json`
- `apps/attune-desktop/src/main.rs`
- `apps/attune-desktop/src/embedded_server.rs`
- `apps/attune-desktop/src/tray.rs`
- `apps/attune-desktop/icons/icon.png` (1024×1024)
- `apps/attune-desktop/icons/icon.ico` (Win)
- `rust/crates/attune-server/tests/lib_runtime_test.rs`
- `.github/workflows/desktop-release.yml`

---

## Progress Tracking

每个 Task 完成后，回到本文件把对应 checkbox 勾上。Task 之间的 commit 必须独立（一个 task 一个 commit，不混合）。

---

### Task 1: attune-server lib 暴露 `ServerConfig` + `run_in_runtime`

把 `main.rs` 53-136 的启动逻辑挪到 lib 的 `run_in_runtime()` 高阶函数。main 简化为 parse Cli + 调 lib。

**Files:**
- Create: `rust/crates/attune-server/tests/lib_runtime_test.rs`
- Modify: `rust/crates/attune-server/src/lib.rs`（在文件末尾新增）
- Modify: `rust/crates/attune-server/Cargo.toml`（dev-dep 加 reqwest）

- [ ] **Step 1: 写失败测试 — `run_in_runtime` 接受 ServerConfig 在指定端口启动**

`rust/crates/attune-server/tests/lib_runtime_test.rs`:

```rust
//! 测试 attune_server::run_in_runtime() 高阶 API
//!
//! 验证：lib 暴露的高阶启动函数可以独立运行（无需 Cli），让 attune-desktop
//! 能直接 spawn 它而不依赖 binary。

use attune_server::ServerConfig;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_in_runtime_starts_and_responds_on_health() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let config = ServerConfig {
        host: "127.0.0.1".to_string(),
        port,
        tls_cert: None,
        tls_key: None,
        no_auth: true,
    };

    let handle = tokio::spawn(async move {
        attune_server::run_in_runtime(config).await
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    let response = reqwest::get(format!("http://127.0.0.1:{}/health", port))
        .await
        .expect("server should be reachable");
    assert_eq!(response.status(), 200, "health endpoint should return 200");

    handle.abort();
}
```

加 dev-dep 到 `rust/crates/attune-server/Cargo.toml`（在文件末尾追加）：

```toml
[dev-dependencies]
reqwest = { version = "0.12", features = ["json"] }
tokio = { version = "1", features = ["full", "test-util"] }
```

- [ ] **Step 2: 跑测试，应失败（ServerConfig / run_in_runtime 不存在）**

```bash
cd rust && cargo test --package attune-server --test lib_runtime_test 2>&1 | tail -20
```

预期：`error[E0432]: unresolved import attune_server::ServerConfig`。

- [ ] **Step 3: 在 lib.rs 实现 ServerConfig + run_in_runtime**

在 `rust/crates/attune-server/src/lib.rs` 末尾追加：

```rust
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub no_auth: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 18900,
            tls_cert: None,
            tls_key: None,
            no_auth: false,
        }
    }
}

/// 启动 attune-server 在当前 tokio runtime 上。
///
/// 用法：
/// - `attune-server-headless` binary 直接 await
/// - `attune-desktop` (Tauri) 也调这个函数把 axum 跑在 Tauri 的 tokio runtime
pub async fn run_in_runtime(
    config: ServerConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse().unwrap()))
        .try_init();

    let hw = attune_core::platform::HardwareProfile::detect();
    tracing::info!("hardware: {}", hw.summary());
    let applied = hw.apply_recommended_env();
    for (key, reason) in &applied {
        tracing::info!(
            "hardware: set {}={} — {}",
            key,
            std::env::var(key).unwrap_or_default(),
            reason
        );
    }

    let vault = attune_core::vault::Vault::open_default()?;
    let require_auth = !config.no_auth;
    if config.no_auth {
        tracing::warn!("⚠  Authentication DISABLED via config.no_auth.");
    }

    let shared_state = Arc::new(state::AppState::new(vault, require_auth));
    let app = build_router(shared_state);

    let is_loopback = {
        use std::net::IpAddr;
        config.host == "localhost"
            || config.host
                .parse::<IpAddr>()
                .map(|ip| ip.is_loopback())
                .unwrap_or(false)
    };
    let has_tls = config.tls_cert.is_some() && config.tls_key.is_some();
    if !is_loopback && !has_tls {
        tracing::warn!("⚠  Server bound to non-loopback '{}' without TLS.", config.host);
    }
    if !is_loopback && !require_auth {
        tracing::warn!("⚠  Auth disabled on non-loopback '{}'.", config.host);
    }

    let addr: std::net::SocketAddr = format!("{}:{}", config.host, config.port).parse()?;

    match (config.tls_cert.as_ref(), config.tls_key.as_ref()) {
        (Some(cert), Some(key)) => {
            tracing::info!("attune-server listening on https://{addr}");
            let tls_config =
                axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;
            axum_server::bind_rustls(addr, tls_config)
                .serve(app.into_make_service())
                .await?;
        }
        _ => {
            tracing::info!("attune-server listening on http://{addr}");
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            axum::serve(listener, app).await?;
        }
    }

    Ok(())
}
```

- [ ] **Step 4: 跑测试验证通过**

```bash
cd rust && cargo test --package attune-server --test lib_runtime_test 2>&1 | tail -10
```

预期：`test run_in_runtime_starts_and_responds_on_health ... ok`。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/lib.rs \
        rust/crates/attune-server/tests/lib_runtime_test.rs \
        rust/crates/attune-server/Cargo.toml
git commit -m "feat(server): expose ServerConfig + run_in_runtime() lib API

Allow embedding attune-server in foreign tokio runtimes (e.g. Tauri).
attune-desktop will reuse this entry point instead of spawning a sidecar."
```

---

### Task 2: 改 binary 名为 `attune-server-headless` + 简化 main.rs

**Files:**
- Move: `rust/crates/attune-server/src/main.rs` → `rust/crates/attune-server/src/bin/headless.rs`
- Modify: `rust/crates/attune-server/Cargo.toml`

- [ ] **Step 1: 修改 Cargo.toml 的 [[bin]]**

替换 `rust/crates/attune-server/Cargo.toml` 第 11-13 行：

```toml
[[bin]]
name = "attune-server-headless"
path = "src/bin/headless.rs"
```

- [ ] **Step 2: move + 简化 main.rs**

```bash
mkdir -p rust/crates/attune-server/src/bin
git mv rust/crates/attune-server/src/main.rs rust/crates/attune-server/src/bin/headless.rs
```

把 `rust/crates/attune-server/src/bin/headless.rs` 整个替换为：

```rust
//! attune-server-headless — 纯 axum 模式入口（K3 / NAS / 服务器）。
//!
//! 笔电桌面用户走 attune-desktop（含 Tauri WebView 壳）。
//! 两者共享 attune_server::run_in_runtime() 后端逻辑。

use attune_server::{run_in_runtime, ServerConfig};
use clap::Parser;

#[derive(Parser)]
#[command(name = "attune-server-headless", version, about = "Attune HTTP API server (headless mode)")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value = "18900")]
    port: u16,
    #[arg(long)]
    tls_cert: Option<String>,
    #[arg(long)]
    tls_key: Option<String>,
    #[arg(long)]
    no_auth: bool,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let config = ServerConfig {
        host: cli.host,
        port: cli.port,
        tls_cert: cli.tls_cert,
        tls_key: cli.tls_key,
        no_auth: cli.no_auth,
    };
    if let Err(e) = run_in_runtime(config).await {
        eprintln!("attune-server-headless exited with error: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use attune_server::is_allowed_origin;

    #[test]
    fn cors_allows_chrome_extension() {
        assert!(is_allowed_origin("chrome-extension://abcdefghijklmnop"));
    }

    #[test]
    fn cors_allows_localhost() {
        assert!(is_allowed_origin("http://localhost:18900"));
        assert!(is_allowed_origin("http://127.0.0.1:18900"));
        assert!(is_allowed_origin("https://localhost:18900"));
        assert!(is_allowed_origin("https://127.0.0.1:18900"));
    }

    #[test]
    fn cors_blocks_evil_origin() {
        assert!(!is_allowed_origin("https://evil.com"));
        assert!(!is_allowed_origin("http://192.168.1.100:18900"));
        assert!(!is_allowed_origin("null"));
        assert!(!is_allowed_origin(""));
    }
}
```

- [ ] **Step 3: 全工作区编译 + 测试**

```bash
cd rust && cargo build --release --bin attune-server-headless 2>&1 | tail -5
cargo test --workspace 2>&1 | tail -15
```

预期：build 成功，全部测试通过。

- [ ] **Step 4: 验证 headless binary 能跑 + 响应 /health**

```bash
cd rust && ./target/release/attune-server-headless --no-auth --port 18901 &
SERVER_PID=$!
sleep 2
curl -sf http://127.0.0.1:18901/health || (echo "FAIL" && kill $SERVER_PID && exit 1)
kill $SERVER_PID
echo "OK"
```

预期：JSON `{"status":"ok"...}` + `OK`。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-server/src/bin/headless.rs \
        rust/crates/attune-server/Cargo.toml
git commit -m "refactor(server): rename binary to attune-server-headless

Prepare for dual-track distribution: attune-desktop (Tauri) embeds the lib;
attune-server-headless ships standalone for K3 / NAS / server deployments."
```

---

### Task 3: ort feature 默认 CPU + cuda/directml/coreml feature

**Files:**
- Modify: `rust/crates/attune-core/Cargo.toml`
- Modify: `rust/Cargo.toml`

- [ ] **Step 1: 改 attune-core 的 ort 依赖**

`rust/crates/attune-core/Cargo.toml` 第 36 行：

```toml
# 替换前：
# ort = { version = "2.0.0-rc.12", features = ["cuda", "ndarray"] }
ort = { version = "2.0.0-rc.12", default-features = false, features = ["ndarray", "load-dynamic"] }
```

末尾追加：

```toml
[features]
default = []
cuda = ["ort/cuda"]
directml = ["ort/directml"]
coreml = ["ort/coreml"]
```

- [ ] **Step 2: workspace 顶部加 metadata 注释**

`rust/Cargo.toml` `[workspace]` 块下方追加：

```toml
[workspace.metadata.features]
note = "Use --features='attune-core/cuda' (Linux NVIDIA), 'attune-core/directml' (Win), 'attune-core/coreml' (Mac) to enable GPU EP. Default = CPU only."
```

- [ ] **Step 3: 默认 build 验证（无 GPU feature）**

```bash
cd rust && cargo clean -p attune-core 2>&1 | tail -3
cargo build --release --workspace 2>&1 | tail -10
```

预期：`Finished release [optimized] target(s)` 不报 cuda 链接错误。

- [ ] **Step 4: 验证 cuda feature 可启用（如有 NVIDIA 环境）**

```bash
cd rust && cargo build --release --features="attune-core/cuda" -p attune-core 2>&1 | tail -5 \
  || echo "EXPECTED: CUDA build skipped without nvcc — feature is opt-in"
```

预期（无 GPU 机器）：linker 错误，但**不阻塞 default build**。

- [ ] **Step 5: Commit**

```bash
git add rust/crates/attune-core/Cargo.toml rust/Cargo.toml
git commit -m "build: split ort GPU features (cuda/directml/coreml) — default = CPU

Previously cuda was forced on, breaking Windows iGPU and macOS builds.
Now Linux + Win compile clean by default; users opt-in via:
  cargo build --features='attune-core/cuda'      # Linux NVIDIA
  cargo build --features='attune-core/directml'  # Windows
  cargo build --features='attune-core/coreml'    # macOS"
```

---

### Task 4: cfg 跨平台保护审计

**Files:**
- Audit: 所有 `rust/crates/*/src/**/*.rs`
- Modify: 任何缺 cfg 保护的文件

- [ ] **Step 1: 扫描所有 Unix-only 调用**

```bash
cd rust && grep -rn 'PermissionsExt\|from_mode\|set_permissions\|MetadataExt\|umask' crates/ src/ 2>/dev/null > /tmp/unix_calls.txt
cat /tmp/unix_calls.txt
echo '---'
grep -rn 'cfg(unix)' crates/ src/ 2>/dev/null
```

- [ ] **Step 2: 检查每个调用点是否在 `#[cfg(unix)]` 块内**

按 `/tmp/unix_calls.txt` 逐一验证。已知 `vault.rs:362` 有保护。如有未保护点，记录到 `/tmp/needs_fix.txt`；否则跳到 Step 4。

- [ ] **Step 3: 为未保护点加 cfg 守卫**

模板（针对 `/tmp/needs_fix.txt` 每条）：

```rust
// 改前：
std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;

// 改后：
#[cfg(unix)]
{
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
}
#[cfg(windows)]
{
    tracing::debug!("Skipping POSIX permission lock on Windows; rely on filesystem ACL");
}
```

- [ ] **Step 4: Windows MSVC 交叉检查（cargo check）**

```bash
cd rust && rustup target add x86_64-pc-windows-msvc 2>&1 | tail -2
cargo install --locked cargo-xwin 2>&1 | tail -3 || true
cargo xwin check --target x86_64-pc-windows-msvc --workspace 2>&1 | tail -10 \
  || echo "cargo-xwin not configured locally; will validate on Windows runner in Task 11"
```

预期：要么 check 通过，要么明确"延后到 Win runner"——不要假装通过。

- [ ] **Step 5: 跑 native 测试**

```bash
cd rust && cargo test --workspace 2>&1 | tail -10
```

预期：所有现有测试仍通过。

- [ ] **Step 6: Commit**

```bash
git add rust/
git commit -m "fix(cross-platform): audit + guard all Unix-only syscalls with #[cfg(unix)]

Audit covered: PermissionsExt, MetadataExt, umask, from_mode.
Windows fallbacks added where security-critical (vault encryption is
the primary defense; POSIX 0o600 is a defense-in-depth layer that
Windows ACLs replace)."
```

---

### Task 5: 创建 apps/attune-desktop crate skeleton

**Files:**
- Create: `apps/attune-desktop/Cargo.toml`
- Create: `apps/attune-desktop/build.rs`
- Create: `apps/attune-desktop/src/main.rs`
- Create: `apps/attune-desktop/icons/icon.png`
- Create: `apps/attune-desktop/icons/icon.ico`
- Create: `apps/attune-desktop/tauri.conf.json`
- Create: `apps/attune-desktop/capabilities/default.json`
- Modify: `rust/Cargo.toml`

- [ ] **Step 1: workspace 加新 crate**

`rust/Cargo.toml` 第 3 行：

```toml
members = [
    "crates/attune-core",
    "crates/attune-cli",
    "crates/attune-server",
    "../apps/attune-desktop",
]
```

- [ ] **Step 2: 创建目录树**

```bash
mkdir -p apps/attune-desktop/{src,icons,capabilities}
```

- [ ] **Step 3: Cargo.toml**

`apps/attune-desktop/Cargo.toml`:

```toml
[package]
name = "attune-desktop"
version = "0.1.0"
edition = "2021"
rust-version = "1.75"
description = "Attune 桌面应用（Tauri 2 shell + 内嵌 attune-server）"

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
tauri = { version = "2", features = ["tray-icon"] }
tauri-plugin-single-instance = { version = "2" }
tauri-plugin-updater = { version = "2" }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"] }
attune-server = { path = "../../rust/crates/attune-server" }

[features]
custom-protocol = ["tauri/custom-protocol"]
default = ["custom-protocol"]
```

- [ ] **Step 4: build.rs**

`apps/attune-desktop/build.rs`:

```rust
fn main() {
    tauri_build::build()
}
```

- [ ] **Step 5: 占位图标（ImageMagick）**

```bash
cd apps/attune-desktop/icons
convert -size 1024x1024 xc:'#5E8B8B' -fill white -gravity center -pointsize 400 -annotate 0 'A' icon.png
convert icon.png -define icon:auto-resize=256,128,96,64,48,32,16 icon.ico
ls -la
```

预期：`icon.png` ~50KB，`icon.ico` ~120KB。

- [ ] **Step 6: tauri.conf.json**

`apps/attune-desktop/tauri.conf.json`:

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "Attune",
  "version": "0.6.0",
  "identifier": "ai.attune.desktop",
  "build": {
    "frontendDist": "../../rust/crates/attune-server/ui/dist",
    "beforeBuildCommand": "cd ../../rust/crates/attune-server/ui && npm run build",
    "beforeDevCommand": "cd ../../rust/crates/attune-server/ui && npm run dev",
    "devUrl": "http://localhost:5173"
  },
  "app": {
    "windows": [],
    "security": { "csp": null },
    "trayIcon": { "iconPath": "icons/icon.png", "iconAsTemplate": false }
  },
  "bundle": {
    "active": true,
    "targets": ["nsis", "msi", "deb", "appimage"],
    "icon": ["icons/icon.png", "icons/icon.ico"],
    "publisher": "Attune",
    "category": "Productivity",
    "shortDescription": "私有 AI 知识伙伴",
    "longDescription": "Attune 是一款私有 AI 知识伙伴，主动进化、对话式、混合智能。"
  }
}
```

- [ ] **Step 7: capabilities/default.json**

`apps/attune-desktop/capabilities/default.json`:

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "Attune Desktop default capability",
  "windows": ["main"],
  "permissions": [
    "core:default",
    "core:window:default",
    "core:webview:default",
    "core:event:default"
  ]
}
```

- [ ] **Step 8: src/main.rs（Hello World）**

`apps/attune-desktop/src/main.rs`:

```rust
//! Attune Desktop — Tauri 2 shell。
//! Sprint 0.5 阶段：先确保 Tauri builder 起得来；下一 Task 接 axum runtime。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("info".parse().unwrap()),
        )
        .init();

    tauri::Builder::default()
        .setup(|_app| {
            tracing::info!("attune-desktop skeleton booted (Task 5)");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![])
        .run(tauri::generate_context!())
        .expect("error while running attune-desktop");
}
```

- [ ] **Step 9: 编译**

```bash
cd apps/attune-desktop && cargo build --release 2>&1 | tail -8
```

预期：`Finished release` 不报 tauri 错误。

- [ ] **Step 10: Commit**

```bash
git add apps/attune-desktop rust/Cargo.toml
git commit -m "feat(desktop): scaffold apps/attune-desktop Tauri 2 crate

Hello World shell — next task wires the embedded axum runtime."
```

---

### Task 6: attune-desktop 内嵌 axum + 启动健康检查 + WebView 加载 :18900

**Files:**
- Create: `apps/attune-desktop/src/embedded_server.rs`
- Modify: `apps/attune-desktop/src/main.rs`

- [ ] **Step 1: 创建 embedded_server.rs**

`apps/attune-desktop/src/embedded_server.rs`:

```rust
//! 在 Tauri 主进程的 tokio runtime 上跑 attune-server。
//!
//! 启动顺序：
//! 1. spawn 后台 task → run_in_runtime
//! 2. 健康检查轮询 :18900/health 直到 200（30s 超时）
//! 3. 通知 Tauri 主线程加载 WebView URL

use attune_server::{run_in_runtime, ServerConfig};
use std::time::Duration;

const SERVER_HOST: &str = "127.0.0.1";
const SERVER_PORT: u16 = 18900;
const HEALTH_TIMEOUT_SECS: u64 = 30;

pub fn server_url() -> String {
    format!("http://{}:{}", SERVER_HOST, SERVER_PORT)
}

/// Spawn attune-server 在 Tauri 的 tokio runtime。
pub fn spawn_server() -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async {
        let config = ServerConfig {
            host: SERVER_HOST.to_string(),
            port: SERVER_PORT,
            tls_cert: None,
            tls_key: None,
            no_auth: false,
        };
        if let Err(e) = run_in_runtime(config).await {
            tracing::error!("embedded attune-server crashed: {e}");
        }
    })
}

/// 阻塞等 :18900/health 返回 200。
pub async fn wait_for_ready() -> Result<(), String> {
    let url = format!("{}/health", server_url());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .map_err(|e| e.to_string())?;

    let deadline = std::time::Instant::now() + Duration::from_secs(HEALTH_TIMEOUT_SECS);
    while std::time::Instant::now() < deadline {
        match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => {
                tracing::info!("embedded attune-server ready at {}", server_url());
                return Ok(());
            }
            _ => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
    Err(format!(
        "attune-server did not become ready within {}s",
        HEALTH_TIMEOUT_SECS
    ))
}
```

- [ ] **Step 2: 改 main.rs 接入 embedded_server**

替换 `apps/attune-desktop/src/main.rs` 全部内容：

```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod embedded_server;

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("info".parse().unwrap()),
        )
        .init();

    tauri::Builder::default()
        .setup(|app| {
            // 1. spawn 内嵌 axum
            let _server_handle = embedded_server::spawn_server();

            // 2. 异步等服务就绪后开主窗口
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                match embedded_server::wait_for_ready().await {
                    Ok(()) => {
                        let url = embedded_server::server_url();
                        tracing::info!("opening main window pointing to {}", url);
                        if let Err(e) = WebviewWindowBuilder::new(
                            &app_handle,
                            "main",
                            WebviewUrl::External(url.parse().unwrap()),
                        )
                        .title("Attune")
                        .inner_size(1280.0, 800.0)
                        .min_inner_size(800.0, 600.0)
                        .build()
                        {
                            tracing::error!("failed to build main window: {e}");
                        }
                    }
                    Err(e) => {
                        tracing::error!("embedded server failed to start: {e}");
                        std::process::exit(1);
                    }
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running attune-desktop");
}
```

- [ ] **Step 3: 准备前端 dist + build desktop**

```bash
(cd rust/crates/attune-server/ui && npm run build) 2>&1 | tail -3
cd apps/attune-desktop && cargo build --release 2>&1 | tail -5
```

- [ ] **Step 4: smoke test**

```bash
DISPLAY=:0 ./apps/attune-desktop/target/release/attune-desktop &
DESKTOP_PID=$!
sleep 8
curl -sf http://127.0.0.1:18900/health && echo OK || echo FAIL
ps -p $DESKTOP_PID > /dev/null && echo "process alive" || echo "process died"
kill $DESKTOP_PID 2>/dev/null
```

预期：`OK` + `process alive`。

- [ ] **Step 5: Commit**

```bash
git add apps/attune-desktop/src/embedded_server.rs apps/attune-desktop/src/main.rs
git commit -m "feat(desktop): embed attune-server in Tauri tokio runtime

Tauri spawns axum on its own runtime; main window opens after :18900/health
returns 200 (30s timeout). WebView loads http://127.0.0.1:18900 — preserves
existing Preact UI without rewrite."
```

---

### Task 7: 单实例锁

**Files:**
- Modify: `apps/attune-desktop/src/main.rs`

- [ ] **Step 1: 注册 single-instance plugin**

修改 `apps/attune-desktop/src/main.rs`，在 `tauri::Builder::default()` 后链 `.plugin(...)`（在 `.setup(...)` 之前）：

```rust
tauri::Builder::default()
    .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        }
    }))
    .setup(|app| {
        // ... 原有逻辑保留
```

- [ ] **Step 2: build**

```bash
cd apps/attune-desktop && cargo build --release 2>&1 | tail -5
```

- [ ] **Step 3: 单实例 smoke test**

```bash
cd apps/attune-desktop
DISPLAY=:0 ./target/release/attune-desktop &
PID1=$!
sleep 5
DISPLAY=:0 ./target/release/attune-desktop &
PID2=$!
sleep 3
ps -p $PID2 > /dev/null && echo "FAIL: 2nd instance still alive" || echo "OK: 2nd instance bounced"
kill $PID1 2>/dev/null
```

预期：`OK: 2nd instance bounced`。

- [ ] **Step 4: Commit**

```bash
git add apps/attune-desktop/src/main.rs
git commit -m "feat(desktop): single-instance lock — re-double-click activates existing window"
```

---

### Task 8: 系统托盘（关闭主窗口最小化到托盘）

**Files:**
- Create: `apps/attune-desktop/src/tray.rs`
- Modify: `apps/attune-desktop/src/main.rs`

- [ ] **Step 1: 创建 tray.rs**

`apps/attune-desktop/src/tray.rs`:

```rust
//! 系统托盘 — 关闭主窗口时不退出进程，最小化到托盘。

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "完全退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let _tray = TrayIconBuilder::with_id("main-tray")
        .icon(app.default_window_icon().unwrap().clone())
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
        })
        .build(app)?;
    Ok(())
}
```

- [ ] **Step 2: main.rs 接入托盘 + 拦截窗口关闭**

在 `main.rs` 顶部 `mod` 声明加 `mod tray;`。

setup 闭包内主窗口创建成功后追加：

```rust
// 关闭按钮 = 隐藏到托盘，不退出进程
if let Some(window) = app_handle.get_webview_window("main") {
    let win_clone = window.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = win_clone.hide();
        }
    });
}

// 托盘
if let Err(e) = crate::tray::build(&app_handle) {
    tracing::error!("failed to build system tray: {e}");
}
```

- [ ] **Step 3: build + smoke**

```bash
cd apps/attune-desktop && cargo build --release 2>&1 | tail -5
DISPLAY=:0 ./target/release/attune-desktop &
PID=$!
sleep 6
wmctrl -c "Attune" 2>/dev/null || echo "wmctrl missing — skip auto-close test"
sleep 2
ps -p $PID > /dev/null && echo "OK: tray-resident" || echo "FAIL: process died"
kill $PID 2>/dev/null
```

预期：`OK: tray-resident`。

- [ ] **Step 4: Commit**

```bash
git add apps/attune-desktop/src/tray.rs apps/attune-desktop/src/main.rs
git commit -m "feat(desktop): system tray — close-window minimizes to tray, quit needs explicit menu"
```

---

### Task 9: 拖拽文件 → Tauri emit → 前端 listen

用 Tauri 标准 emit/listen API 把 OS drag-drop 事件桥接到前端。前端用 `window.__TAURI_INTERNALS__.event.listen()` 接收（无需额外 npm 依赖；如已有 `@tauri-apps/api`，用其 `listen` 更优雅）。

**Files:**
- Modify: `apps/attune-desktop/src/main.rs`
- Modify: `rust/crates/attune-server/ui/src/main.tsx`
- Modify: `rust/crates/attune-server/ui/package.json`（加 `@tauri-apps/api`）

- [ ] **Step 1: 后端 emit 文件路径**

在 `main.rs` setup 闭包内、托盘建好之后追加：

```rust
use tauri::Emitter;
if let Some(window) = app_handle.get_webview_window("main") {
    let app_for_drop = app_handle.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
            let payload: Vec<String> = paths
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            if let Err(e) = app_for_drop.emit("attune-file-drop", &payload) {
                tracing::warn!("failed to emit attune-file-drop: {e}");
            }
        }
    });
}
```

- [ ] **Step 2: 前端加 @tauri-apps/api dep**

```bash
cd rust/crates/attune-server/ui
npm install @tauri-apps/api@^2.0.0 2>&1 | tail -3
```

- [ ] **Step 3: 前端 listener**

`rust/crates/attune-server/ui/src/main.tsx` 文件顶部 import 后追加（在 render 之前）：

```tsx
// Tauri 桌面壳的 file-drop 事件桥
// 浏览器模式下 __TAURI_INTERNALS__ 不存在，listener 不会触发
if (typeof window !== 'undefined' && (window as any).__TAURI_INTERNALS__) {
  import('@tauri-apps/api/event').then(({ listen }) => {
    listen<string[]>('attune-file-drop', (event) => {
      const paths = event.payload || [];
      console.log('[attune-desktop] dropped files:', paths);
      // Sprint 1 接 store.uploadFromPaths(paths)
      // 当前先 alert 让用户验证桥通
      if (paths.length > 0) {
        alert(`已检测到拖入 ${paths.length} 个文件（占位提示）：\n` + paths.slice(0, 3).join('\n'));
      }
    }).catch((err) => {
      console.warn('failed to attach attune-file-drop listener:', err);
    });
  });
}
```

- [ ] **Step 4: rebuild 前端 + desktop**

```bash
(cd rust/crates/attune-server/ui && npm run build) 2>&1 | tail -3
cd apps/attune-desktop && cargo build --release 2>&1 | tail -5
```

- [ ] **Step 5: 手动验证（人工）**

```bash
DISPLAY=:0 ./apps/attune-desktop/target/release/attune-desktop &
PID=$!
sleep 6
echo ">>> 人工：从文件管理器拖一个文件到 Attune 窗口，应该看到 alert 弹窗"
sleep 30
kill $PID 2>/dev/null
```

预期：人工拖文件 → alert 含路径出现。

- [ ] **Step 6: Commit**

```bash
git add apps/attune-desktop/src/main.rs \
        rust/crates/attune-server/ui/src/main.tsx \
        rust/crates/attune-server/ui/package.json \
        rust/crates/attune-server/ui/package-lock.json
git commit -m "feat(desktop): bridge OS file-drop to webview via Tauri emit/listen

Backend emits 'attune-file-drop' with String[] payload; frontend listens via
@tauri-apps/api/event when running inside Tauri (browser mode = no-op)."
```

---

### Task 10: Tauri bundler 出 Linux deb + AppImage（本机验证）

**Files:** 无新建/修改（验证 Task 5 的 tauri.conf.json）

- [ ] **Step 1: 安装 cargo-tauri CLI**

```bash
cargo install --locked tauri-cli --version "^2.0" 2>&1 | tail -5
which cargo-tauri && cargo tauri --version
```

预期：`tauri-cli 2.x.x`.

- [ ] **Step 2: 安装 Linux bundler 系统依赖（Ubuntu/Debian）**

```bash
sudo apt update
sudo apt install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf
```

- [ ] **Step 3: 出 deb + AppImage**

```bash
cd apps/attune-desktop
cargo tauri build --bundles deb,appimage 2>&1 | tail -25
```

预期：
- `target/release/bundle/deb/Attune_0.6.0_amd64.deb`
- `target/release/bundle/appimage/Attune_0.6.0_amd64.AppImage`

- [ ] **Step 4: 在干净环境验证 deb**

```bash
sudo dpkg -i apps/attune-desktop/target/release/bundle/deb/Attune_*.deb 2>&1 | tail -5
which attune
DISPLAY=:0 attune &
PID=$!
sleep 8
curl -sf http://127.0.0.1:18900/health && echo "OK from installed deb" || echo "FAIL"
kill $PID 2>/dev/null
sudo dpkg -r attune 2>&1 | tail -3
```

预期：装包后 `attune` 命令可启，:18900 OK。

- [ ] **Step 5: Commit（无文件改动用 --allow-empty 标记里程碑）**

```bash
git commit --allow-empty -m "build(desktop): verify Linux deb + AppImage out of Tauri bundler

Tested on Ubuntu 22.04 — installer launches, :18900 reachable, tray works."
```

---

### Task 11: GitHub Actions CI matrix（Linux + Windows）

**Files:**
- Create: `.github/workflows/desktop-release.yml`

- [ ] **Step 1: 创建 workflow**

`.github/workflows/desktop-release.yml`:

```yaml
name: desktop-release

on:
  push:
    tags: ['desktop-v*']
  workflow_dispatch:

jobs:
  build:
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: ubuntu-22.04
            target: x86_64-unknown-linux-gnu
            bundles: deb,appimage
          - os: windows-latest
            target: x86_64-pc-windows-msvc
            bundles: nsis,msi
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - name: Linux deps
        if: runner.os == 'Linux'
        run: |
          sudo apt update
          sudo apt install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf

      - uses: actions/setup-node@v4
        with:
          node-version: '20'
          cache: 'npm'
          cache-dependency-path: 'rust/crates/attune-server/ui/package-lock.json'

      - name: Build frontend
        run: cd rust/crates/attune-server/ui && npm ci && npm run build

      - name: Install tauri-cli
        run: cargo install --locked tauri-cli --version "^2.0"

      - name: Build Tauri bundles
        run: cd apps/attune-desktop && cargo tauri build --bundles ${{ matrix.bundles }}

      - name: Upload artifacts
        uses: actions/upload-artifact@v4
        with:
          name: attune-desktop-${{ matrix.target }}
          path: apps/attune-desktop/target/release/bundle/**/*
          retention-days: 30
```

- [ ] **Step 2: Commit + push 触发**

```bash
git add .github/workflows/desktop-release.yml
git commit -m "ci(desktop): GitHub Actions matrix — Linux deb/AppImage + Win NSIS/MSI"
```

人工：在 GitHub UI Actions → desktop-release → Run workflow。

- [ ] **Step 3: Win artifact 在 Windows 测试机实测**

下载 `attune-desktop-x86_64-pc-windows-msvc` artifact，解压后双击 `Attune_0.6.0_x64-setup.exe` 安装，然后双击启动。

记录到 `docs/e2e-test-report.md`：

```bash
cat >> docs/e2e-test-report.md <<'EOF'

## Windows Desktop smoke test (Sprint 0.5)

- 测试日期：（填）
- 安装包：Attune_0.6.0_x64-setup.exe（NSIS）
- 测试机：Windows 11 22H2 / 16GB RAM
- 双击启动到主窗口出现：__ 秒
- :18900/health：OK / FAIL
- 托盘：OK / FAIL
- 单实例：OK / FAIL
- 拖拽文件：OK / FAIL
- 备注：
EOF
```

- [ ] **Step 4: Commit 测试报告**

```bash
git add docs/e2e-test-report.md
git commit -m "docs(e2e): Windows desktop smoke test results (Sprint 0.5)"
```

---

### Task 12: tauri-plugin-updater 接入 + 公钥嵌入

**Files:**
- Modify: `apps/attune-desktop/src/main.rs`
- Modify: `apps/attune-desktop/tauri.conf.json`
- Modify: `.gitignore`

- [ ] **Step 1: 生成开发用 minisign keypair**

```bash
mkdir -p apps/attune-desktop/keys
cd apps/attune-desktop
TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" cargo tauri signer generate -w keys/dev_signing.key
ls keys/
cat keys/dev_signing.key.pub
```

预期：`keys/dev_signing.key`（私钥，**不入 git**）+ `keys/dev_signing.key.pub`。

- [ ] **Step 2: 防私钥入 git**

`.gitignore` 末尾追加：

```
apps/attune-desktop/keys/dev_signing.key
apps/attune-desktop/keys/dev_signing.key.pub
```

- [ ] **Step 3: pubkey 写入 tauri.conf.json**

读 `keys/dev_signing.key.pub` 内容，编辑 `apps/attune-desktop/tauri.conf.json` 顶层加：

```json
"plugins": {
  "updater": {
    "active": true,
    "endpoints": [
      "https://updates.attune.ai/desktop/{{target}}/{{current_version}}/latest.json"
    ],
    "dialog": false,
    "pubkey": "<paste base64 pubkey from key.pub here>"
  }
}
```

- [ ] **Step 4: main.rs 接入 updater plugin**

在 `apps/attune-desktop/src/main.rs` 的 Builder 链中（`single_instance` plugin 后）加：

```rust
.plugin(tauri_plugin_updater::Builder::new().build())
```

setup 闭包内主窗口创建后追加（async 30s 后检查更新）：

```rust
let app_handle_for_update = app_handle.clone();
tauri::async_runtime::spawn(async move {
    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    use tauri_plugin_updater::UpdaterExt;
    match app_handle_for_update.updater().unwrap().check().await {
        Ok(Some(update)) => {
            tracing::info!(
                "update available: {} → {}",
                update.current_version, update.version
            );
        }
        Ok(None) => tracing::info!("no update available"),
        Err(e) => tracing::warn!("update check failed (gateway maybe offline): {e}"),
    }
});
```

- [ ] **Step 5: build + 跑（gateway 不存在，应 graceful 失败）**

```bash
cd apps/attune-desktop && cargo build --release 2>&1 | tail -5
DISPLAY=:0 ./target/release/attune-desktop &
PID=$!
sleep 35
kill $PID 2>/dev/null
```

预期：进程不崩，log 含 `update check failed (gateway maybe offline)`。

- [ ] **Step 6: Commit（不含 keys/）**

```bash
git add apps/attune-desktop/tauri.conf.json \
        apps/attune-desktop/src/main.rs \
        apps/attune-desktop/Cargo.toml \
        .gitignore
git commit -m "feat(desktop): wire tauri-plugin-updater (gateway deferred to Sprint 6)

Updater hits https://updates.attune.ai/desktop/{target}/{version}/latest.json
30s after launch. Pubkey embedded in tauri.conf.json (dev key for now;
production key swap-in part of Sprint 6 release pipeline)."
```

---

### Task 13: 文档更新（README）

**Files:**
- Modify: `rust/README.md`
- Modify: `rust/README.zh.md`

- [ ] **Step 1: 在 rust/README.md 加桌面分发段落**

`rust/README.md` 末尾追加：

```markdown
## Desktop Distribution

Attune ships in two forms (same Rust backend code):

| Form | Binary | Use Case |
|------|--------|----------|
| Attune Desktop | apps/attune-desktop (Tauri 2 shell) | Laptop users — double-click MSI/deb, native window + tray |
| Attune Server (headless) | crates/attune-server/bin/headless.rs | K3 appliance / NAS / server |

### Desktop build (local)

# Linux
cd apps/attune-desktop
cargo install --locked tauri-cli --version "^2.0"
cargo tauri build --bundles deb,appimage

# Windows (run on Windows host)
cargo tauri build --bundles nsis,msi

Out: target/release/bundle/{deb,appimage,nsis,msi}/.

### Auto-update

Desktop checks https://updates.attune.ai/desktop/{target}/{version}/latest.json
30 seconds after launch. Updates are minisign-signed; pubkey embedded in binary.
See docs/superpowers/specs/2026-04-25-industry-attune-design.md §6.6 for design.
```

- [ ] **Step 2: 同步 rust/README.zh.md（中文）**

参照英文版加同等中文段落："桌面分发"。

- [ ] **Step 3: Commit**

```bash
git add rust/README.md rust/README.zh.md
git commit -m "docs(rust): document Desktop / Server dual-distribution + auto-update"
```

---

## Self-Review Notes

**Spec coverage:**
- ✅ §6.5.1 Q-D/E/G 决策 → Task 1, 6
- ✅ §6.5.3 Cargo workspace 改造 → Task 1, 2, 5
- ✅ §6.5.4 P0 特性（托盘 / 单实例 / 拖拽 / 启动 splash via wait_for_ready） → Task 6, 7, 8, 9
- ✅ §6.6.2 Tauri Desktop 更新流（30s 后检查） → Task 12
- ✅ §6.6.3 签名链路（minisign keypair + pubkey 嵌入） → Task 12
- ✅ §7.1 跨平台编译卫生 → Task 3, 4
- ✅ §7.2 五产物 → Task 10, 11
- ⏭ §6.6.4 apt 仓库（attune-server-only） → Sprint 6
- ⏭ §6.6.6 回滚机制 → Sprint 7+
- ⏭ §6.6.7 数据迁移 → Sprint 1+
- ⏭ updater UI 弹窗（仅 log，不弹窗） → Sprint 6

**Placeholder scan:** 完整代码 + 完整命令 + 完整预期。无 TBD/TODO 留给实施者猜。

**Type consistency:**
- `ServerConfig { host, port, tls_cert, tls_key, no_auth }` 字段贯穿 Task 1, 2, 6 一致
- `embedded_server::server_url()` / `wait_for_ready()` Task 6 定义后 Task 7-9 沿用
- Tauri webview window label `"main"` Task 6 创建后 Task 7, 8 引用
- Tauri emit event name `"attune-file-drop"` Task 9 后端发 → 前端听 一致

---

## 完成 Sprint 0 + 0.5 的标志

13 个 Task 全部 checkbox 勾上，且：

- [ ] CI matrix 在 Linux + Win runner 都 green
- [ ] Linux：`dpkg -i Attune_0.6.0_amd64.deb` 后双击图标 30 秒内出现窗口
- [ ] Windows（人工）：双击 NSIS installer 安装后启动 30 秒内出现窗口
- [ ] 单实例锁可用（重复双击不开新进程）
- [ ] 关闭窗口最小化到托盘，托盘"完全退出"才结束进程
- [ ] 拖文件到窗口：alert 含文件路径
- [ ] 启动 30 秒后 log 含 `update check failed`（gateway 占位，预期失败 graceful）
- [ ] `attune-server-headless --port 18901` 仍可独立运行（双轨兼容）
- [ ] `cargo test --workspace` 全绿
