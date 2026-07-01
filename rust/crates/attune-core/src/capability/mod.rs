//! Capability Registry —— 「重能力」元数据的单一真相源(枢纽抽象)。
//!
//! 每个 Capability 记录:装 / 启 / 会员 / 本地模型 / 出网 / tier / 构建档位 / 健康 / UI 可见。
//! UI 导航(Sidebar `ui_visible`)、设置页、诊断中心(`/diagnostics/capabilities`)、插件市场、
//! 错误提示、OSS-Pro 边界(`tier`)统一查它,取代 N 处 ad-hoc 状态读取。
//!
//! **纯元数据,不放业务逻辑**(spec §11):registry 不 own provider、不跑 work;health/enabled
//! 由 server 侧 `refresh_capability_health` 从既有 `model_bootstrap`/`stack_install`/provider
//! presence 投影。本模块零 axum/network dep,可随 attune-core 编。
//!
//! 设计见 spec `2026-06-26-architecture-capability-registry-and-service-decomposition`(P0 ②)。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

/// 能力类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    /// 推理模型(embedding/reranker/llm/vlm/asr)。
    Model,
    /// 数据来源连接器(web_search/webdav/rss/...)。
    Source,
    /// 专业 agent。
    Agent,
    /// 功能特性(ocr/pluginhub/marketplace/...)。
    Feature,
}

/// OSS / Pro / Enterprise 分层(OSS-Pro 边界:OSS 构建不应暴露 pro 能力残影)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityTier {
    /// 开源通用。
    Oss,
    /// 个人行业增强(attune-pro)。
    Pro,
    /// 企业(attune-enterprise)。
    Enterprise,
}

/// 构建档位(保留元数据:core-lite 已否决,保持单一发布 ≈ standard;预留扩展)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildProfile {
    /// 标准桌面(当前唯一发布形态)。
    Standard,
    /// 预留:更重的研究/全功能档。
    Full,
}

/// 运行时健康(请求时由 server 投影)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityHealth {
    /// 就绪可用。
    Ok,
    /// 降级(部分可用 / 回退 CPU 等)。
    Degraded,
    /// 不可用(缺模型 / 栈未装 / 未登录)。
    Unavailable,
    /// 安装/下载中(首次按需拉取)。
    Installing,
}

/// 单个重能力的元数据 + 运行时健康。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capability {
    /// 稳定 id(与 EvidenceEnvelope CapabilityTag.id / UI 路由对齐)。
    pub id: String,
    /// 展示名。
    pub name: String,
    /// 类别。
    pub kind: CapabilityKind,
    /// 是否已安装(模型/栈/插件就位)。
    pub installed: bool,
    /// 是否启用(用户开关 + 运行时可用)。
    pub enabled: bool,
    /// 是否需会员。
    pub requires_member: bool,
    /// 是否需本地模型(离线推理)。
    pub requires_local_model: bool,
    /// 是否出网(与 PrivacyService/OutboundGate 对齐)。
    pub allows_outbound: bool,
    /// OSS / Pro / Enterprise。
    pub tier: CapabilityTier,
    /// 构建档位。
    pub build_profile: BuildProfile,
    /// 运行时健康。
    pub health: CapabilityHealth,
    /// UI 是否显示入口(驱动 Sidebar 收敛)。
    pub ui_visible: bool,
}

impl Capability {
    /// 内置 OSS 标准能力的默认描述符(installed/enabled/ui_visible=true,tier=Oss,health=Ok)。
    /// 链式 setter 覆盖非默认维度。
    pub fn builtin(id: impl Into<String>, name: impl Into<String>, kind: CapabilityKind) -> Self {
        Capability {
            id: id.into(),
            name: name.into(),
            kind,
            installed: true,
            enabled: true,
            requires_member: false,
            requires_local_model: false,
            allows_outbound: false,
            tier: CapabilityTier::Oss,
            build_profile: BuildProfile::Standard,
            health: CapabilityHealth::Ok,
            ui_visible: true,
        }
    }

