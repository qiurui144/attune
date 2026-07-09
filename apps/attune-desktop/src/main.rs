#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod desktop;
mod embedded_server;
mod tray;
mod update_feed;

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tracing_subscriber::fmt::writer::MakeWriterExt;

/// Auto-updater 状态机:UI 通过监听 `attune-update-status` 事件获得这些状态.
/// 维持纯字符串(不引入额外 serde 类型),前端 JS 直接 switch.
const EV_UPDATE_STATUS: &str = "attune-update-status";

/// Tauri command:UI 主动触发检查更新.成功命中时 (latest > current) 先 emit
/// `available`,随后下载+安装 (含进度 emit `downloading` / `installing`),完成 emit
/// `ready`,失败 emit `error`.无新版返回 false 不 emit.
///
/// 返回 Ok(true) = 有更新且已开始下载; Ok(false) = 无更新; Err = 检查/下载/安装失败.
#[tauri::command]
async fn check_for_update_now(app: AppHandle, source: Option<String>) -> Result<bool, String> {
    use tauri_plugin_updater::UpdaterExt;
    // Resolve feed endpoints at runtime (company mirror first, GitHub fallback)
    // instead of the compile-time tauri.conf.json default. Signature pubkey is
    // unchanged → still verified against whichever endpoint serves latest.json.
    let endpoints = update_feed::resolve_endpoints_for_source(source.as_deref());
    let _ = app.emit(EV_UPDATE_STATUS, serde_json::json!({"state": "checking"}));
    let updater = app
        .updater_builder()
        .endpoints(endpoints.iter().filter_map(|e| e.parse().ok()).collect())
        .map_err(|e| e.to_string())?
        .build()
        .map_err(|e| e.to_string())?;
    let update = match updater.check().await.map_err(|e| e.to_string())? {
        Some(u) => u,
        None => {
            tracing::info!("manual update check: no update available");
            let _ = app.emit(EV_UPDATE_STATUS, serde_json::json!({"state": "up-to-date"}));
            return Ok(false);
        }
    };
    let current = update.current_version.clone();
    let next = update.version.clone();
    tracing::info!("update available {} -> {}", current, next);
    let _ = app.emit(
        EV_UPDATE_STATUS,
        serde_json::json!({"state": "available", "from": current, "to": next}),
    );

    // download_and_install 一步走完;进度回调中 emit downloading 比例
    let app_for_progress = app.clone();
    update
        .download_and_install(
            move |chunk, total| {
                if let Some(total) = total {
                    let pct = if total > 0 {
                        ((chunk as f64 / total as f64) * 100.0) as u32
                    } else {
                        0
                    };
                    let _ = app_for_progress.emit(
                        EV_UPDATE_STATUS,
                        serde_json::json!({"state": "downloading", "percent": pct}),
                    );
                }
            },
            || {
                tracing::info!("update downloaded, ready to install");
            },
        )
        .await
        .map_err(|e| {
            let msg = e.to_string();
            let _ = app.emit(
                EV_UPDATE_STATUS,
                serde_json::json!({"state": "error", "message": msg.clone()}),
            );
            msg
        })?;

    let _ = app.emit(EV_UPDATE_STATUS, serde_json::json!({"state": "ready"}));
    tracing::info!("update installed; user must restart");
    Ok(true)
}

/// Tauri command:用户在 UI 上点 "重启应用" 后调用此 command 完成重启.
/// 仅触发 app.restart(),不做其他副作用.
#[tauri::command]
fn restart_for_update(app: AppHandle) {
    tracing::info!("restart-for-update invoked");
    app.restart();
}

#[tauri::command]
fn desktop_app_info() -> desktop::DesktopAppInfo {
    desktop::desktop_app_info()
}

#[tauri::command]
fn open_desktop_path(kind: String) -> Result<String, String> {
    desktop::open_desktop_path(&kind)
}

#[tauri::command]
fn create_diagnostic_bundle() -> Result<String, String> {
    desktop::create_diagnostic_bundle()
}

#[tauri::command]
fn reveal_diagnostic_bundle(path: String) -> Result<(), String> {
    desktop::reveal_diagnostic_bundle(&path)
}

