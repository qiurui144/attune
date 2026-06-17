//! Re-verify 编排(纯逻辑层,T8)。worker(attune-server)与 refresh 路由共用本层把
//! cloud `/member/verify` 响应**经 SEC-1/2 验签 + nonce + freshness 后**才转 Active,
//! 失败按 §7.2 宽限/业务拒分流。
//!
//! ## 为什么放 attune-core
//!
//! "转 Active 前必经 [`crate::entitlement_anchor::authorize_snapshot_fresh`]" 是吊销逃逸
//! 闭合的**集成层**不变量(SEC-1/2 在 worker 落地)。把它做成纯函数
//! [`apply_reverify`] 让 `worker_forged_response_does_not_activate` 可在 attune-core 单测
//! (不依赖 server 起服 / 网络),与 §6.1 测试矩阵 adversarial 行对齐。
//!
//! ## 退避(spec §3.3 / §7.2)
//!
//! 连续失败退避 1h → 4h → 24h(封顶),恢复成功重置 —— [`backoff_after`]。
//!
//! ## 网络错 vs 业务拒(spec §7.2 error 5,严格区分)
//!
//! - 传输层失败 / 5xx(`verify_entitlements` 返 `Err`)→ [`ReverifyOutcome::NetworkError`]
//!   → 进**宽限**(grace),**不破**已有缓存。
//! - 200 + 验签通过 + `status=revoked/suspended` → [`ReverifyOutcome::BusinessDeny`]
//!   → 立即 Suspended(fail-closed)。
//! - 200 + 验签通过 + `status=active` → [`ReverifyOutcome::Active`] → 转 Active 续期。
//! - 200 + 验签失败/伪造/重放(strict)→ [`ReverifyOutcome::Unauthorized`] → 走宽限
//!   (**绝不**转 Active —— SEC 闭合)。

use chrono::{DateTime, Duration, Utc};

use crate::cloud_client::{CloudClient, EntitlementSnapshot};
use crate::entitlement::EntitlementCache;
use crate::entitlement_anchor::{authorize_snapshot_fresh, ENTITLEMENT_SIGNING_PUBKEYS, SnapshotAuthorization};
use crate::plugin_sig::TrustMode;
use crate::store::plugin_entitlements::EntitlementRow;

/// 退避序列:0 次失败 → 不退避;1 次 → 1h;2 次 → 4h;≥3 次 → 24h(封顶)。
pub fn backoff_after(consecutive_failures: u32) -> Duration {
    match consecutive_failures {
        0 => Duration::zero(),
        1 => Duration::hours(1),
        2 => Duration::hours(4),
        _ => Duration::hours(24),
    }
}

/// 生成一次性 128-bit verify challenge nonce(hex)。每次 verify 新生成,**不持久化**
/// (SEC-2 anti-replay:重放旧响应必然 nonce 不匹配)。用 `OsRng`(crypto-secure 系统
/// 熵源),**不用** `thread_rng`(per spec §T-auth-2 + plugin_sig.rs:300 惯例)。
pub fn new_client_nonce() -> String {
    use aes_gcm::aead::{rand_core::RngCore, OsRng};
    let mut buf = [0u8; 16];
    OsRng.fill_bytes(&mut buf);
    hex::encode(buf)
}

/// 一轮 re-verify 的端到端结果:cloud 调用结果 + 经 SEC-1/2 门后的 [`ReverifyOutcome`]。
pub struct VerifyRound {
    /// 本轮生成的 client nonce(SEC-2 challenge)。
    pub client_nonce: String,
    /// SEC-1/2 门后的判定(worker/route 据此 [`apply_reverify`])。
    pub outcome: ReverifyOutcome,
    /// 本轮接受的 `verified_at`(仅 Active/BusinessDeny 时有意义;用于写回缓存基准)。
    pub verified_at: Option<String>,
}

