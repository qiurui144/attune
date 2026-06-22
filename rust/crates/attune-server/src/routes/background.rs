//! Background-task control + power policy (L_hw power layer, plan 0.4).
//!
//! Exposes the resource_governor registry that previously had `pause_all` /
//! `resume_all` but NO HTTP surface (the "顶栏暂停后台任务" toggle was unwired).
//! Adds power-policy read/write so the desktop Settings toggle can configure
//! battery behavior (throttle / pause / off + low-battery floor).
//!
//! Background tasks (embedding / scanner / webdav / annotator / skill /
//! memory) auto-throttle on battery via the default Throttle policy even
//! without these endpoints; this surface is for user control + status.

use axum::Json;

use attune_core::resource_governor::{global_registry, PowerPolicy};

use crate::error::{AppError, AppResult};

/// `GET /api/v1/background/status` — governor snapshot + live power state + policy.
pub async fn status() -> Json<serde_json::Value> {
    let reg = global_registry();
    let power = attune_core::platform::power::probe();
    Json(serde_json::json!({
        "governors": reg.snapshot(),
        "power": {
            "source": format!("{:?}", power.source),
            "battery_pct": power.battery_pct,
            "profile": format!("{:?}", power.profile),
            "thermal_pressure": power.thermal_pressure,
            "energy_constrained": power.is_energy_constrained(),
        },
        "policy": reg.current_power_policy(),
    }))
}

/// `POST /api/v1/background/pause` — 顶栏一键暂停所有后台任务。
pub async fn pause() -> Json<serde_json::Value> {
    global_registry().pause_all();
    Json(serde_json::json!({ "ok": true, "paused": true }))
}

/// `POST /api/v1/background/resume` — 恢复后台任务。
pub async fn resume() -> Json<serde_json::Value> {
    global_registry().resume_all();
    Json(serde_json::json!({ "ok": true, "paused": false }))
}

/// `POST /api/v1/background/power-policy` — set the battery/power policy.
/// Body: `{"mode": "throttle"|"pause"|"off", "low_battery_pct": 20}`.
pub async fn set_power_policy(
    Json(body): Json<PowerPolicy>,
) -> AppResult<Json<serde_json::Value>> {
    if body.low_battery_pct > 100 {
        return Err(AppError::BadRequest(
            "low_battery_pct must be 0-100".into(),
        ));
    }
    global_registry().set_power_policy(body);
    Ok(Json(serde_json::json!({ "ok": true, "policy": body })))
}
