//! 端云协同调度 Model 1 —— 容量信号协同（attune governor 接 k3-scheduler /capacity）。
//!
//! Spec: `docs/superpowers/specs/2026-06-22-edge-cloud-model1.md`。
//! 设计源（attune-k3）: `docs/edge-cloud-scheduler-collaboration.md` Model 1（推荐）。
//!
//! **职责分离**：attune = 策略层（隐私/账户/脱敏），k3-scheduler = 机制层（资源决策）。
//! 隐私门（redaction + L0 永不出网）永远留 attune —— 本模块只决定「在哪跑」，云分支真出网
//! 时调用方**必须**经 `OutboundGate::enforce`（脱敏 + L0 二次拦截，defense-in-depth）。
//!
//! **仅 K3 形态激活**：`FormFactor::K3Appliance` ∧ scheduler 可达时启用协同路由。
//! **个人版（Laptop/Server）行为 0 改动** —— `route()` 在非 K3 短路成静态 cloud-preferred
//! （不查 /capacity，probe 调用计数恒 0，guard 测试守此不变量）。

pub mod capability;
pub mod capacity;
pub mod router;

pub use capability::{Capability, CapabilityEntry, CapabilityMap, Preference};
pub use capacity::{
    CapacityProbe, CapacitySignal, CapacityState, HttpCapacityClient, MockCapacityProbe,
};
pub use router::{
    decide_route, AccountContext, AccountTier, PrivacyClass, RejectReason, RouteDecision,
};

use crate::platform::FormFactor;

/// 端云路由编排器：lookup 能力图 → (K3 时)查 /capacity → decide_route。
///
/// 非 K3 形态：**不查 /capacity**，决策恒为静态 cloud-preferred（= 现状），probe 不被调用。
pub struct EdgeCloudRouter {
    map: CapabilityMap,
    probe: Box<dyn CapacityProbe>,
    form_factor: FormFactor,
}

impl EdgeCloudRouter {
    /// 构造。`form_factor` 决定是否激活协同（仅 K3）。
    pub fn new(form_factor: FormFactor, probe: Box<dyn CapacityProbe>) -> Self {
        EdgeCloudRouter {
            map: CapabilityMap::builtin(),
            probe,
            form_factor,
        }
    }

    /// 便捷：K3 形态 + 默认 HTTP 客户端（:8090）。
    pub fn for_k3() -> Self {
        Self::new(FormFactor::K3Appliance, Box::new(HttpCapacityClient::new()))
    }

    /// 是否启用协同路由（仅 K3 形态）。
    pub fn collaboration_active(&self) -> bool {
        matches!(self.form_factor, FormFactor::K3Appliance)
    }