/// 跑一轮真 re-verify:生成 nonce → 调 cloud `/member/verify` → 经 SEC-1/2 门
/// ([`authorize_snapshot_fresh`])→ 映射 [`ReverifyOutcome`]。**纯编排,无锁/无 DB**;
/// 调用方拿到 [`VerifyRound`] 后自行 [`apply_reverify`] + 短取 vault 锁写回。
///
/// 网络错 / 5xx(`verify_entitlements` 返 `Err`)→ [`ReverifyOutcome::NetworkError`]
/// (走宽限,不破缓存)。**转 Active 唯一合法路径** = `authorize_snapshot_fresh`
/// 返 `Authorized("active")`(SEC 闭合)。
pub fn verify_round(
    client: &CloudClient,
    license_id: &str,
    mode: TrustMode,
    keys: &[&str],
    last_accepted: Option<&DateTime<Utc>>,
    now: &DateTime<Utc>,
) -> VerifyRound {
    let client_nonce = new_client_nonce();
    let cloud_resp: Result<EntitlementSnapshot, ()> =
        client.verify_entitlements(license_id, &client_nonce).map_err(|_| ());
    let verified_at = cloud_resp
        .as_ref()
        .ok()
        .and_then(|s| s.signed_payload.as_ref())
        .map(|p| p.verified_at.clone());
    let outcome = classify_reverify(&cloud_resp, mode, keys, &client_nonce, last_accepted, now);
    VerifyRound { client_nonce, outcome, verified_at }
}

/// re-verify 一轮的判定结果(供 worker / refresh 路由据此更新缓存)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReverifyOutcome {
    /// 验签 + nonce + freshness 全过、status=active → 转 Active 续期。
    Active,
    /// 验签通过且 status=revoked/suspended → 立即 fail-closed(Suspended)。
    BusinessDeny(String),
    /// 传输层失败 / 5xx → 网络错,进宽限,不破缓存。
    NetworkError,
    /// 验签失败 / 伪造 / 重放 / nonce 不符(strict)→ 不转 Active,走宽限。
    Unauthorized(&'static str),
}

/// 把一次 re-verify 的 cloud 响应(`Ok(snapshot)` = 收到 200,`Err` = 网络/5xx)经
/// SEC-1/2 门后映射为 [`ReverifyOutcome`](纯函数,无锁/无 DB/无网络)。
///
/// **转 Active 唯一合法路径**:`authorize_snapshot_fresh` 返 `Authorized("active")`。
/// 任何伪造/重放/未签名(strict)→ `Unauthorized` → **绝不** Active(SEC 闭合)。
#[allow(clippy::too_many_arguments)]
pub fn classify_reverify(
    cloud_resp: &Result<EntitlementSnapshot, ()>,
    mode: TrustMode,
    keys: &[&str],
    client_nonce: &str,
    last_accepted: Option<&DateTime<Utc>>,
    now: &DateTime<Utc>,
) -> ReverifyOutcome {
    let snap = match cloud_resp {
        Err(()) => return ReverifyOutcome::NetworkError,
        Ok(s) => s,
    };
    match authorize_snapshot_fresh(snap, mode, keys, client_nonce, last_accepted, now) {
        SnapshotAuthorization::Authorized(status)
        | SnapshotAuthorization::AuthorizedWithWarning(status) => {
            if status == "revoked" || status == "suspended" {
                ReverifyOutcome::BusinessDeny(status)
            } else if status == "active" {
                ReverifyOutcome::Active
            } else {
                // 其他态(trial 等)按 active 续期处理(可用)。
                ReverifyOutcome::Active
            }
        }
        SnapshotAuthorization::Unauthorized(code) => ReverifyOutcome::Unauthorized(code),
    }
}

/// 把 [`ReverifyOutcome`] 应用到缓存(转 Active / 立即关 / 进宽限 / 不动)。返回是否
/// **接受**(true = 转 Active 续期 / 业务拒生效)。
///
/// 锁序铁律:本函数只取 [`EntitlementCache`] 自身的锁(独立锁),**不取**
/// fulltext/vectors/vault。worker 在调用本函数**后**(cache 锁释放后)再短取 vault 锁
/// 写回 vault DB —— 两锁不重叠持有。
pub fn apply_reverify(
    cache: &EntitlementCache,
    plugin_id: &str,
    outcome: &ReverifyOutcome,
    verified_at: &str,
) -> bool {
    match outcome {
        ReverifyOutcome::Active => {
            // 转 Active 续期 —— 推进 last_verified_at(SEC-2 单调基准)。
            cache.set_status(plugin_id, "active", verified_at);
            true
        }
        ReverifyOutcome::BusinessDeny(status) => {
            // 立即 fail-closed —— 显式降级路径(唯一能把 active 降到 suspended/revoked)。
            cache.set_status(plugin_id, status, verified_at);
            true
        }
        // 网络错 / 未授权(伪造/重放/未签名 strict)→ **不破缓存**(走宽限),不转 Active。
        ReverifyOutcome::NetworkError | ReverifyOutcome::Unauthorized(_) => false,
    }
}

