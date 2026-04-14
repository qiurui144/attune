# vault-tauri

桌面客户端壳，包装 vault-server HTTP API 为原生桌面应用。

## 状态

**脚手架阶段** — 尚未完整实现。提供独立会话启动后续完善：

1. 安装 Tauri CLI: `cargo install tauri-cli --version "^2.0"`
2. 运行初始化：`cargo tauri init` 在本目录生成 `tauri.conf.json` 和 icons
3. 添加到 workspace：在 `npu-vault/Cargo.toml` 的 `members` 添加 `"crates/vault-tauri"`
4. 配置 `tauri.conf.json` 的 URL 指向嵌入式 server：`http://localhost:18900/`
5. 系统托盘：使用 `tauri-plugin-system-tray` 或 `tray-icon` crate

## 架构

```
Tauri Webview (加载 vault-server 的 Web UI)
    ↓
spawn vault-server as child process (or embed as library)
    ↓
本地 HTTP API 服务 (已存在)
```

## 最小可行集成

最简方式：Tauri 启动时 spawn `npu-vault-server --port 18900` 作为子进程，然后 webview 加载 `http://localhost:18900/`。用户关闭时 kill 子进程。

更深集成：将 vault-server 重构为 library，Tauri 直接调用 Axum router 而不是 subprocess。

## 依赖

```toml
[dependencies]
tauri = { version = "2", features = ["tray-icon"] }
tauri-plugin-shell = "2"
serde = { version = "1", features = ["derive"] }
```

## 启动后续工作

当前阶段保留此目录作为占位符，避免污染已完成的 vault-core / vault-server / vault-cli 构建。

全面实现在未来独立会话执行，预计工作量：
- 初始化 Tauri 项目（icons, bundle config）
- 系统托盘菜单 + 右键操作（lock / unlock / quit / status）
- Child process 管理
- Bundle 配置（AppImage / MSI / DMG）
- 自动更新 (tauri-plugin-updater)