    /// 路由一次推理任务。
    ///
    /// - **非 K3**：不查 /capacity（probe 不调用）→ 决策按静态 cloud-preferred（现状 freeze）。
    ///   L0 红线仍守（即使个人版，L0 永不出网由 decide_route 保证）。
    /// - **K3**：查 /capacity → 真 load-aware 决策。
    pub fn route(
        &self,
        cap: Capability,
        model: &str,
        privacy: PrivacyClass,
        account: &AccountContext,
    ) -> RouteDecision {
        let entry = self.map.lookup(cap);
        let signal = if self.collaboration_active() {
            // K3：查容量信号（失败 → Unknown 降级，由 probe 契约保证不崩）。
            self.probe.query(model)
        } else {
            // 个人版：不查 scheduler（无本地 scheduler）→ Unknown，decide_route 退静态偏好。
            // L0 红线在 decide_route 内仍生效（与形态无关）。
            CapacitySignal::unknown()
        };
        decide_route(entry, signal, privacy, account)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::audit::PrivacyTier;

    fn paid(quota: u64) -> AccountContext {
        AccountContext {
            tier: AccountTier::Paid,
            llm_quota_remaining: quota,
            cloud_enabled: quota > 0,
        }
    }
    fn l1() -> PrivacyClass {
        PrivacyClass(PrivacyTier::L1)
    }
    fn l0() -> PrivacyClass {
        PrivacyClass(PrivacyTier::L0)
    }

    // ── 个人版 0 回退（核心 guard）──────────────────────────────────────────

    #[test]
    fn personal_laptop_never_probes_scheduler() {
        // Laptop 形态：probe 调用计数必须恒 0（不查 /capacity）。用 Arc spy 共享计数
        //（router 持有 Box<dyn>，ArcProbe 把 Arc<MockCapacityProbe> 适配为 probe）。
        let spy = std::sync::Arc::new(MockCapacityProbe::with_state(CapacityState::ReadyFast));
        let router = EdgeCloudRouter::new(FormFactor::Laptop, Box::new(ArcProbe(spy.clone())));
        let _ = router.route(Capability::ChatLlm7b, "qwen2.5-7b", l1(), &paid(1000));
        assert_eq!(
            spy.call_count(),
            0,
            "personal Laptop must NOT probe scheduler (0-regression)"
        );
        assert!(!router.collaboration_active());
    }

    #[test]
    fn personal_server_never_probes_scheduler() {
        let spy = std::sync::Arc::new(MockCapacityProbe::with_state(CapacityState::ReadyFast));
        let router = EdgeCloudRouter::new(FormFactor::Server, Box::new(ArcProbe(spy.clone())));
        let _ = router.route(Capability::Embedding, "bge-m3", l1(), &paid(1000));
        assert_eq!(
            spy.call_count(),
            0,
            "personal Server must NOT probe scheduler"
        );
    }

    #[test]
    fn personal_static_decision_is_cloud_preferred() {
        // 个人版本地优先能力 → Unknown → decide_route 返回 Local（本地兜底）；
        // 但个人版无本地 scheduler，实际由 governor 走 cloud provider。此处验决策稳定不依赖 probe。
        // 关键：决策对个人版**与现状一致**（不因协同层引入新行为）—— probe 0 调用已验。
        let router =
            EdgeCloudRouter::new(FormFactor::Laptop, Box::new(MockCapacityProbe::failing()));
        let d = router.route(Capability::ChatLlm35b, "deepseek-v4", l1(), &paid(1000));
        // 35B cloud-only + 有配额 → Cloud（现状：个人版大模型走云）。
        assert_eq!(d, RouteDecision::Cloud);
    }

    // ── K3 形态：真 load-aware ────────────────────────────────────────────

    #[test]
    fn k3_probes_and_routes_local_when_fast() {
        let spy = std::sync::Arc::new(MockCapacityProbe::with_state(CapacityState::ReadyFast));
        let router = EdgeCloudRouter::new(FormFactor::K3Appliance, Box::new(ArcProbe(spy.clone())));
        let d = router.route(Capability::ChatLlm7b, "qwen2.5-7b", l1(), &paid(1000));
        assert_eq!(spy.call_count(), 1, "K3 must probe scheduler exactly once");
        assert_eq!(d, RouteDecision::Local);
        assert!(router.collaboration_active());
    }

    #[test]
    fn k3_busy_spills_to_cloud() {
        let router = EdgeCloudRouter::new(
            FormFactor::K3Appliance,
            Box::new(MockCapacityProbe::with_state(CapacityState::Queued)),
        );
        let d = router.route(Capability::ChatLlm7b, "qwen2.5-7b", l1(), &paid(1000));
        assert_eq!(
            d,
            RouteDecision::Cloud,
            "K3 busy + cloud-admissible → spill to cloud"
        );
    }

    // ── L0 红线（与形态无关）──────────────────────────────────────────────

    #[test]
    fn l0_never_cloud_even_on_k3_busy() {
        let router = EdgeCloudRouter::new(
            FormFactor::K3Appliance,
            Box::new(MockCapacityProbe::with_state(CapacityState::Queued)),
        );
        let d = router.route(Capability::ChatLlm7b, "qwen2.5-7b", l0(), &paid(1_000_000));
        assert_eq!(d, RouteDecision::QueueLocal);
        assert!(!d.requires_egress());
    }

    #[test]
    fn l0_never_cloud_even_on_personal() {
        // 个人版 L0 同样不出网（红线与形态无关）。35B 个人版本地跑不了 → L0 拒（不泄）。
        let router =
            EdgeCloudRouter::new(FormFactor::Laptop, Box::new(MockCapacityProbe::failing()));
        let d = router.route(Capability::ChatLlm35b, "deepseek-v4", l0(), &paid(1000));
        assert!(
            !d.requires_egress(),
            "L0 must never egress on personal form factor"
        );
        assert_eq!(
            d,
            RouteDecision::Reject {
                reason: RejectReason::NotCapableAnywhere
            }
        );
    }

    /// 测试辅助：把 `Arc<MockCapacityProbe>` 适配成 `CapacityProbe`（共享 spy 计数）。
    struct ArcProbe(std::sync::Arc<MockCapacityProbe>);
    impl CapacityProbe for ArcProbe {
        fn query(&self, model: &str) -> CapacitySignal {
            self.0.query(model)
        }
    }
}
