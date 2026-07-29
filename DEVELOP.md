# 开发指南

Attune 是 Rust-first 仓库。当前运行时代码在 Rust workspace、嵌入式 Web UI、Chrome 扩展和 Tauri 桌面壳中维护。

## 分支模型

仓库采用简化 GitFlow：

| 分支 | 用途 | 推送方式 |
|------|------|---------|
| `main` | 稳定发布线。正式 `vX.Y.Z` / `desktop-vX.Y.Z` tag 从这里出。 | 仅通过 `develop -> main` 发布合入 |
| `develop` | 集成线。日常开发汇总。 | 通过 feature PR 合入 |
| `feature/<name>` | 短期特性分支。 | 本地开发 -> push -> PR -> merge 后删除 |

## 构建

### Rust 后端 / CLI

```bash
cd rust
cargo build --workspace
cargo build --release -p attune-server -p attune-cli
```

### 嵌入式 Web UI

```bash
cd rust/crates/attune-server/ui
npm ci
npm run build
```

### Chrome 扩展

```bash
cd extension
npm ci
npm run build
```

### Tauri 桌面

```bash
cd apps/attune-desktop
cargo tauri build
```

## 检查与测试

```bash
cd rust
cargo fmt --all --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

扩展：

```bash
cd extension
npm run build
```

仓库维护门：

```bash
bash scripts/maintenance-audit.sh
bash scripts/privacy-audit.sh
```

测试金字塔入口：

```bash
bash scripts/test-pyramid.sh
bash scripts/test-pyramid.sh --with-e2e
```

## 发布

Attune 双轨发布：

| Tag 前缀 | Workflow | 产物 |
|---------|----------|------|
| `vX.Y.Z` | `.github/workflows/rust-release.yml` | server / CLI tarball |
| `desktop-vX.Y.Z` | `.github/workflows/desktop-release.yml` | Tauri 桌面安装器 |

正式 GA 只在 `main` 打 tag；`develop` 只用于 alpha/beta/rc 集成验证。

## 目录

```text
rust/                         Rust workspace
rust/crates/attune-core/      核心库：加密、存储、搜索、agent、OCR、LLM 等
rust/crates/attune-server/    Axum HTTP API + embedded UI
rust/crates/attune-cli/       CLI
rust/crates/attune-server/ui/ Preact + Vite Web UI
extension/                    Chrome MV3 扩展
apps/attune-desktop/          Tauri 桌面壳
tests/e2e/                    仓库级 E2E 脚本
```