/// 把一条 [`EntitlementRow`] 转为 dispatch-ready 视图状态(便于 worker 写回 vault)。
/// 仅辅助:row 的 `last_verified_at`/`status` 推进后,调用方 `upsert_entitlement` 落盘。
pub fn touched_row(row: &EntitlementRow, status: &str, verified_at: &str) -> EntitlementRow {
    EntitlementRow {
        status: status.to_string(),
        last_verified_at: verified_at.to_string(),
        updated_at: verified_at.to_string(),
        ..row.clone()
    }
}

/// 对缓存里每条 entitlement 跑一轮真 [`verify_round`](生产 anchor
/// [`ENTITLEMENT_SIGNING_PUBKEYS`]),返回 `(plugin_id, outcome, verified_at)` 列表
/// 供 [`apply_refresh_rounds`] 应用。**只读缓存 + 调网络,不写缓存/不取 vault 锁**
/// (写回由调用方在锁释放后做)。`last_accepted` 取自缓存当前 `last_verified_at`
/// (SEC-2 单调基准)。
pub fn reverify_all(
    cache: &EntitlementCache,
    client: &CloudClient,
    mode: TrustMode,
    now: &DateTime<Utc>,
) -> Vec<(String, ReverifyOutcome, Option<String>)> {
    let keys: Vec<&str> = ENTITLEMENT_SIGNING_PUBKEYS.to_vec();
    cache
        .snapshot()
        .iter()
        .map(|row| {
            let last = cache.last_verified_at(&row.plugin_id);
            let round = verify_round(client, &row.license_id, mode, &keys, last.as_ref(), now);
            (row.plugin_id.clone(), round.outcome, round.verified_at)
        })
        .collect()
}

/// 一轮多-plugin refresh 的聚合结果(供 refresh 路由映射 200/502)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefreshSummary {
    /// 实际转 Active/降级被接受的 plugin_id 计数(`refreshed` 字段)。
    pub refreshed: usize,
    /// 每个 plugin 的最终运行期状态字符串(`statuses` 字段,plugin_id → api status)。
    pub statuses: Vec<(String, String)>,
    /// 本轮是否**所有** verify 都是网络错(cloud 完全不可达)→ 路由回 502。
    pub all_network_error: bool,
}

