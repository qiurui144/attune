//! `GET /api/v1/version` — active version notification endpoint.
//!
//! per spec `docs/superpowers/specs/2026-05-26-v1-0-1-upgrade-strategy-and-support.md` §5.2
//! and plan C3.
//!
//! 用途:server 主动告知 user 当前版本 + GitHub latest release 版本,UI 可显示
//! "有新版本"提示。配合 Tauri auto-updater 走 silent path,但本 endpoint 让前端有
//! 显式入口(Settings → About → Check for updates 按钮)。
//!
//! 设计约束:
//! - **零 LLM 依赖**(per § 成本契约 — 此 endpoint 走零成本路径)
//! - **6h ETag cache** 防 GitHub API rate limit(60 req/h unauthenticated)
//! - **offline graceful** — GitHub query 失败时返当前版本 + `latest_available: null`,不 panic
//! - **semver compare** 判 breaking change(major bump)

use crate::state::SharedState;
use attune_core::outbound_gate::{OutboundGate, OutboundKind, OutboundPolicy};
use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// 返回体 schema(JSON shape 稳定 per § API 契约,客户端可针对处理)。
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct VersionInfo {
    /// 当前 server 版本(`CARGO_PKG_VERSION` 内置)。
    pub current: String,
    /// GitHub latest release tag(去掉前缀 `v` / `desktop-v`)。
    /// offline 或 GH API fail 时为 `None`。
    pub latest_available: Option<String>,
    /// `latest_available > current` 时为 `true`,否则 `false`/`None`。
    pub upgrade_available: Option<bool>,
    /// release page URL,user click 直跳。
    pub upgrade_url: Option<String>,
    /// 是否为 breaking change(semver major bump)。
    /// `Some(true)` 提示走 docs/UPGRADING.md。
    pub breaking_changes: Option<bool>,
    /// 是否支持 rollback(v1.0.1+ 内置 `attune rollback` CLI 后恒 `true`)。
    pub rollback_supported: bool,
    /// R1.1b: `Some("disabled-by-privacy-settings")` when the GitHub update
    /// check was refused by the outbound gate (privacy `telemetry` toggle off,
    /// the default). Omitted (`None`) on normal responses — additive field,
    /// existing clients unaffected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_check: Option<String>,
}

/// 简单内存 cache(6h TTL),无需 ETag 持久化 — server restart 时重新 fetch 即可。
struct VersionCache {
    info: VersionInfo,
    fetched_at: Instant,
}

static CACHE: OnceLock<Mutex<Option<VersionCache>>> = OnceLock::new();

const CACHE_TTL: Duration = Duration::from_secs(6 * 3600);
const GH_API_URL: &str = "https://api.github.com/repos/qiurui144/attune/releases/latest";

/// Public endpoint handler — `GET /api/v1/version`.
pub async fn get_version(State(state): State<SharedState>) -> Json<VersionInfo> {
    let current = env!("CARGO_PKG_VERSION").to_string();

    // R1.1b: the GitHub release lookup is a network egress and MUST pass the
    // OutboundGate like every other outbound point. Destination class:
    // `Telemetry` — the request carries zero vault/user data (metadata-only GET
    // to api.github.com) and must work pre-unlock, exactly the telemetry
    // contract (gate skips the vault-locked check for Telemetry). It therefore
    // honors `settings.privacy.telemetry` and fails closed (all 5 egress points
    // default off) until the user opts in. On refusal we degrade gracefully:
    // current version only + `update_check: disabled`, no network touched.
    let telemetry_enabled =
        crate::routes::chat::read_privacy_outbound_enabled(&state, OutboundKind::Telemetry.as_str());
    let policy = OutboundPolicy {
        kind: OutboundKind::Telemetry,
        enabled: telemetry_enabled,
        vault_unlocked: false, // ignored for Telemetry (no vault data on the wire)
        redactor: None,        // empty payload → no redactor needed
        local_destination: false,
        contains_l0: false,
    };
    if OutboundGate::enforce(&policy, "").is_err() {
        return Json(update_check_disabled_info(&current));
    }

    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().await;

    // Cache hit & fresh
    if let Some(c) = guard.as_ref() {
        if c.fetched_at.elapsed() < CACHE_TTL {
            return Json(c.info.clone());
        }
    }

    // Cache miss / expired — fetch
    let info = fetch_with_fallback(&current).await;
    *guard = Some(VersionCache {
        info: info.clone(),
        fetched_at: Instant::now(),
    });

    Json(info)
}

/// Fetch latest release from GitHub, gracefully fall back to current-only on any error.
async fn fetch_with_fallback(current: &str) -> VersionInfo {
    match fetch_latest_from_github().await {
        Ok(latest_tag) => {
            let normalized = normalize_tag(&latest_tag);
            let upgrade = is_upgrade_available(current, &normalized);
            let breaking = upgrade.then(|| is_major_bump(current, &normalized));
            VersionInfo {
                current: current.to_string(),
                latest_available: Some(normalized.clone()),
                upgrade_available: Some(upgrade),
                upgrade_url: Some(format!(
                    "https://github.com/qiurui144/attune/releases/tag/v{}",
                    normalized
                )),
                breaking_changes: breaking,
                rollback_supported: true,
                update_check: None,
            }
        }
        Err(_) => VersionInfo {
            current: current.to_string(),
            latest_available: None,
            upgrade_available: None,
            upgrade_url: None,
            breaking_changes: None,
            rollback_supported: true,
            update_check: None,
        },
    }
}

