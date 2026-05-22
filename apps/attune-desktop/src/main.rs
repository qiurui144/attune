#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod embedded_server;
mod tray;

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

/// Auto-updater 状态机:UI 通过监听 `attune-update-status` 事件获得这些状态.
/// 维持纯字符串(不引入额外 serde 类型),前端 JS 直接 switch.
const EV_UPDATE_STATUS: &str = "attune-update-status";

/// Tauri command:UI 主动触发检查更新.成功命中时 (latest > current) 先 emit
/// `available`,随后下载+安装 (含进度 emit `downloading` / `installing`),完成 emit
/// `restart-required`,失败 emit `error`.无新版返回 false 不 emit.
///
/// 返回 Ok(true) = 有更新且已开始下载; Ok(false) = 无更新; Err = 检查/下载/安装失败.
#[tauri::command]
async fn check_for_update_now(app: AppHandle) -> Result<bool, String> {
    use tauri_plugin_updater::UpdaterExt;
    let updater = app.updater().map_err(|e| e.to_string())?;
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

    let _ = app.emit(
        EV_UPDATE_STATUS,
        serde_json::json!({"state": "restart-required"}),
    );
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

/// Tauri command: upload local file paths to the embedded server's /api/v1/upload endpoint.
/// Called by the web UI after receiving an `attune-file-drop` event.
#[tauri::command]
async fn upload_dropped_paths(paths: Vec<String>) -> Result<Vec<String>, String> {
    let client = reqwest::Client::new();
    let token = std::env::var("ATTUNE_DEV_TOKEN").unwrap_or_default();
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
        let mut req = client
            .post("http://127.0.0.1:18900/api/v1/upload")
            .multipart(form);
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

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("info".parse().expect("'info' is a valid log directive")),
        )
        .init();

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
            check_for_update_now,
            restart_for_update
        ])
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
                            WebviewUrl::External(url.parse().expect("embedded server URL is well-formed")),
                        )
                        .title("Attune")
                        .inner_size(1280.0, 800.0)
                        .min_inner_size(800.0, 600.0)
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
                                    api.prevent_close();
                                    let _ = win_clone.hide();
                                }
                                tauri::WindowEvent::DragDrop(
                                    tauri::DragDropEvent::Drop { paths, .. },
                                ) => {
                                    let payload: Vec<String> = paths
                                        .iter()
                                        .map(|p| p.to_string_lossy().into_owned())
                                        .collect();
                                    if let Err(e) =
                                        app_for_drop.emit("attune-file-drop", &payload)
                                    {
                                        tracing::warn!(
                                            "failed to emit attune-file-drop: {e}"
                                        );
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
                            match app_handle_for_update.updater() {
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
                        std::process::exit(1);
                    }
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running attune-desktop");
}