    /// 设是否需会员。
    pub fn requires_member(mut self, v: bool) -> Self {
        self.requires_member = v;
        self
    }
    /// 设是否需本地模型。
    pub fn requires_local_model(mut self, v: bool) -> Self {
        self.requires_local_model = v;
        self
    }
    /// 设是否出网。
    pub fn allows_outbound(mut self, v: bool) -> Self {
        self.allows_outbound = v;
        self
    }
    /// 设 tier。
    pub fn tier(mut self, v: CapabilityTier) -> Self {
        self.tier = v;
        self
    }
    /// 设构建档位。
    pub fn build_profile(mut self, v: BuildProfile) -> Self {
        self.build_profile = v;
        self
    }
    /// 设健康。
    pub fn health(mut self, v: CapabilityHealth) -> Self {
        self.health = v;
        self
    }
    /// 设 installed。
    pub fn installed(mut self, v: bool) -> Self {
        self.installed = v;
        self
    }
    /// 设 enabled。
    pub fn enabled(mut self, v: bool) -> Self {
        self.enabled = v;
        self
    }
    /// 设 UI 可见。
    pub fn ui_visible(mut self, v: bool) -> Self {
        self.ui_visible = v;
        self
    }
}

/// 线程安全 Capability 注册表(纯元数据投影目标)。`Clone` 共享同一 `Arc<RwLock>`。
#[derive(Clone, Default)]
pub struct CapabilityRegistry {
    inner: Arc<RwLock<BTreeMap<String, Capability>>>,
}

impl CapabilityRegistry {
    /// 空注册表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册(按 id 幂等:同 id 覆盖)。
    pub fn register(&self, cap: Capability) {
        let mut g = self.inner.write().unwrap_or_else(|e| e.into_inner());
        g.insert(cap.id.clone(), cap);
    }

    /// 取单个(克隆)。
    pub fn get(&self, id: &str) -> Option<Capability> {
        let g = self.inner.read().unwrap_or_else(|e| e.into_inner());
        g.get(id).cloned()
    }

    /// 全量列表(id 升序,BTreeMap 决定性顺序)。
    pub fn list(&self) -> Vec<Capability> {
        let g = self.inner.read().unwrap_or_else(|e| e.into_inner());
        g.values().cloned().collect()
    }

    /// 投影快照(== list,语义别名,供诊断路由)。
    pub fn snapshot(&self) -> Vec<Capability> {
        self.list()
    }

    /// 更新已存在能力的 health;不存在返 false(不插入)。
    pub fn set_health(&self, id: &str, health: CapabilityHealth) -> bool {
        let mut g = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = g.get_mut(id) {
            c.health = health;
            true
        } else {
            false
        }
    }