#[tauri::command]
fn get_launch_at_login() -> desktop::LaunchAtLoginState {
    desktop::get_launch_at_login()
}

#[tauri::command]
fn set_launch_at_login(enabled: bool) -> Result<desktop::LaunchAtLoginState, String> {
    desktop::set_launch_at_login(enabled)
}

#[tauri::command]
fn get_close_behavior() -> desktop::DesktopPreferences {
    desktop::load_preferences()
}

#[tauri::command]
fn set_close_behavior(close_action: String) -> Result<desktop::DesktopPreferences, String> {
    desktop::set_close_action(&close_action)
}

/// Tauri command: upload local file paths to the embedded server's /api/v1/upload endpoint.
/// Called by the web UI after receiving an `attune-file-drop` event.
#[tauri::command]
async fn upload_dropped_paths(paths: Vec<String>) -> Result<Vec<String>, String> {
    let client = reqwest::Client::new();
    let token = std::env::var("ATTUNE_DEV_TOKEN").unwrap_or_default();
    let upload_url = format!("{}/api/v1/upload", embedded_server::server_url());
    let mut results = Vec::new();
    for path_str in paths {
        let path = std::path::Path::new(&path_str);
        if !path.exists() || !path.is_file() {
            results.push(format!("skip:{path_str}"));
            continue;
        }
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown".to_string());
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                results.push(format!("error:{path_str}:{e}"));
                continue;
            }
        };
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(file_name.clone())
            .mime_str("application/octet-stream")
            .map_err(|e| e.to_string())?;
        let form = reqwest::multipart::Form::new().part("file", part);
        let mut req = client.post(&upload_url).multipart(form);
        if !token.is_empty() {
            req = req.bearer_auth(&token);
        }
        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                results.push(format!("ok:{file_name}"));
            }
            Ok(resp) => {
                results.push(format!("fail:{file_name}:{}", resp.status()));
            }
            Err(e) => {
                results.push(format!("error:{file_name}:{e}"));
            }
        }
    }
    Ok(results)
}

#[derive(serde::Serialize)]
struct LocalFilePayload {
    file_name: String,
    bytes: Vec<u8>,
}

/// Tauri command: read a user-selected local file into the web UI.
///
/// The UI only calls this after the native file picker returns a path. Keep the
/// payload bounded so a mistaken selection cannot pin the webview with a huge
/// byte array.
#[tauri::command]
fn read_local_file(path: String) -> Result<LocalFilePayload, String> {
    const MAX_PICKER_READ_BYTES: u64 = 512 * 1024 * 1024;

    let path = std::path::PathBuf::from(path);
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("selected path is not a file".into());
    }
    if meta.len() > MAX_PICKER_READ_BYTES {
        return Err(format!(
            "selected file is too large for picker read: {} bytes",
            meta.len()
        ));
    }

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "selected-file".to_string());
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    Ok(LocalFilePayload { file_name, bytes })
}

fn app_log_dir() -> std::path::PathBuf {
    desktop::log_dir()
}

fn append_startup_line(log_dir: &std::path::Path, line: &str) {
    let _ = std::fs::create_dir_all(log_dir);
    let path = log_dir.join("attune-desktop-startup.log");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = writeln!(file, "{:?} {line}", std::time::SystemTime::now());
    }
}