/// R1.1b graceful refusal shape — same as the offline fallback, plus an explicit
/// `update_check` marker so the UI can tell "check disabled by privacy settings"
/// apart from "GitHub unreachable".
fn update_check_disabled_info(current: &str) -> VersionInfo {
    VersionInfo {
        current: current.to_string(),
        latest_available: None,
        upgrade_available: None,
        upgrade_url: None,
        breaking_changes: None,
        rollback_supported: true,
        update_check: Some("disabled-by-privacy-settings".to_string()),
    }
}

/// GitHub API call. Returns tag like "v1.0.1" or "desktop-v1.0.1".
async fn fetch_latest_from_github() -> Result<String, reqwest::Error> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .user_agent(concat!("attune-server/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let resp = client
        .get(GH_API_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?;

    let body: serde_json::Value = resp.json().await?;
    let tag = body
        .get("tag_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok(tag)
}

/// Strip "v" / "desktop-v" prefix → "1.0.1" / "1.0.1-rc.1".
pub(crate) fn normalize_tag(tag: &str) -> String {
    tag.strip_prefix("desktop-v")
        .or_else(|| tag.strip_prefix('v'))
        .unwrap_or(tag)
        .to_string()
}

/// Naive semver compare: split on '.' and '-', lexically. Good enough for "1.0.0"/"1.0.1" path;
/// pre-release suffix ("-rc.1") makes it "less than" GA per semver, which we honor.
pub(crate) fn is_upgrade_available(current: &str, latest: &str) -> bool {
    compare_semver(latest, current) == std::cmp::Ordering::Greater
}

/// `latest.major > current.major` → semver major bump → breaking.
pub(crate) fn is_major_bump(current: &str, latest: &str) -> bool {
    let cur_major = current
        .split('.')
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    let lat_major = latest
        .split('.')
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    lat_major > cur_major
}

/// Numeric semver compare(忽略 pre-release suffix 的细粒度排序 — 仅用于 ">" 判断)。
pub(crate) fn compare_semver(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |s: &str| {
        let core = s.split('-').next().unwrap_or(s);
        let parts: Vec<u32> = core
            .split('.')
            .map(|p| p.parse::<u32>().unwrap_or(0))
            .collect();
        (
            parts.first().copied().unwrap_or(0),
            parts.get(1).copied().unwrap_or(0),
            parts.get(2).copied().unwrap_or(0),
            // pre-release 存在 → 视为更小;无后缀 → 更大
            !s.contains('-'),
        )
    };
    parse(a).cmp(&parse(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_tag_strips_prefixes() {
        assert_eq!(normalize_tag("v1.0.1"), "1.0.1");
        assert_eq!(normalize_tag("desktop-v1.0.1"), "1.0.1");
        assert_eq!(normalize_tag("1.0.1"), "1.0.1");
        assert_eq!(normalize_tag("v1.0.1-rc.1"), "1.0.1-rc.1");
    }

    #[test]
    fn upgrade_detection_patch_bump() {
        assert!(is_upgrade_available("1.0.0", "1.0.1"));
        assert!(is_upgrade_available("1.0.0", "1.1.0"));
        assert!(is_upgrade_available("1.0.0", "2.0.0"));
        assert!(!is_upgrade_available("1.0.1", "1.0.0"));
        assert!(!is_upgrade_available("1.0.1", "1.0.1"));
    }

    #[test]
    fn pre_release_less_than_ga() {
        // 1.0.1-rc.1 < 1.0.1 GA
        assert!(is_upgrade_available("1.0.1-rc.1", "1.0.1"));
        assert!(!is_upgrade_available("1.0.1", "1.0.1-rc.1"));
    }

    #[test]
    fn major_bump_detected() {
        assert!(is_major_bump("1.0.0", "2.0.0"));
        assert!(is_major_bump("1.5.3", "2.0.0"));
        assert!(!is_major_bump("1.0.0", "1.1.0"));
        assert!(!is_major_bump("1.0.0", "1.0.1"));
    }

    #[test]
    fn version_info_serializes() {
        let info = VersionInfo {
            current: "1.0.0".into(),
            latest_available: Some("1.0.1".into()),
            upgrade_available: Some(true),
            upgrade_url: Some("https://github.com/qiurui144/attune/releases/tag/v1.0.1".into()),
            breaking_changes: Some(false),
            rollback_supported: true,
            update_check: None,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"current\":\"1.0.0\""));
        assert!(json.contains("\"upgrade_available\":true"));
        assert!(json.contains("\"rollback_supported\":true"));
        // additive field omitted when None — existing clients see the old shape
        assert!(!json.contains("update_check"));
    }

    #[test]
    fn update_check_disabled_shape() {
        // R1.1b: gate refusal → current-only + explicit disabled marker, no panic.
        let info = update_check_disabled_info("1.2.0");
        assert_eq!(info.current, "1.2.0");
        assert!(info.latest_available.is_none());
        assert!(info.upgrade_available.is_none());
        assert!(info.rollback_supported);
        assert_eq!(
            info.update_check.as_deref(),
            Some("disabled-by-privacy-settings")
        );
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"update_check\":\"disabled-by-privacy-settings\""));
    }

    #[tokio::test]
    async fn offline_fallback_returns_current_only() {
        // 关键约束:无网络 / API fail 时 endpoint 不 panic
        let info = fetch_with_fallback("1.0.0").await;
        assert_eq!(info.current, "1.0.0");
        assert!(info.rollback_supported);
        // latest 可能 Some(...) 真有网,或 None 真离线;两种都合法
        if info.latest_available.is_none() {
            assert_eq!(info.upgrade_available, None);
            assert_eq!(info.upgrade_url, None);
        }
    }
}
