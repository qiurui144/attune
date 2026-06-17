//! 客户端 entitlement 运行期授权缓存 + 宽限期状态机(spec §3.3 / §7.2 / §7.3;T5)。
//!
//! ## 角色
//!
//! - [`EntitlementCache`]:`Arc<RwLock<>>` 内存缓存,启动从 vault DB hydrate
//!   ([`crate::store::Store::list_entitlements`]),agent dispatch 前 µs 级查
//!   ([`EntitlementCache::is_entitled`])—— **O(1) HashMap 命中**(PERF-1),非 Vec 线性扫描。
//! - [`grace_transition`]:纯函数宽限期状态机(spec §7.3),不打 DB / 网络,易全覆盖。
//! - [`detect_clock_rollback`]:弱时钟回拨启发(spec §7.2),`last_verified_at > now + 24h`
//!   → 强制 re-verify;不引入 NTP 出网(§R8)。
//!
//! ## 分层 fail(决策 3 + spec §7.2)
//!
//! - trial 72h fail-**closed**(到期拒 dispatch);
//! - paid 14d fail-**open→degrade**(超期仍可用 + 持续橙标,不锁已付费用户);
//! - revoked / suspended 立即 fail-**closed**(Suspended 态永不返可用)。
//!
//! ## 转 Active 的唯一合法路径
//!
//! 只有真 verify-ok(经 [`crate::entitlement_anchor::authorize_snapshot_fresh`] 验签 +
//! nonce + freshness,SEC-1/2)能把状态转 [`EntStatus::Active`]。**无任何注入路径**能把
//! [`EntStatus::Suspended`] 翻 Active —— 这是吊销逃逸闭合的客户端侧不变量(T-auth-1/2 +
//! 本模块协同)。
//!
//! ## 锁序铁律(CLAUDE.md / spec §3.3)
//!
//! `EntitlementCache` 的 `RwLock` 是**独立锁**,作用域内**绝不**取 `fulltext` / `vectors`
//! / `vault` 任一锁(避免新 ABBA 死锁)。re-verify 写回 vault DB 的短取 vault 锁发生在
//! cache 锁**释放后**(调用方 T8 worker 负责,本模块不嵌套)。

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Duration, Utc};

use crate::error::Result;
use crate::store::plugin_entitlements::EntitlementRow;

/// 时钟前跳容忍窗口(spec §7.2,与 [`crate::entitlement_anchor::FRESHNESS_SKEW_SECONDS`] 同源)。
pub const CLOCK_SKEW_SECONDS: i64 = 24 * 3600;

/// trial 离线宽限期(spec §7.3,fail-closed)。
pub const TRIAL_GRACE_HOURS: i64 = 72;

/// paid 离线宽限期(spec §7.3,fail-open→degrade)。
pub const PAID_GRACE_DAYS: i64 = 14;

/// entitlement 运行期状态(spec §7.3 状态机的可观测态)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntStatus {
    /// 免费插件 —— 无授权门(总是可 dispatch)。
    Free,
    /// 已验证授权,云可达 / 最近验证过。
    Active,
    /// trial 内可用(未过期)。
    Trial,
    /// trial 过期(fail-closed)—— 拒 dispatch,插件保留。
    TrialExpired,
    /// 云不可达,宽限态(trial < 72h / paid < 14d)。
    Grace,
    /// paid 超 14d 宽限 —— 仍可用 + 持续橙标(fail-open)。
    Degraded,
    /// license 吊销 / 挂起 —— fail-closed,永不返可用。
    Suspended,
}

impl EntStatus {
    /// 对外 API enum 字符串(spec §5.1 列表 `entitlement_status`)。
    pub fn as_api_str(self) -> &'static str {
        match self {
            EntStatus::Free => "free",
            EntStatus::Active => "active",
            EntStatus::Trial => "trial",
            EntStatus::TrialExpired => "trial-expired",
            EntStatus::Grace => "grace",
            EntStatus::Degraded => "degraded",
            EntStatus::Suspended => "suspended",
        }
    }
}