fn init_observability() -> std::path::PathBuf {
    let log_dir = app_log_dir();
    let _ = std::fs::create_dir_all(&log_dir);
    append_startup_line(&log_dir, "process entry");

    let panic_log_dir = log_dir.clone();
    std::panic::set_hook(Box::new(move |info| {
        append_startup_line(&panic_log_dir, &format!("panic: {info}"));
        eprintln!("attune-desktop panic: {info}");
    }));

    let file_appender = tracing_appender::rolling::daily(&log_dir, "attune-desktop");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    Box::leak(Box::new(guard));

    let env_filter = tracing_subscriber::EnvFilter::from_default_env()
        .add_directive("info".parse().expect("'info' is a valid log directive"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr.and(non_blocking))
        .try_init();

    tracing::info!(
        "attune-desktop observability initialized; log_dir={}",
        log_dir.display()
    );
    log_dir
}

fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn desktop_headless_mode() -> bool {
    env_flag_enabled("ATTUNE_DESKTOP_HEADLESS") || env_flag_enabled("ATTUNE_DESKTOP_SERVER_ONLY")
}

fn run_headless_server(log_dir: &std::path::Path) -> Result<(), String> {
    let config = attune_server::ServerConfig {
        host: "127.0.0.1".to_string(),
        port: embedded_server::server_port(),
        tls_cert: None,
        tls_key: None,
        no_auth: false,
    };
    tracing::info!(
        "attune-desktop headless mode: embedded server only at {}",
        embedded_server::server_url()
    );
    append_startup_line(
        log_dir,
        &format!(
            "headless server mode enabled at {}",
            embedded_server::server_url()
        ),
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime
        .block_on(attune_server::run_in_runtime(config))
        .map_err(|e| e.to_string())
}

fn main() {
    // webkit2gtk 2.42+ 默认启用 DMABUF/GBM EGL 渲染器,在 NVIDIA 私有驱动(及部分虚拟
    // 显示)上初始化 GBM EGL 失败 → "Could not create GBM EGL display:
    // EGL_NOT_INITIALIZED. Aborting..." → 窗口启动即崩(deb 在 N 卡机上点图标即崩的根因,
    // 2026-06-13 实测)。出厂禁用该渲染器走兼容路径,让 attune 在 NVIDIA 机上开箱可用;
    // 用户可显式 export WEBKIT_DISABLE_DMABUF_RENDERER 覆盖。必须在 GTK/webview 初始化前设。
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    let log_dir = init_observability();
    let start_in_background = desktop::is_background_launch();

    if desktop_headless_mode() {
        if let Err(e) = run_headless_server(&log_dir) {
            tracing::error!("attune-desktop headless server exited with error: {e}");
            append_startup_line(&log_dir, &format!("headless server exited with error: {e}"));
            std::process::exit(1);
        }
        return;
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // 重复双击：激活已有主窗口（unminimize + show + focus），第二个进程立即退出
            tracing::info!("single-instance: another launch detected, focusing existing window");
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            upload_dropped_paths,
            read_local_file,
            check_for_update_now,
            restart_for_update,
            desktop_app_info,
            open_desktop_path,
            create_diagnostic_bundle,
            reveal_diagnostic_bundle,
            get_launch_at_login,
            set_launch_at_login,
            get_close_behavior,
            set_close_behavior
        ])
        .setup(move |app| {
            tracing::info!(
                "attune-desktop setup: server_url={} log_dir={}",
                embedded_server::server_url(),
                log_dir.display()
            );
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
                            WebviewUrl::External(
                                url.parse().expect("embedded server URL is well-formed"),
                            ),
                        )
                        .title("Attune")
                        .inner_size(1280.0, 800.0)
                        .min_inner_size(800.0, 600.0)
                        .visible(!start_in_background)
                        .build()
                        {
                            tracing::error!("failed to build main window: {e}");
                        }

                        // 主窗口事件处理：
                        //   1. 关闭按钮 = 隐藏到托盘，不退出进程
                        //   2. OS 级文件拖拽 → emit 'attune-file-drop' 给前端
                        if let Some(window) = app_handle.get_webview_window("main") {
                            let win_clone = window.clone();
                            let app_for_drop = app_handle.clone();
                            window.on_window_event(move |event| match event {
                                tauri::WindowEvent::CloseRequested { api, .. } => {
                                    if crate::desktop::close_action() == "tray" {
                                        api.prevent_close();
                                        let _ = win_clone.hide();
                                        let _ = app_for_drop.emit(
                                            "attune-window-hidden",
                                            serde_json::json!({"reason": "close-request"}),
                                        );
                                    }
                                }
                                tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop {
                                    paths,
                                    ..
                                }) => {
                                    let payload: Vec<String> = paths
                                        .iter()
                                        .map(|p| p.to_string_lossy().into_owned())
                                        .collect();
                                    if let Err(e) = app_for_drop.emit("attune-file-drop", &payload)
                                    {
                                        tracing::warn!("failed to emit attune-file-drop: {e}");
                                    }
                                }
                                _ => {}
                            });
                        }

                        // 系统托盘
                        if let Err(e) = crate::tray::build(&app_handle) {
                            tracing::error!("failed to build system tray: {e}");
                        }

                        // 启动 30s 后被动检查更新:仅 emit "available" 事件让 UI 显示
                        // banner,**不**自动下载(尊重用户带宽 + 让用户选时机).
                        // 主动下载/安装走 check_for_update_now command (用户点按钮触发).
                        // 网络不可达 → 静默 log warn,不弹窗不 panic.
                        let app_handle_for_update = app_handle.clone();
                        tauri::async_runtime::spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                            use tauri_plugin_updater::UpdaterExt;
                            let endpoints = update_feed::resolve_endpoints_from_env();
                            let updater = app_handle_for_update
                                .updater_builder()
                                .endpoints(
                                    endpoints.iter().filter_map(|e| e.parse().ok()).collect(),
                                )
                                .and_then(|b| b.build());
                            match updater {
                                Ok(updater) => match updater.check().await {
                                    Ok(Some(update)) => {
                                        tracing::info!(
                                            "update available: {} -> {}",
                                            update.current_version,
                                            update.version
                                        );
                                        let _ = app_handle_for_update.emit(
                                            EV_UPDATE_STATUS,
                                            serde_json::json!({
                                                "state": "available",
                                                "from": update.current_version,
                                                "to": update.version,
                                            }),
                                        );
                                    }
                                    Ok(None) => tracing::info!("no update available"),
                                    Err(e) => tracing::warn!(
                                        "update check failed (endpoint unreachable): {e}"
                                    ),
                                },
                                Err(e) => tracing::warn!("updater handle unavailable: {e}"),
                            }
                        });
                    }
                    Err(e) => {
                        tracing::error!("embedded server failed to start: {e}");
                        append_startup_line(
                            &log_dir,
                            &format!("embedded server failed to start: {e}"),
                        );
                        std::process::exit(1);
                    }
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running attune-desktop");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn with_env<T>(name: &str, value: Option<&str>, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let old = std::env::var(name).ok();
        match value {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
        let out = f();
        match old {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
        out
    }

    fn with_two_envs<T>(
        first: (&str, Option<&str>),
        second: (&str, Option<&str>),
        f: impl FnOnce() -> T,
    ) -> T {
        let _guard = ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let first_old = std::env::var(first.0).ok();
        let second_old = std::env::var(second.0).ok();
        match first.1 {
            Some(value) => std::env::set_var(first.0, value),
            None => std::env::remove_var(first.0),
        }
        match second.1 {
            Some(value) => std::env::set_var(second.0, value),
            None => std::env::remove_var(second.0),
        }
        let out = f();
        match first_old {
            Some(value) => std::env::set_var(first.0, value),
            None => std::env::remove_var(first.0),
        }
        match second_old {
            Some(value) => std::env::set_var(second.0, value),
            None => std::env::remove_var(second.0),
        }
        out
    }

    #[test]
    fn env_flag_accepts_common_truthy_values() {
        for value in ["1", "true", "TRUE", "yes", "on", " On "] {
            with_env("ATTUNE_TEST_FLAG", Some(value), || {
                assert!(env_flag_enabled("ATTUNE_TEST_FLAG"));
            });
        }
    }

    #[test]
    fn env_flag_rejects_missing_and_falsey_values() {
        with_env("ATTUNE_TEST_FLAG", None, || {
            assert!(!env_flag_enabled("ATTUNE_TEST_FLAG"));
        });
        for value in ["0", "false", "no", "off", ""] {
            with_env("ATTUNE_TEST_FLAG", Some(value), || {
                assert!(!env_flag_enabled("ATTUNE_TEST_FLAG"));
            });
        }
    }

    #[test]
    fn desktop_headless_mode_supports_canonical_and_legacy_env_names() {
        with_two_envs(
            ("ATTUNE_DESKTOP_HEADLESS", Some("1")),
            ("ATTUNE_DESKTOP_SERVER_ONLY", None),
            || {
                assert!(desktop_headless_mode());
            },
        );
        with_two_envs(
            ("ATTUNE_DESKTOP_HEADLESS", None),
            ("ATTUNE_DESKTOP_SERVER_ONLY", Some("true")),
            || {
                assert!(desktop_headless_mode());
            },
        );
    }
}
