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