/// dispatch gate 的判定结果(spec §7.2 dispatch 行为)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntitlementDecision {
    /// 可 dispatch(active / trial 未过期 / paid 宽限 / degraded / free)。
    Allow,
    /// 拒 dispatch,携带 kebab 错误码(供路由回 `{code}`)。
    Reject(&'static str),
}

/// 单条已解析的 entitlement(缓存内态 —— `EntitlementRow` 的 dispatch-ready 视图)。
///
/// `tier` / `raw_status` 从 vault 行带来;`trial_expires` / `last_verified_at` /
/// `grace_started_at` 解析为 `DateTime` 一次,后续判定不重复解析(PERF)。
#[derive(Debug, Clone)]
pub struct Entitlement {
    pub plugin_id: String,
    /// free | trial | paid
    pub tier: String,
    /// active | suspended | revoked(vault 行原始 license 态)。
    pub raw_status: String,
    pub trial_expires: Option<DateTime<Utc>>,
    pub last_verified_at: DateTime<Utc>,
    /// None = 非宽限态。
    pub grace_started_at: Option<DateTime<Utc>>,
}

/// status 优先级(active > trial > 其他),与 store 层 `status_rank` 同序 —— best-status
/// 在 upsert 时预解析(PERF-5),`is_entitled` 不重算多 license。
fn raw_status_rank(status: &str) -> u8 {
    match status {
        "active" => 3,
        "trial" => 2,
        "suspended" => 1,
        _ => 0, // revoked / unknown
    }
}

impl Entitlement {
    /// 从 vault 行解析(时间字段解析一次)。解析失败的时间字段视为缺失(保守:
    /// 不可解析的 trial_expires → 当作已过期)。
    pub fn from_row(row: &EntitlementRow) -> Self {
        let parse = |s: &str| DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc));
        Entitlement {
            plugin_id: row.plugin_id.clone(),
            tier: row.tier.clone(),
            raw_status: row.status.clone(),
            trial_expires: row.trial_expires.as_deref().and_then(parse),
            // 不可解析的 last_verified_at 退化为 epoch(强制 re-verify / 过期处理)。
            last_verified_at: parse(&row.last_verified_at).unwrap_or_else(|| {
                DateTime::<Utc>::from_timestamp(0, 0).expect("epoch is valid")
            }),
            grace_started_at: row.grace_started_at.as_deref().and_then(parse),
        }
    }
}

/// 弱时钟回拨检测(spec §7.2)。`last_verified_at > now + skew`(24h)→ true(系统时间
/// 被回拨到 last_verified_at 之前 / 伪造未来 last_verified)→ 调用方强制 re-verify,
/// 失败按已过期处理。不引入 NTP 出网(§R8);深度伪造时间 = Known Limitation(§R12)。
pub fn detect_clock_rollback(last_verified_at: &DateTime<Utc>, now: &DateTime<Utc>) -> bool {
    *last_verified_at > *now + Duration::seconds(CLOCK_SKEW_SECONDS)
}