/// 把"每个 plugin 一轮 [`ReverifyOutcome`]"应用到缓存并聚合为 [`RefreshSummary`]
/// (纯函数,只取 [`EntitlementCache`] 独立锁,**不取** vault/fulltext/vectors)。
///
/// - `rounds`:`(plugin_id, outcome, verified_at)` 列表(verified_at 仅 Active/Deny 用)。
/// - 全部 NetworkError 且非空 → `all_network_error = true`(路由回 502,缓存原样)。
/// - 任一非网络错(转 Active / 业务拒 / 未授权)→ `all_network_error = false`(路由回 200)。
///
/// `statuses` 反映 apply 后缓存的运行期态(经 [`EntitlementCache::status`])。
pub fn apply_refresh_rounds(
    cache: &EntitlementCache,
    rounds: &[(String, ReverifyOutcome, Option<String>)],
    now: &DateTime<Utc>,
) -> RefreshSummary {
    let mut refreshed = 0usize;
    let mut net_err = 0usize;
    for (plugin_id, outcome, verified_at) in rounds {
        let va = verified_at.as_deref().unwrap_or("");
        if apply_reverify(cache, plugin_id, outcome, va) {
            refreshed += 1;
        }
        if matches!(outcome, ReverifyOutcome::NetworkError) {
            net_err += 1;
        }
    }
    let statuses = cache
        .snapshot()
        .iter()
        .map(|row| (row.plugin_id.clone(), cache.status(&row.plugin_id, now).as_api_str().to_string()))
        .collect();
    RefreshSummary {
        refreshed,
        statuses,
        all_network_error: !rounds.is_empty() && net_err == rounds.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_client::{EntitlementSnapshot, SignedPayload};
    use crate::entitlement::EntStatus;
    use ed25519_dalek::{Signer, SigningKey};

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn signing_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn pubkey_hex(sk: &SigningKey) -> String {
        hex::encode(sk.verifying_key().to_bytes())
    }

    fn signed_snap(signer: &SigningKey, status: &str, nonce: &str, verified_at: &str) -> EntitlementSnapshot {
        let payload = SignedPayload {
            status: status.into(),
            allowed_plugins: vec!["law-pro".into()],
            expires_at: None,
            nonce: nonce.into(),
            verified_at: verified_at.into(),
        };
        let sig = signer.sign(&payload.canonical_bytes());
        let sig_b64 = base64::engine::general_purpose::STANDARD.encode(sig.to_bytes());
        let json = serde_json::json!({
            "valid": true, "plan": "pro", "entitlement_schema": 1,
            "nonce": nonce,
            "signed_payload": serde_json::to_value(&payload).unwrap(),
            "signature": sig_b64, "entitlements": [], "next_verify_after_hours": 24
        });
        serde_json::from_value(json).unwrap()
    }

    use base64::Engine;

    // ── backoff sequence 1h → 4h → 24h ──────────────────────────────────────

    #[test]
    fn backoff_sequence_1h_4h_24h() {
        assert_eq!(backoff_after(0), Duration::zero());
        assert_eq!(backoff_after(1), Duration::hours(1));
        assert_eq!(backoff_after(2), Duration::hours(4));
        assert_eq!(backoff_after(3), Duration::hours(24));
        assert_eq!(backoff_after(10), Duration::hours(24), "capped at 24h");
    }

    // ── network err vs business deny (strict separation, §7.2 error 5) ──────

    #[test]
    fn network_err_yields_network_error_not_expired() {
        let resp: Result<EntitlementSnapshot, ()> = Err(());
        let now = ts("2026-06-12T00:00:00+00:00");
        let out = classify_reverify(&resp, TrustMode::Strict, &[], "n1", None, &now);
        assert_eq!(out, ReverifyOutcome::NetworkError, "5xx/transport → grace, not expired");
    }

    #[test]
    fn business_deny_revoked_is_immediate() {
        let signer = signing_key(11);
        let keys = [pubkey_hex(&signer)];
        let kr: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
        let snap = signed_snap(&signer, "revoked", "n1", "2026-06-12T00:00:00+00:00");
        let now = ts("2026-06-12T01:00:00+00:00");
        let out = classify_reverify(&Ok(snap), TrustMode::Strict, &kr, "n1", None, &now);
        assert_eq!(out, ReverifyOutcome::BusinessDeny("revoked".into()));
    }

    #[test]
    fn active_signed_fresh_transitions_active() {
        let signer = signing_key(11);
        let keys = [pubkey_hex(&signer)];
        let kr: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
        let snap = signed_snap(&signer, "active", "n1", "2026-06-12T00:00:00+00:00");
        let now = ts("2026-06-12T00:30:00+00:00");
        let out = classify_reverify(&Ok(snap), TrustMode::Strict, &kr, "n1", None, &now);
        assert_eq!(out, ReverifyOutcome::Active);
    }

    // ── SEC: worker forged response does NOT activate (closure invariant) ────

    #[test]
    fn worker_forged_response_does_not_activate() {
        // Attacker (non-anchor key) signs a forged "active" 200. In strict, the
        // worker's classify must NOT yield Active — it yields Unauthorized.
        let attacker = signing_key(99);
        let official = signing_key(11);
        let keys = [pubkey_hex(&official)]; // attacker NOT in allowlist
        let kr: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
        let forged = signed_snap(&attacker, "active", "n1", "2026-06-12T00:00:00+00:00");
        let now = ts("2026-06-12T00:30:00+00:00");
        let out = classify_reverify(&Ok(forged), TrustMode::Strict, &kr, "n1", None, &now);
        assert!(
            matches!(out, ReverifyOutcome::Unauthorized(_)),
            "forged active must NOT transition to Active (SEC-1)"
        );
        assert_ne!(out, ReverifyOutcome::Active);
    }

    #[test]
    fn worker_replayed_response_does_not_activate() {
        // Replayed old (genuinely-signed) active with stale nonce → Unauthorized.
        let official = signing_key(11);
        let keys = [pubkey_hex(&official)];
        let kr: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
        let old = signed_snap(&official, "active", "old-nonce", "2026-06-10T00:00:00+00:00");
        let last = ts("2026-06-11T00:00:00+00:00");
        let now = ts("2026-06-12T00:00:00+00:00");
        // This round's client nonce differs from the replayed response's nonce.
        let out = classify_reverify(&Ok(old), TrustMode::Strict, &kr, "fresh-nonce", Some(&last), &now);
        assert!(matches!(out, ReverifyOutcome::Unauthorized(_)), "replay must NOT activate (SEC-2)");
    }

    // ── apply_reverify cache effects ────────────────────────────────────────

    #[test]
    fn apply_active_sets_cache_active_and_advances_verified_at() {
        let cache = EntitlementCache::new();
        let row = EntitlementRow {
            plugin_id: "law-pro".into(),
            license_id: "l".into(),
            tier: "paid".into(),
            status: "suspended".into(),
            trial_expires: None,
            signing_pubkey_hex: "00".repeat(32),
            last_verified_at: "2026-06-10T00:00:00+00:00".into(),
            grace_started_at: None,
            updated_at: "2026-06-10T00:00:00+00:00".into(),
        };
        cache.upsert(row);
        let accepted = apply_reverify(&cache, "law-pro", &ReverifyOutcome::Active, "2026-06-12T00:00:00+00:00");
        assert!(accepted);
        let now = ts("2026-06-12T00:00:01+00:00");
        assert_eq!(cache.status("law-pro", &now), EntStatus::Active);
    }

    #[test]
    fn apply_network_error_preserves_cache() {
        // §7.2 error 5: a NetworkError must NOT mutate the cached status.
        let cache = EntitlementCache::new();
        let row = EntitlementRow {
            plugin_id: "law-pro".into(),
            license_id: "l".into(),
            tier: "paid".into(),
            status: "active".into(),
            trial_expires: None,
            signing_pubkey_hex: "00".repeat(32),
            last_verified_at: "2026-06-12T00:00:00+00:00".into(),
            grace_started_at: None,
            updated_at: "2026-06-12T00:00:00+00:00".into(),
        };
        cache.upsert(row);
        let before = cache.snapshot();
        let accepted = apply_reverify(&cache, "law-pro", &ReverifyOutcome::NetworkError, "ignored");
        assert!(!accepted);
        assert_eq!(cache.snapshot(), before, "network error must not mutate cache");
    }

    #[test]
    fn apply_business_deny_revokes() {
        let cache = EntitlementCache::new();
        let row = EntitlementRow {
            plugin_id: "law-pro".into(),
            license_id: "l".into(),
            tier: "paid".into(),
            status: "active".into(),
            trial_expires: None,
            signing_pubkey_hex: "00".repeat(32),
            last_verified_at: "2026-06-12T00:00:00+00:00".into(),
            grace_started_at: None,
            updated_at: "2026-06-12T00:00:00+00:00".into(),
        };
        cache.upsert(row);
        apply_reverify(&cache, "law-pro", &ReverifyOutcome::BusinessDeny("revoked".into()), "2026-06-12T01:00:00+00:00");
        let now = ts("2026-06-12T01:00:01+00:00");
        assert_eq!(cache.status("law-pro", &now), EntStatus::Suspended);
    }

    #[test]
    fn apply_unauthorized_preserves_cache() {
        // forged/replayed (Unauthorized) must NOT mutate the cached status either.
        let cache = EntitlementCache::new();
        let row = EntitlementRow {
            plugin_id: "law-pro".into(),
            license_id: "l".into(),
            tier: "paid".into(),
            status: "active".into(),
            trial_expires: None,
            signing_pubkey_hex: "00".repeat(32),
            last_verified_at: "2026-06-12T00:00:00+00:00".into(),
            grace_started_at: None,
            updated_at: "2026-06-12T00:00:00+00:00".into(),
        };
        cache.upsert(row);
        let before = cache.snapshot();
        let accepted = apply_reverify(&cache, "law-pro", &ReverifyOutcome::Unauthorized("entitlement-sig-invalid"), "x");
        assert!(!accepted);
        assert_eq!(cache.snapshot(), before, "unauthorized must not mutate cache (no forged activation)");
    }
}
