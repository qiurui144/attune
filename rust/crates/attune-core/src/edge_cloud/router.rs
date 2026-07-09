//! 路由决策（纯函数）—— `decide_route(capability, capacity, privacy, account)` →
//! `RouteDecision`。
//!
//! Spec: `docs/superpowers/specs/2026-06-22-edge-cloud-model1.md` §3 数据流 / §5 / §7 边界。
//!
//! **隐私不变量（红线，§11 R1）**：`privacy == L0` ⇒ 永不返回 `Cloud`（L0 = 永不出网）。
//! decide_route 是纯函数 → 单测可穷举 + proptest 守不变量。云分支真正出网时仍必经
//! `OutboundGate::enforce`（defense-in-depth，脱敏永在 attune）。

use super::capability::{CapabilityEntry, Preference};
use super::capacity::{CapacitySignal, CapacityState};
use crate::store::audit::PrivacyTier;

/// 任务隐私级（复用既有 `PrivacyTier`）。`L0` = local-only（永不出网）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrivacyClass(pub PrivacyTier);

impl PrivacyClass {
    /// L0 = 永不出网（强制本地）。
    pub fn is_local_only(self) -> bool {
        matches!(self.0, PrivacyTier::L0)
    }
}

/// 账户档位（从 `MemberState` 派生，见 `From` 实现）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountTier {
    /// 未登录（self-host）。
    LoggedOut,
    /// 免费会员（自配 API key）。
    Free,
    /// 付费会员（gateway 下发，有月度配额）。
    Paid,
}

/// 路由用账户上下文：档位 + 剩余 LLM 配额 + 用户是否允许云出网。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountContext {
    pub tier: AccountTier,
    /// 剩余 LLM 月配额（Paid 才有意义；Free/LoggedOut 用 has_cloud_credential 表达可用性）。
    pub llm_quota_remaining: u64,
    /// 用户是否配了云出网凭据（Free=自己的 key；Paid=gateway token）且未在隐私设置里禁用云。
    pub cloud_enabled: bool,
}

impl AccountContext {
    /// 云路由是否在账户层准入（有凭据 + 未禁用 + (非付费 或 配额>0)）。
    fn cloud_admissible(&self) -> bool {
        if !self.cloud_enabled {
            return false;
        }
        match self.tier {
            // 付费：配额耗尽即不准入云。
            AccountTier::Paid => self.llm_quota_remaining > 0,
            // 免费 / 未登录：用自己的 key（无 gateway 配额概念）；cloud_enabled 已表达可用性。
            AccountTier::Free | AccountTier::LoggedOut => true,
        }
    }
}

impl From<&crate::member_session::MemberState> for AccountContext {
    /// 从客户端会员态派生账户上下文。
    ///
    /// - `Paid` → 带剩余配额；`cloud_enabled = quota>0`（gateway 下发，配额>0 即可云）。
    /// - `Free` → 自配 key；`cloud_enabled = true`（用户自己有 key 才走到这；准入由 OutboundGate
    ///   settings.enabled 二次把关）。
    /// - `LoggedOut` → self-host；`cloud_enabled = true`（用户自配，OutboundGate 二次把关）。
    fn from(m: &crate::member_session::MemberState) -> Self {
        use crate::member_session::MemberState;
        match m {
            MemberState::Paid {
                llm_quota_remaining,
                ..
            } => AccountContext {
                tier: AccountTier::Paid,
                llm_quota_remaining: *llm_quota_remaining,
                cloud_enabled: *llm_quota_remaining > 0,
            },
            MemberState::Free { .. } => AccountContext {
                tier: AccountTier::Free,
                llm_quota_remaining: 0,
                cloud_enabled: true,
            },
            MemberState::LoggedOut => AccountContext {
                tier: AccountTier::LoggedOut,
                llm_quota_remaining: 0,
                cloud_enabled: true,
            },
        }
    }
}