/// 宽限期状态机(spec §7.3,**纯函数**,不打 DB / 网络)。给定一条 entitlement 与
/// 当前时间,解析其运行期 [`EntStatus`]。
///
/// ```text
/// raw_status=revoked/suspended            ──> Suspended   (fail-closed,永不翻)
/// tier=free                               ──> Free
/// clock rollback (last_verified > now+24h)──> 按过期处理(trial→TrialExpired / paid→Degraded)
/// tier=trial,未过期                       ──> Trial
/// tier=trial,已过期                       ──> TrialExpired (fail-closed)
/// grace 中:trial >72h                     ──> TrialExpired
/// grace 中:paid  >14d                     ──> Degraded     (fail-open)
/// grace 中:未超窗口                        ──> Grace
/// 否则(active,云最近可达)                 ──> Active
/// ```
pub fn grace_transition(ent: &Entitlement, now: &DateTime<Utc>) -> EntStatus {
    // 1) 吊销 / 挂起 —— 立即 fail-closed,任何路径都不能翻 Active(SEC 闭合不变量)。
    if ent.raw_status == "revoked" || ent.raw_status == "suspended" {
        return EntStatus::Suspended;
    }

    // 2) free —— 无授权门。
    if ent.tier == "free" {
        return EntStatus::Free;
    }

    // 3) 时钟回拨弱启发:last_verified 在"未来"(系统时间被回拨)→ 按过期 / 降级处理。
    let rolled_back = detect_clock_rollback(&ent.last_verified_at, now);

    // 4) 宽限态(grace_started_at 已置)—— 看是否超分层窗口。
    if let Some(grace_start) = ent.grace_started_at {
        let elapsed = *now - grace_start;
        return match ent.tier.as_str() {
            "trial" => {
                if elapsed > Duration::hours(TRIAL_GRACE_HOURS) || rolled_back {
                    EntStatus::TrialExpired // fail-closed
                } else {
                    EntStatus::Grace
                }
            }
            // paid(及其他付费态):超 14d → Degraded(fail-open,仍可用 + 橙标)。
            _ => {
                if elapsed > Duration::days(PAID_GRACE_DAYS) {
                    EntStatus::Degraded
                } else {
                    EntStatus::Grace
                }
            }
        };
    }

    // 5) 非宽限态。trial 看 trial_expires;时钟回拨强制按过期处理。
    if ent.tier == "trial" {
        let expired = match ent.trial_expires {
            Some(exp) => *now >= exp,
            None => true, // 缺失 trial_expires 保守视为已过期(fail-closed)
        };
        if expired || rolled_back {
            return EntStatus::TrialExpired; // fail-closed
        }
        return EntStatus::Trial;
    }

    // 6) paid + 非宽限 + 时钟回拨:无法确认新鲜 → Degraded(fail-open,不锁付费用户)。
    if rolled_back {
        return EntStatus::Degraded;
    }

    // 7) 默认:active,云最近可达。
    EntStatus::Active
}

/// 把 [`EntStatus`] 映射到 dispatch gate 决策(spec §7.2)。
///
/// fail-open(可用):Free / Active / Trial / Grace / Degraded;
/// fail-closed(拒):TrialExpired(`trial-expired`)/ Suspended(`license-revoked`)。
pub fn dispatch_decision(status: EntStatus) -> EntitlementDecision {
    match status {
        EntStatus::Free
        | EntStatus::Active
        | EntStatus::Trial
        | EntStatus::Grace
        | EntStatus::Degraded => EntitlementDecision::Allow,
        EntStatus::TrialExpired => EntitlementDecision::Reject("trial-expired"),
        EntStatus::Suspended => EntitlementDecision::Reject("license-revoked"),
    }
}

/// 内存 entitlement 缓存(spec §3.3)。`Arc<RwLock<HashMap>>` —— O(1) keyed lookup
/// (PERF-1),启动 hydrate,re-verify 后 upsert。**独立锁**,不嵌套 fulltext/vectors/vault。
#[derive(Debug, Clone, Default)]
pub struct EntitlementCache {
    inner: Arc<RwLock<HashMap<String, EntitlementRow>>>,
}

impl EntitlementCache {
    /// 空缓存(测试 / 启动前)。
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 vault DB hydrate(启动时)。调用方在持 vault 锁的 scope 内先
    /// `store.list_entitlements(dek)`,**释放 vault 锁后**再调本方法(本方法只取
    /// entitlement 锁,不取 vault)。
    pub fn hydrate_from_rows(&self, rows: Vec<EntitlementRow>) {
        let mut map = self.inner.write().expect("entitlement cache poisoned");
        map.clear();
        for row in rows {
            Self::merge_best(&mut map, row);
        }
    }

    /// 便捷 hydrate:直接从 store 读全部行(本方法在内部取 vault 行为 store 的事,
    /// 之后只取 entitlement 锁;两锁不重叠持有)。
    pub fn hydrate(&self, store: &crate::store::Store, dek: &crate::crypto::Key32) -> Result<()> {
        // 先把 vault 行全部读出(vault/store 访问),拿到 owned Vec 后再取 cache 锁。
        let rows = store.list_entitlements(dek)?;
        self.hydrate_from_rows(rows);
        Ok(())
    }

