//! 系统托盘 — 关闭主窗口时不退出进程，最小化到托盘。

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "打开设置", true, None::<&str>)?;
    let lock_vault = MenuItem::with_id(app, "lock-vault", "锁定 Vault", true, None::<&str>)?;
    let check_update = MenuItem::with_id(app, "check-update", "检查更新", true, None::<&str>)?;
    let open_logs = MenuItem::with_id(app, "open-logs", "打开日志目录", true, None::<&str>)?;
    let open_data = MenuItem::with_id(app, "open-data", "打开数据目录", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "完全退出", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &show,
            &settings,
            &lock_vault,
            &check_update,
            &open_logs,
            &open_data,
            &quit,
        ],
    )?;

    let tray = TrayIconBuilder::with_id("main-tray")
        .icon(
            app.default_window_icon()
                .expect("default window icon embedded via tauri.conf.json")
                .clone(),
        )
        .menu(&menu)
        .tooltip("Attune 正在后台运行")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                show_main(app);
            }
            "settings" => {
                show_main(app);
                let _ = app.emit(
                    "attune-navigate",
                    serde_json::json!({"view": "settings", "settingsTab": "general"}),
                );
            }
            "lock-vault" => {
                let _ = app.emit("attune-lock-vault", serde_json::json!({}));
            }
            "check-update" => {
                let app_handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = crate::check_for_update_now(app_handle, None).await;
                });
            }
            "open-logs" => {
                if let Err(e) = crate::desktop::open_desktop_path("logs") {
                    tracing::warn!("open logs from tray failed: {e}");
                }
            }
            "open-data" => {
                if let Err(e) = crate::desktop::open_desktop_path("data") {
                    tracing::warn!("open data from tray failed: {e}");
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
                show_main(app);
            }
        })
        .build(app)?;
    app.manage(tray);
    Ok(())
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}