/// 拒绝原因（kebab，§7 错误码）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// 配额耗尽 + 任务仅云可行（本地跑不了）→ 拒 + 提示升级。
    QuotaExhaustedNoLocal,
    /// 用户禁用云 + 仅云可行 → 拒。
    CloudDisabledNoLocal,
    /// 端云都不可行（能力图 local=false ∧ cloud=false，或 L0 无本地容量）。
    NotCapableAnywhere,
}

impl RejectReason {
    pub fn as_str(self) -> &'static str {
        match self {
            RejectReason::QuotaExhaustedNoLocal => "quota-exhausted",
            RejectReason::CloudDisabledNoLocal => "cloud-disabled",
            RejectReason::NotCapableAnywhere => "l0-no-local-capacity",
        }
    }
}

/// 路由决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteDecision {
    /// 走 :8090 本地（local scheduler）。
    Local,
    /// 等本地（不去云）—— L0 忙 / 本地强偏好忙。
    QueueLocal,
    /// 走云（OutboundGate 脱敏 + cloud_client gateway）。
    Cloud,
    /// 拒绝（附原因）。
    Reject { reason: RejectReason },
}

impl RouteDecision {
    /// 稳定标签（telemetry / audit meta）。
    pub fn as_str(self) -> &'static str {
        match self {
            RouteDecision::Local => "local",
            RouteDecision::QueueLocal => "queue-local",
            RouteDecision::Cloud => "cloud",
            RouteDecision::Reject { .. } => "reject",
        }
    }

    /// 是否需要出网（仅 Cloud）。出网分支调用方**必须**经 OutboundGate（§3 红线）。
    pub fn requires_egress(self) -> bool {
        matches!(self, RouteDecision::Cloud)
    }
}