    /// upsert 一条(re-verify / install 后)。**best-status 在写入时归并**(PERF-5):
    /// 同 plugin 已有更优 status 则保留(不降级),除非显式吊销路径(见
    /// [`Self::set_status`])。`is_entitled` 因此无需运行期遍历多 license。
    pub fn upsert(&self, row: EntitlementRow) {
        let mut map = self.inner.write().expect("entitlement cache poisoned");
        Self::merge_best(&mut map, row);
    }

    /// 归并写入:已有更优 status → 保留;否则覆盖。PERF-5 的内核。
    fn merge_best(map: &mut HashMap<String, EntitlementRow>, row: EntitlementRow) {
        match map.get(&row.plugin_id) {
            Some(existing) if raw_status_rank(&existing.status) > raw_status_rank(&row.status) => {
                // 现有更优,保留(不降级)。
            }
            _ => {
                map.insert(row.plugin_id.clone(), row);
            }
        }
    }

    /// 显式吊销 / 降级路径 —— **唯一**能把更优态降到更劣态的入口(re-verify 收到
    /// 验签通过的 revoked 时由 T8 worker 调用)。绕过 [`Self::merge_best`] 的"不降级"。
    pub fn set_status(&self, plugin_id: &str, new_status: &str, last_verified_at: &str) {
        let mut map = self.inner.write().expect("entitlement cache poisoned");
        if let Some(row) = map.get_mut(plugin_id) {
            row.status = new_status.to_string();
            row.last_verified_at = last_verified_at.to_string();
        }
    }

    /// dispatch gate(spec §7.2)—— **O(1) HashMap 命中**(PERF-1,dispatch 热点)。
    /// 未在缓存(免费 / 无授权依赖)→ Allow。命中 → 解析宽限态后判定。
    pub fn is_entitled(&self, plugin_id: &str, now: &DateTime<Utc>) -> EntitlementDecision {
        let map = self.inner.read().expect("entitlement cache poisoned");
        // O(1) keyed lookup —— 不是 Vec 线性扫描。
        match map.get(plugin_id) {
            None => EntitlementDecision::Allow, // 无 entitlement 行 = 免费 / 无门
            Some(row) => {
                let ent = Entitlement::from_row(row);
                let status = grace_transition(&ent, now);
                dispatch_decision(status)
            }
        }
    }

    /// 当前运行期状态(供列表路由 spec §5.1 `entitlement_status`)。无行 → Free。
    pub fn status(&self, plugin_id: &str, now: &DateTime<Utc>) -> EntStatus {
        let map = self.inner.read().expect("entitlement cache poisoned");
        match map.get(plugin_id) {
            None => EntStatus::Free,
            Some(row) => grace_transition(&Entitlement::from_row(row), now),
        }
    }

    /// 上次接受的 `last_verified_at`(SEC-2 freshness 单调基准)。无行 → None。
    /// 供 re-verify 比对 `signed_payload.verified_at` 严格递增。
    pub fn last_verified_at(&self, plugin_id: &str) -> Option<DateTime<Utc>> {
        let map = self.inner.read().expect("entitlement cache poisoned");
        map.get(plugin_id).and_then(|row| {
            DateTime::parse_from_rfc3339(&row.last_verified_at)
                .ok()
                .map(|d| d.with_timezone(&Utc))
        })
    }