    /// 更新已存在能力的 enabled;不存在返 false(不插入)。
    pub fn set_enabled(&self, id: &str, enabled: bool) -> bool {
        let mut g = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = g.get_mut(id) {
            c.enabled = enabled;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_defaults_are_oss_standard_ok() {
        let c = Capability::builtin("embedding", "Embedding", CapabilityKind::Model);
        assert_eq!(c.id, "embedding");
        assert_eq!(c.kind, CapabilityKind::Model);
        assert!(c.installed);
        assert!(c.enabled);
        assert!(!c.requires_member);
        assert!(!c.requires_local_model);
        assert!(!c.allows_outbound);
        assert_eq!(c.tier, CapabilityTier::Oss);
        assert_eq!(c.build_profile, BuildProfile::Standard);
        assert_eq!(c.health, CapabilityHealth::Ok);
        assert!(c.ui_visible);
    }

    #[test]
    fn chainable_setters_compose() {
        let c = Capability::builtin("pluginhub", "Plugin Hub", CapabilityKind::Feature)
            .requires_member(true)
            .allows_outbound(true)
            .tier(CapabilityTier::Oss)
            .ui_visible(false)
            .health(CapabilityHealth::Degraded);
        assert!(c.requires_member);
        assert!(c.allows_outbound);
        assert!(!c.ui_visible);
        assert_eq!(c.health, CapabilityHealth::Degraded);
    }

    #[test]
    fn serializes_snake_case_enums() {
        let c = Capability::builtin("web-search", "Web Search", CapabilityKind::Source)
            .allows_outbound(true);
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["kind"], "source");
        assert_eq!(v["tier"], "oss");
        assert_eq!(v["build_profile"], "standard");
        assert_eq!(v["health"], "ok");
        assert_eq!(v["allows_outbound"], true);
    }

    #[test]
    fn register_get_list_roundtrip() {
        let r = CapabilityRegistry::new();
        r.register(Capability::builtin("ocr", "OCR", CapabilityKind::Feature));
        r.register(Capability::builtin("asr", "ASR", CapabilityKind::Feature));
        assert_eq!(r.get("ocr").unwrap().name, "OCR");
        assert!(r.get("missing").is_none());
        let all = r.list();
        assert_eq!(
            all.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            vec!["asr", "ocr"]
        );
    }

    #[test]
    fn register_is_idempotent_on_id() {
        let r = CapabilityRegistry::new();
        r.register(Capability::builtin("llm", "LLM", CapabilityKind::Model));
        r.register(Capability::builtin("llm", "LLM v2", CapabilityKind::Model));
        assert_eq!(r.list().len(), 1);
        assert_eq!(r.get("llm").unwrap().name, "LLM v2");
    }

    #[test]
    fn set_health_and_enabled_mutate_existing_only() {
        let r = CapabilityRegistry::new();
        r.register(Capability::builtin(
            "embedding",
            "Embedding",
            CapabilityKind::Model,
        ));
        assert!(r.set_health("embedding", CapabilityHealth::Installing));
        assert!(r.set_enabled("embedding", false));
        let c = r.get("embedding").unwrap();
        assert_eq!(c.health, CapabilityHealth::Installing);
        assert!(!c.enabled);
        assert!(!r.set_health("nope", CapabilityHealth::Ok));
        assert!(r.get("nope").is_none());
    }

    #[test]
    fn registry_is_send_sync_and_clones_share_state() {
        let r = CapabilityRegistry::new();
        let r2 = r.clone();
        r.register(Capability::builtin("vlm", "VLM", CapabilityKind::Model));
        assert!(r2.get("vlm").is_some());
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CapabilityRegistry>();
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// list()/snapshot() 始终按 id 升序,与插入顺序无关(golden 测试可依赖的决定性顺序)。
        #[test]
        fn list_is_always_id_sorted(
            ids in proptest::collection::vec("[a-z][a-z0-9-]{0,8}", 0..20)
        ) {
            let r = CapabilityRegistry::new();
            for id in &ids {
                r.register(Capability::builtin(id, id, CapabilityKind::Feature));
            }
            let listed: Vec<String> = r.list().into_iter().map(|c| c.id).collect();
            let mut sorted = listed.clone();
            sorted.sort();
            prop_assert_eq!(listed.clone(), sorted);
            // snapshot 与 list 同序同内容(别名语义)。
            let snap: Vec<String> = r.snapshot().into_iter().map(|c| c.id).collect();
            prop_assert_eq!(snap, listed);
        }

        /// register 按 id 幂等:最终条数 == 唯一 id 数(同 id 覆盖不增条)。
        #[test]
        fn count_equals_unique_ids(
            ids in proptest::collection::vec("[a-z]{1,6}", 0..30)
        ) {
            let r = CapabilityRegistry::new();
            for id in &ids {
                r.register(Capability::builtin(id, id, CapabilityKind::Model));
            }
            let unique: std::collections::BTreeSet<_> = ids.iter().collect();
            prop_assert_eq!(r.list().len(), unique.len());
        }

        /// JSON 序列化→反序列化 roundtrip 保留全字段(snapshot 稳定性)。
        #[test]
        fn capability_json_roundtrip(
            member in any::<bool>(),
            outbound in any::<bool>(),
            vis in any::<bool>(),
            local in any::<bool>()
        ) {
            let c = Capability::builtin("x", "X", CapabilityKind::Source)
                .requires_member(member)
                .allows_outbound(outbound)
                .requires_local_model(local)
                .ui_visible(vis);
            let json = serde_json::to_string(&c).unwrap();
            let back: Capability = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(c, back);
        }

        /// set_health 只改存在的 id:对随机不存在 id 返 false 且不插入(注册集合不变)。
        #[test]
        fn set_health_only_mutates_existing(
            present in "[a-z]{1,6}",
            absent in "[A-Z]{1,6}"  // 大写域,与小写 present 永不相交
        ) {
            let r = CapabilityRegistry::new();
            r.register(Capability::builtin(&present, &present, CapabilityKind::Model));
            let before = r.list().len();
            // 存在的 id → true 且 health 改变。
            prop_assert!(r.set_health(&present, CapabilityHealth::Installing));
            prop_assert_eq!(r.get(&present).unwrap().health, CapabilityHealth::Installing);
            // 不存在的 id → false 且不插入。
            prop_assert!(!r.set_health(&absent, CapabilityHealth::Ok));
            prop_assert!(r.get(&absent).is_none());
            prop_assert_eq!(r.list().len(), before);
        }
    }
}