/// 端云路由决策核心（纯函数，无 I/O / 无锁）。
///
/// 决策顺序（短路）：
/// 1. **隐私 L0 红线**（最先）：L0 ⇒ 绝不 Cloud。
///    - 本地可行 + 容量 ok → Local / QueueLocal（忙则排队，绝不溢出云）。
///    - 本地可行 + Unavailable → QueueLocal（等本地恢复）。
///    - 本地不可行 → Reject{NotCapableAnywhere}（L0 不可云，宁拒不泄）。
/// 2. **能力图准入**：端云都不可行 → Reject{NotCapableAnywhere}。
/// 3. **本地优先 + 容量信号**：local_capable ∧ (ReadyFast / Unknown→静态偏好本地) → Local。
/// 4. **本地忙 → 溢出云**（若 cloud_capable ∧ 账户准入）；否则 QueueLocal（本地强偏好）/ 拒。
/// 5. **仅云可行**（35B 等）：账户准入 → Cloud；配额耗尽 → Reject{QuotaExhaustedNoLocal}；
///    云禁用 → Reject{CloudDisabledNoLocal}。
pub fn decide_route(
    entry: CapabilityEntry,
    signal: CapacitySignal,
    privacy: PrivacyClass,
    account: &AccountContext,
) -> RouteDecision {
    // ── 1. 隐私 L0 红线（最先检查，不可被任何后续逻辑越过）──────────────
    if privacy.is_local_only() {
        if !entry.local_capable {
            // L0 不可云 + 本地跑不了 → 宁拒不泄。
            return RouteDecision::Reject {
                reason: RejectReason::NotCapableAnywhere,
            };
        }
        return match signal.state {
            // 本地空闲 / Unknown(probe 失败但 local_capable) → 直接本地。
            CapacityState::ReadyFast | CapacityState::Unknown => RouteDecision::Local,
            // 本地可服务（含慢）→ 仍本地。
            CapacityState::ReadySlow => RouteDecision::Local,
            // 本地排队 → 等本地（绝不去云）。
            CapacityState::Queued => RouteDecision::QueueLocal,
            // 本地此刻不可用 → 等本地恢复（L0 永不溢出云）。
            CapacityState::Unavailable => RouteDecision::QueueLocal,
        };
    }

    // ── 2. 能力图准入：端云都不可行 → 拒 ───────────────────────────────
    if !entry.local_capable && !entry.cloud_capable {
        return RouteDecision::Reject {
            reason: RejectReason::NotCapableAnywhere,
        };
    }

    // 云准入预算（账户层）。
    let cloud_ok = entry.cloud_capable && account.cloud_admissible();

    // 云不可达时的拒因（仅云可行时用）。优先判付费配额耗尽（更可操作:提示升级/等重置），
    // 再判用户禁用云。注:Paid 形态 quota=0 时 `cloud_enabled` 派生为 false（gateway 无配额
    // 即不下发），故必须先看 tier+quota，否则会把"配额耗尽"误报成"用户禁用云"。
    let cloud_reject_reason = || -> RejectReason {
        if account.tier == AccountTier::Paid && account.llm_quota_remaining == 0 {
            RejectReason::QuotaExhaustedNoLocal
        } else if !account.cloud_enabled {
            RejectReason::CloudDisabledNoLocal
        } else {
            // 兜底（理论不可达:cloud_enabled 为真则 cloud_admissible 应已准入）。
            RejectReason::QuotaExhaustedNoLocal
        }
    };

    // ── 仅云可行（如 35B 本地跑不了）─────────────────────────────────────
    if !entry.local_capable {
        return if cloud_ok {
            RouteDecision::Cloud
        } else {
            RouteDecision::Reject {
                reason: cloud_reject_reason(),
            }
        };
    }

    // ── 3/4. 本地可行：按容量信号 + 偏好决策 ─────────────────────────────
    match signal.state {
        // 本地空闲 → 本地（任何偏好）。
        CapacityState::ReadyFast => RouteDecision::Local,
        // Unknown（probe 失败 / scheduler 未启用）→ 静态二分：按偏好。
        CapacityState::Unknown => match entry.preference {
            // 云优先且账户准入 → 云；否则本地兜底。
            Preference::CloudPreferred if cloud_ok => RouteDecision::Cloud,
            _ => RouteDecision::Local,
        },
        // 本地排队 / 慢 → 是否溢出云。
        CapacityState::Queued | CapacityState::ReadySlow => match entry.preference {
            // 本地强偏好（隐私敏感 OCR/ASR/rerank）：忙也排队本地，不溢云。
            Preference::LocalStrong => RouteDecision::QueueLocal,
            // 本地优先 / 云优先：本地忙 → 溢出云（若准入）；否则排队本地。
            Preference::LocalPreferred | Preference::CloudPreferred => {
                if cloud_ok {
                    RouteDecision::Cloud
                } else {
                    // 云不准入 → 本地兜底排队（不拒，本地能跑）。
                    RouteDecision::QueueLocal
                }
            }
        },
        // 本地此刻不可用：溢出云（若准入）；否则——
        CapacityState::Unavailable => {
            if cloud_ok {
                RouteDecision::Cloud
            } else if entry.preference == Preference::LocalStrong {
                // 本地强偏好 + 云不准入 → 等本地恢复（非隐私拒，可排队）。
                RouteDecision::QueueLocal
            } else {
                // 本地不可用 + 云不准入 → 拒（本地此刻真跑不了）。
                RouteDecision::Reject {
                    reason: cloud_reject_reason(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge_cloud::capacity::CapacitySignal;

    fn sig(state: CapacityState) -> CapacitySignal {
        CapacitySignal {
            state,
            eta_ms: 0,
            mem_headroom_mb: 1024,
        }
    }
    fn local_pref() -> CapabilityEntry {
        CapabilityEntry {
            local_capable: true,
            cloud_capable: true,
            preference: Preference::LocalPreferred,
        }
    }
    fn local_strong() -> CapabilityEntry {
        CapabilityEntry {
            local_capable: true,
            cloud_capable: true,
            preference: Preference::LocalStrong,
        }
    }
    fn cloud_only() -> CapabilityEntry {
        CapabilityEntry {
            local_capable: false,
            cloud_capable: true,
            preference: Preference::CloudPreferred,
        }
    }
    fn paid(quota: u64) -> AccountContext {
        AccountContext {
            tier: AccountTier::Paid,
            llm_quota_remaining: quota,
            cloud_enabled: quota > 0,
        }
    }
    fn free_cloud() -> AccountContext {
        AccountContext {
            tier: AccountTier::Free,
            llm_quota_remaining: 0,
            cloud_enabled: true,
        }
    }
    fn l1() -> PrivacyClass {
        PrivacyClass(PrivacyTier::L1)
    }
    fn l0() -> PrivacyClass {
        PrivacyClass(PrivacyTier::L0)
    }

    // ── happy ──────────────────────────────────────────────────────────────

    #[test]
    fn ready_fast_local_capable_routes_local() {
        let d = decide_route(
            local_pref(),
            sig(CapacityState::ReadyFast),
            l1(),
            &paid(1000),
        );
        assert_eq!(d, RouteDecision::Local);
    }

    #[test]
    fn local_busy_local_preferred_spills_to_cloud() {
        let d = decide_route(local_pref(), sig(CapacityState::Queued), l1(), &paid(1000));
        assert_eq!(d, RouteDecision::Cloud);
    }

    #[test]
    fn unavailable_local_capable_spills_to_cloud() {
        let d = decide_route(
            local_pref(),
            sig(CapacityState::Unavailable),
            l1(),
            &paid(1000),
        );
        assert_eq!(d, RouteDecision::Cloud);
    }

    #[test]
    fn cloud_only_with_quota_routes_cloud() {
        // 35B 本地跑不了 + 有配额 → 云。
        let d = decide_route(cloud_only(), sig(CapacityState::Unknown), l1(), &paid(1000));
        assert_eq!(d, RouteDecision::Cloud);
    }

    // ── 隐私红线（adversarial）──────────────────────────────────────────────

    #[test]
    fn l0_busy_never_goes_cloud_queues_local() {
        // L0 + 本地忙 → 排队本地，绝不 Cloud（核心红线）。
        let d = decide_route(
            local_pref(),
            sig(CapacityState::Queued),
            l0(),
            &paid(1_000_000),
        );
        assert_eq!(d, RouteDecision::QueueLocal);
        assert!(!d.requires_egress(), "L0 must never require egress");
    }

    #[test]
    fn l0_unavailable_never_goes_cloud_queues_local() {
        // L0 + 本地 Unavailable（即使有充足配额、云可行）→ 仍排队本地，不溢出云。
        let d = decide_route(
            local_pref(),
            sig(CapacityState::Unavailable),
            l0(),
            &paid(1_000_000),
        );
        assert_eq!(d, RouteDecision::QueueLocal);
        assert!(!d.requires_egress());
    }

    #[test]
    fn l0_ready_fast_routes_local() {
        let d = decide_route(
            local_pref(),
            sig(CapacityState::ReadyFast),
            l0(),
            &paid(1000),
        );
        assert_eq!(d, RouteDecision::Local);
    }

    #[test]
    fn l0_not_local_capable_rejects_never_cloud() {
        // L0 + 本地跑不了（35B）→ 宁拒不泄，绝不去云。
        let d = decide_route(
            cloud_only(),
            sig(CapacityState::Unknown),
            l0(),
            &paid(1_000_000),
        );
        assert_eq!(
            d,
            RouteDecision::Reject {
                reason: RejectReason::NotCapableAnywhere
            }
        );
        assert!(!d.requires_egress());
    }

    #[test]
    fn l0_probe_unknown_routes_local_not_cloud() {
        // L0 + probe 失败(Unknown) + local_capable → 本地（绝不因 Unknown 去云）。
        let d = decide_route(
            cloud_only_but_local(),
            sig(CapacityState::Unknown),
            l0(),
            &paid(1000),
        );
        assert_eq!(d, RouteDecision::Local);
    }
    fn cloud_only_but_local() -> CapabilityEntry {
        CapabilityEntry {
            local_capable: true,
            cloud_capable: true,
            preference: Preference::CloudPreferred,
        }
    }

    // ── 配额 / 准入 ──────────────────────────────────────────────────────────

    #[test]
    fn quota_exhausted_but_local_capable_falls_back_local() {
        // 配额=0 + 任务可本地 + 本地空闲 → 本地兜底（不拒）。
        let d = decide_route(local_pref(), sig(CapacityState::ReadyFast), l1(), &paid(0));
        assert_eq!(d, RouteDecision::Local);
    }

    #[test]
    fn quota_exhausted_local_busy_queues_local_not_cloud() {
        // 配额=0 + 本地忙 + 仅本地兜底（云不准入）→ 排队本地，不溢云。
        let d = decide_route(local_pref(), sig(CapacityState::Queued), l1(), &paid(0));
        assert_eq!(d, RouteDecision::QueueLocal);
    }

    #[test]
    fn quota_exhausted_cloud_only_rejects_with_upgrade_hint() {
        // 配额=0 + 仅云可行（35B）→ 拒 + 升级提示。
        let d = decide_route(cloud_only(), sig(CapacityState::Unknown), l1(), &paid(0));
        assert_eq!(
            d,
            RouteDecision::Reject {
                reason: RejectReason::QuotaExhaustedNoLocal
            }
        );
    }

    #[test]
    fn cloud_disabled_cloud_only_rejects() {
        // 用户禁用云 + 仅云可行 → 拒（cloud-disabled）。
        let acct = AccountContext {
            tier: AccountTier::Free,
            llm_quota_remaining: 0,
            cloud_enabled: false,
        };
        let d = decide_route(cloud_only(), sig(CapacityState::Unknown), l1(), &acct);
        assert_eq!(
            d,
            RouteDecision::Reject {
                reason: RejectReason::CloudDisabledNoLocal
            }
        );
    }

    // ── 本地强偏好（OCR/ASR/rerank 隐私敏感）─────────────────────────────────

    #[test]
    fn local_strong_busy_queues_local_not_cloud() {
        // 本地强偏好（OCR/ASR）+ 本地忙 → 排队本地，不溢云（即使可云有配额）。
        let d = decide_route(
            local_strong(),
            sig(CapacityState::Queued),
            l1(),
            &paid(1_000_000),
        );
        assert_eq!(d, RouteDecision::QueueLocal);
    }

    #[test]
    fn local_strong_unavailable_spills_cloud_if_admissible() {
        // 本地强偏好 + Unavailable（本地真跑不了）+ 可云 → 溢出云（非隐私拒，可降级）。
        let d = decide_route(
            local_strong(),
            sig(CapacityState::Unavailable),
            l1(),
            &paid(1000),
        );
        assert_eq!(d, RouteDecision::Cloud);
    }

    // ── 静态二分（Unknown probe）─────────────────────────────────────────────

    #[test]
    fn unknown_local_preferred_routes_local() {
        // probe 失败 + 本地优先 → 本地（静态偏好）。
        let d = decide_route(local_pref(), sig(CapacityState::Unknown), l1(), &paid(1000));
        assert_eq!(d, RouteDecision::Local);
    }

    #[test]
    fn unknown_cloud_preferred_local_capable_routes_cloud_if_admissible() {
        let e = CapabilityEntry {
            local_capable: true,
            cloud_capable: true,
            preference: Preference::CloudPreferred,
        };
        let d = decide_route(e, sig(CapacityState::Unknown), l1(), &paid(1000));
        assert_eq!(d, RouteDecision::Cloud);
    }

    #[test]
    fn free_user_with_own_key_routes_cloud_on_spill() {
        let d = decide_route(
            local_pref(),
            sig(CapacityState::Queued),
            l1(),
            &free_cloud(),
        );
        assert_eq!(d, RouteDecision::Cloud);
    }

    #[test]
    fn account_context_from_member_state() {
        use crate::member_session::MemberState;
        let paid = MemberState::Paid {
            account_id: "a".into(),
            license_id: "l".into(),
            llm_quota_remaining: 500,
        };
        let ctx: AccountContext = (&paid).into();
        assert_eq!(ctx.tier, AccountTier::Paid);
        assert_eq!(ctx.llm_quota_remaining, 500);
        assert!(ctx.cloud_enabled);

        let paid0 = MemberState::Paid {
            account_id: "a".into(),
            license_id: "l".into(),
            llm_quota_remaining: 0,
        };
        let ctx0: AccountContext = (&paid0).into();
        assert!(!ctx0.cloud_enabled, "quota=0 → cloud not enabled");

        let lo: AccountContext = (&MemberState::LoggedOut).into();
        assert_eq!(lo.tier, AccountTier::LoggedOut);
    }

    #[test]
    fn decision_labels_stable_kebab() {
        assert_eq!(RouteDecision::Local.as_str(), "local");
        assert_eq!(RouteDecision::QueueLocal.as_str(), "queue-local");
        assert_eq!(RouteDecision::Cloud.as_str(), "cloud");
        assert_eq!(
            RouteDecision::Reject {
                reason: RejectReason::QuotaExhaustedNoLocal
            }
            .as_str(),
            "reject"
        );
    }

    // ── proptest：隐私不变量 + 永不 panic ──────────────────────────────────

    use proptest::prelude::*;

    fn arb_state() -> impl Strategy<Value = CapacityState> {
        prop_oneof![
            Just(CapacityState::ReadyFast),
            Just(CapacityState::Queued),
            Just(CapacityState::ReadySlow),
            Just(CapacityState::Unavailable),
            Just(CapacityState::Unknown),
        ]
    }
    fn arb_entry() -> impl Strategy<Value = CapabilityEntry> {
        (any::<bool>(), any::<bool>(), 0u8..3).prop_map(|(l, c, p)| CapabilityEntry {
            local_capable: l,
            cloud_capable: c,
            preference: match p {
                0 => Preference::LocalPreferred,
                1 => Preference::LocalStrong,
                _ => Preference::CloudPreferred,
            },
        })
    }
    fn arb_account() -> impl Strategy<Value = AccountContext> {
        (0u8..3, any::<u64>(), any::<bool>()).prop_map(|(t, q, e)| AccountContext {
            tier: match t {
                0 => AccountTier::LoggedOut,
                1 => AccountTier::Free,
                _ => AccountTier::Paid,
            },
            llm_quota_remaining: q,
            cloud_enabled: e,
        })
    }

    proptest! {
        /// 不变量 1（红线）：L0 永远不返回 Cloud / 永不 requires_egress。
        #[test]
        fn prop_l0_never_routes_cloud(entry in arb_entry(), state in arb_state(), acct in arb_account()) {
            let d = decide_route(entry, sig(state), PrivacyClass(PrivacyTier::L0), &acct);
            prop_assert_ne!(d, RouteDecision::Cloud);
            prop_assert!(!d.requires_egress(), "L0 decision must never require egress: {:?}", d);
        }

        /// 不变量 2：decide_route 对任意输入永不 panic（穷举组合）。
        #[test]
        fn prop_never_panics(entry in arb_entry(), state in arb_state(), tier in 0u8..3, acct in arb_account()) {
            let privacy = PrivacyClass(match tier { 0 => PrivacyTier::L0, 1 => PrivacyTier::L1, _ => PrivacyTier::L3 });
            let _ = decide_route(entry, sig(state), privacy, &acct);
        }

        /// 不变量 3：Cloud 决策 ⇒ cloud_capable ∧ 账户准入（不会无凭据出网）。
        #[test]
        fn prop_cloud_only_when_admissible(entry in arb_entry(), state in arb_state(), acct in arb_account()) {
            // 非 L0（L0 已由不变量 1 守）。
            let d = decide_route(entry, sig(state), PrivacyClass(PrivacyTier::L1), &acct);
            if d == RouteDecision::Cloud {
                prop_assert!(entry.cloud_capable, "Cloud decision requires cloud_capable");
                prop_assert!(acct.cloud_admissible(), "Cloud decision requires account cloud admission");
            }
        }
    }
}