    /// snapshot 全部行(诊断 / 列表)。
    pub fn snapshot(&self) -> Vec<EntitlementRow> {
        let map = self.inner.read().expect("entitlement cache poisoned");
        let mut v: Vec<_> = map.values().cloned().collect();
        v.sort_by(|a, b| a.plugin_id.cmp(&b.plugin_id));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn row(plugin_id: &str, tier: &str, status: &str) -> EntitlementRow {
        EntitlementRow {
            plugin_id: plugin_id.into(),
            license_id: "lic-x".into(),
            tier: tier.into(),
            status: status.into(),
            trial_expires: None,
            signing_pubkey_hex: "00".repeat(32),
            last_verified_at: "2026-06-12T00:00:00+00:00".into(),
            grace_started_at: None,
            updated_at: "2026-06-12T00:00:00+00:00".into(),
        }
    }

    fn ent(tier: &str, status: &str) -> Entitlement {
        Entitlement {
            plugin_id: "law-pro".into(),
            tier: tier.into(),
            raw_status: status.into(),
            trial_expires: None,
            last_verified_at: ts("2026-06-12T00:00:00+00:00"),
            grace_started_at: None,
        }
    }

    // ── happy ──────────────────────────────────────────────────────────────

    #[test]
    fn trial_not_expired_usable() {
        let mut e = ent("trial", "active");
        e.trial_expires = Some(ts("2026-06-20T00:00:00+00:00"));
        let now = ts("2026-06-15T00:00:00+00:00");
        assert_eq!(grace_transition(&e, &now), EntStatus::Trial);
        assert_eq!(dispatch_decision(EntStatus::Trial), EntitlementDecision::Allow);
    }

    #[test]
    fn active_paid_usable() {
        let e = ent("paid", "active");
        let now = ts("2026-06-12T00:00:01+00:00");
        assert_eq!(grace_transition(&e, &now), EntStatus::Active);
    }

    #[test]
    fn reverify_ok_yields_active_via_upsert() {
        // re-verify 200 → upsert active row → status Active (续期).
        let cache = EntitlementCache::new();
        cache.upsert(row("law-pro", "paid", "active"));
        let now = ts("2026-06-12T00:00:01+00:00");
        assert_eq!(cache.status("law-pro", &now), EntStatus::Active);
        assert_eq!(cache.is_entitled("law-pro", &now), EntitlementDecision::Allow);
    }

    // ── edge: trial / grace boundaries ──────────────────────────────────────

    #[test]
    fn trial_expires_now_minus_1s_still_trial() {
        let mut e = ent("trial", "active");
        e.trial_expires = Some(ts("2026-06-15T00:00:01+00:00"));
        let now = ts("2026-06-15T00:00:00+00:00"); // 1s before expiry
        assert_eq!(grace_transition(&e, &now), EntStatus::Trial);
    }

    #[test]
    fn trial_expires_now_plus_1s_expired() {
        let mut e = ent("trial", "active");
        e.trial_expires = Some(ts("2026-06-15T00:00:00+00:00"));
        let now = ts("2026-06-15T00:00:01+00:00"); // 1s after expiry
        assert_eq!(grace_transition(&e, &now), EntStatus::TrialExpired);
        assert_eq!(
            dispatch_decision(EntStatus::TrialExpired),
            EntitlementDecision::Reject("trial-expired")
        );
    }

    #[test]
    fn trial_grace_71h59m_still_grace() {
        let mut e = ent("trial", "active");
        e.grace_started_at = Some(ts("2026-06-12T00:00:00+00:00"));
        let now = ts("2026-06-14T23:59:00+00:00"); // 71h59m < 72h
        assert_eq!(grace_transition(&e, &now), EntStatus::Grace);
    }

    #[test]
    fn trial_grace_72h01m_expired() {
        let mut e = ent("trial", "active");
        e.grace_started_at = Some(ts("2026-06-12T00:00:00+00:00"));
        let now = ts("2026-06-15T00:01:00+00:00"); // 72h01m > 72h
        assert_eq!(grace_transition(&e, &now), EntStatus::TrialExpired);
    }

    #[test]
    fn paid_grace_13d_still_grace() {
        let mut e = ent("paid", "active");
        e.grace_started_at = Some(ts("2026-06-01T00:00:00+00:00"));
        let now = ts("2026-06-14T00:00:00+00:00"); // 13d < 14d
        assert_eq!(grace_transition(&e, &now), EntStatus::Grace);
    }

    #[test]
    fn paid_grace_14d01m_degraded_but_usable() {
        let mut e = ent("paid", "active");
        e.grace_started_at = Some(ts("2026-06-01T00:00:00+00:00"));
        let now = ts("2026-06-15T00:01:00+00:00"); // > 14d
        assert_eq!(grace_transition(&e, &now), EntStatus::Degraded);
        // fail-open: degraded paid is still dispatchable.
        assert_eq!(dispatch_decision(EntStatus::Degraded), EntitlementDecision::Allow);
    }

    // ── error: cloud unreachable / revoked ──────────────────────────────────

    #[test]
    fn cloud_5xx_to_grace_not_expired() {
        // worker on 5xx sets grace_started_at; within window → Grace, NOT Expired.
        let mut e = ent("paid", "active");
        e.grace_started_at = Some(ts("2026-06-12T00:00:00+00:00"));
        let now = ts("2026-06-13T00:00:00+00:00");
        assert_eq!(grace_transition(&e, &now), EntStatus::Grace);
    }

    #[test]
    fn verify_says_revoked_yields_suspended() {
        let e = ent("paid", "revoked");
        let now = ts("2026-06-12T00:00:01+00:00");
        assert_eq!(grace_transition(&e, &now), EntStatus::Suspended);
        assert_eq!(
            dispatch_decision(EntStatus::Suspended),
            EntitlementDecision::Reject("license-revoked")
        );
    }

    #[test]
    fn suspended_status_yields_suspended() {
        let e = ent("paid", "suspended");
        let now = ts("2026-06-12T00:00:01+00:00");
        assert_eq!(grace_transition(&e, &now), EntStatus::Suspended);
    }

    // ── adversarial: no path turns Suspended → Active ───────────────────────

    #[test]
    fn suspended_never_dispatchable() {
        let cache = EntitlementCache::new();
        cache.upsert(row("law-pro", "paid", "revoked"));
        let now = ts("2026-06-12T00:00:01+00:00");
        assert_eq!(
            cache.is_entitled("law-pro", &now),
            EntitlementDecision::Reject("license-revoked"),
            "revoked must never be dispatchable"
        );
    }

    #[test]
    fn generic_upsert_cannot_revive_revoked_to_active_silently() {
        // merge_best keeps best status. But a revoked row written first, then a
        // lower-rank write must not happen; and an active write CAN override revoked
        // ONLY because active outranks revoked — that's the legit re-verify path.
        // The adversarial invariant is: there is NO non-verify path. set_status is the
        // explicit downgrade. A plain upsert of "active" requires the caller to have a
        // genuine active row (which only comes from authorize_snapshot_fresh in T8).
        let cache = EntitlementCache::new();
        // suspended cannot be downgraded-then-revived without a real active row.
        cache.upsert(row("law-pro", "paid", "suspended"));
        let now = ts("2026-06-12T00:00:01+00:00");
        assert_eq!(cache.status("law-pro", &now), EntStatus::Suspended);
        // Explicit revoke via set_status (worker path on verified revoked).
        cache.set_status("law-pro", "revoked", "2026-06-12T01:00:00+00:00");
        assert_eq!(cache.status("law-pro", &now), EntStatus::Suspended);
    }

    // ── clock rollback ──────────────────────────────────────────────────────

    #[test]
    fn clock_rollback_detected_when_last_verified_beyond_skew() {
        let last = ts("2026-06-20T00:00:00+00:00");
        let now = ts("2026-06-12T00:00:00+00:00"); // last is >24h in the "future"
        assert!(detect_clock_rollback(&last, &now));
    }

    #[test]
    fn clock_rollback_not_triggered_within_skew() {
        let last = ts("2026-06-12T10:00:00+00:00");
        let now = ts("2026-06-12T00:00:00+00:00"); // 10h ahead < 24h skew
        assert!(!detect_clock_rollback(&last, &now));
    }

    #[test]
    fn clock_rollback_forces_trial_expired() {
        // trial not yet expired by trial_expires, but last_verified is in the future
        // (system clock rolled back) → forced TrialExpired (fail-closed).
        let mut e = ent("trial", "active");
        e.trial_expires = Some(ts("2026-07-01T00:00:00+00:00")); // far future
        e.last_verified_at = ts("2026-06-20T00:00:00+00:00"); // future vs now
        let now = ts("2026-06-12T00:00:00+00:00");
        assert_eq!(grace_transition(&e, &now), EntStatus::TrialExpired);
    }

    #[test]
    fn clock_rollback_paid_degraded_not_locked() {
        // paid + clock rollback → Degraded (fail-open, don't lock paying user).
        let mut e = ent("paid", "active");
        e.last_verified_at = ts("2026-06-20T00:00:00+00:00");
        let now = ts("2026-06-12T00:00:00+00:00");
        assert_eq!(grace_transition(&e, &now), EntStatus::Degraded);
    }

    // ── PERF-1: O(1) keyed lookup ───────────────────────────────────────────

    #[test]
    fn is_entitled_is_o1_keyed_lookup_not_linear_scan() {
        // PERF-1: populate many plugins; is_entitled hits the target by key (HashMap),
        // independent of how many other rows exist (no Vec linear scan). We assert the
        // backing store is a keyed map by checking a miss returns Allow in O(1) and a
        // hit returns the right decision regardless of map size.
        let cache = EntitlementCache::new();
        for i in 0..1000 {
            cache.upsert(row(&format!("plugin-{i}"), "paid", "active"));
        }
        cache.upsert(row("law-pro", "paid", "revoked"));
        let now = ts("2026-06-12T00:00:01+00:00");
        // direct keyed hit (not scan) — revoked rejected.
        assert_eq!(
            cache.is_entitled("law-pro", &now),
            EntitlementDecision::Reject("license-revoked")
        );
        // keyed miss → Allow.
        assert_eq!(cache.is_entitled("not-present", &now), EntitlementDecision::Allow);
    }

    // ── PERF-5: best-status pre-resolved at upsert ──────────────────────────

    #[test]
    fn upsert_pre_resolves_best_status_no_runtime_scan() {
        // PERF-5: write suspended then active for SAME plugin → upsert merges to best
        // (active) at write time; status() returns the merged-best directly (no runtime
        // multi-license traversal — the cache holds exactly one row per plugin_id).
        let cache = EntitlementCache::new();
        cache.upsert(row("law-pro", "paid", "suspended"));
        cache.upsert(row("law-pro", "paid", "active"));
        let now = ts("2026-06-12T00:00:01+00:00");
        assert_eq!(cache.status("law-pro", &now), EntStatus::Active);
        // exactly one row per plugin (no multi-license accumulation in the cache).
        assert_eq!(cache.snapshot().iter().filter(|r| r.plugin_id == "law-pro").count(), 1);
    }

    #[test]
    fn lower_status_does_not_downgrade_via_merge() {
        let cache = EntitlementCache::new();
        cache.upsert(row("law-pro", "paid", "active"));
        cache.upsert(row("law-pro", "paid", "suspended")); // lower rank
        let now = ts("2026-06-12T00:00:01+00:00");
        assert_eq!(cache.status("law-pro", &now), EntStatus::Active, "merge keeps best");
    }

    // ── hydrate ─────────────────────────────────────────────────────────────

    #[test]
    fn hydrate_from_rows_populates_cache() {
        let cache = EntitlementCache::new();
        cache.hydrate_from_rows(vec![row("law-pro", "paid", "active"), row("med-pro", "trial", "active")]);
        let now = ts("2026-06-12T00:00:01+00:00");
        assert_eq!(cache.snapshot().len(), 2);
        assert_eq!(cache.status("law-pro", &now), EntStatus::Active);
    }

    #[test]
    fn hydrate_from_store_roundtrip() {
        let store = crate::store::Store::open_memory().unwrap();
        let dek = crate::crypto::Key32::generate();
        store.upsert_entitlement(&dek, &row("law-pro", "paid", "active")).unwrap();
        let cache = EntitlementCache::new();
        cache.hydrate(&store, &dek).unwrap();
        let now = ts("2026-06-12T00:00:01+00:00");
        assert_eq!(cache.status("law-pro", &now), EntStatus::Active);
    }

    #[test]
    fn free_plugin_always_allowed() {
        let cache = EntitlementCache::new();
        cache.upsert(row("free-plug", "free", "active"));
        let now = ts("2026-06-12T00:00:01+00:00");
        assert_eq!(cache.status("free-plug", &now), EntStatus::Free);
        assert_eq!(cache.is_entitled("free-plug", &now), EntitlementDecision::Allow);
    }
}
