//! 能力图 SSOT（capability map）—— 每能力（embedding/rerank/ocr/asr/chat-3b/7b/35b）→
//! `{local 可行, cloud 可行, 默认偏好}`。
//!
//! Spec: `docs/superpowers/specs/2026-06-22-edge-cloud-model1.md` §4 表 / §5。
//!
//! 设计源 doc（attune-k3）`edge-cloud-scheduler-collaboration.md` §4 能力图：
//! | 能力 | 本地 | 云 | 默认偏好 |
//! |---|---|---|---|
//! | embedding (bge-m3) | ✅ | ✅ | 本地优先 |
//! | rerank / OCR / ASR | ✅ | ⚠️(隐私敏感,慎云) | 本地强偏好 |
//! | chat LLM 3B/7B | ✅ | ✅ | 本地优先,忙→云 |
//! | chat LLM 35B-A3B | ⚠️ 仅 32G | ✅ | 32G 本地 / 16G 云 |
//!
//! **数据驱动但内置 baseline 兜底**：内置 `builtin()` 是 SSOT 的字节级 freeze；预留远程
//! 覆盖（§6 扩展点，本次不实现远程拉取）。`lookup()` 对未登记能力返回保守默认（cloud-only
//! preferred）而非 panic。

use std::collections::BTreeMap;

/// attune 端云协同覆盖的能力集合。新增能力 = 加 variant + `builtin()` 加一行。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    Embedding,
    Rerank,
    Ocr,
    Asr,
    /// 对话 LLM 3B 档（16G 本地可跑）。
    ChatLlm3b,
    /// 对话 LLM 7B 档（K3 16G 主推 Qwen2.5-7B q4）。
    ChatLlm7b,
    /// 对话 LLM 35B-A3B 档（仅 32G 本地；16G 必须走云）。
    ChatLlm35b,
}

impl Capability {
    /// 稳定 kebab 标签（telemetry / UI / 诊断用）。
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Embedding => "embedding",
            Capability::Rerank => "rerank",
            Capability::Ocr => "ocr",
            Capability::Asr => "asr",
            Capability::ChatLlm3b => "chat-llm-3b",
            Capability::ChatLlm7b => "chat-llm-7b",
            Capability::ChatLlm35b => "chat-llm-35b",
        }
    }

    /// 全能力（测试 / 遍历用）。
    pub const ALL: [Capability; 7] = [
        Capability::Embedding,
        Capability::Rerank,
        Capability::Ocr,
        Capability::Asr,
        Capability::ChatLlm3b,
        Capability::ChatLlm7b,
        Capability::ChatLlm35b,
    ];
}

/// 默认路由偏好。决策时与容量信号叠加（见 `router::decide_route`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preference {
    /// 本地优先：本地空闲→本地；本地忙→可云（若可云）。
    LocalPreferred,
    /// 本地强偏好（隐私敏感 OCR/ASR/rerank）：本地忙也优先排队本地，仅 Unavailable 才考虑云。
    LocalStrong,
    /// 云优先：默认走云（本地跑不了的大模型 / 个人版静态路径）。
    CloudPreferred,
}

/// 单个能力的端云可行性 + 偏好。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityEntry {
    pub local_capable: bool,
    pub cloud_capable: bool,
    pub preference: Preference,
}

impl CapabilityEntry {
    /// 保守默认（未登记能力）：云可行、本地不可行、云优先。绝不 panic。
    pub const fn conservative_default() -> Self {
        CapabilityEntry {
            local_capable: false,
            cloud_capable: true,
            preference: Preference::CloudPreferred,
        }
    }
}

/// 能力图（capability → entry）。BTreeMap 保证序确定（diff / 测试稳定）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityMap {
    entries: BTreeMap<Capability, CapabilityEntry>,
}

impl CapabilityMap {
    /// 内置 baseline SSOT（编进二进制；离线/首发兜底）。
    ///
    /// 与设计 doc §4 能力图表一一对应。35B 在 K3 16G 形态本地跑不了 → `local_capable: false`
    /// + cloud preferred（本地跑不了的大模型走云，spec §2）。
    pub fn builtin() -> Self {
        let mut entries = BTreeMap::new();
        // embedding：本地 + 云均可，本地优先（建库阶段零云成本）。
        entries.insert(
            Capability::Embedding,
            CapabilityEntry { local_capable: true, cloud_capable: true, preference: Preference::LocalPreferred },
        );
        // rerank / OCR / ASR：本地可行，云慎用（隐私敏感）→ 本地强偏好。
        for c in [Capability::Rerank, Capability::Ocr, Capability::Asr] {
            entries.insert(
                c,
                CapabilityEntry { local_capable: true, cloud_capable: true, preference: Preference::LocalStrong },
            );
        }
        // chat 3B/7B：本地 + 云均可，本地优先（忙→云）。
        for c in [Capability::ChatLlm3b, Capability::ChatLlm7b] {
            entries.insert(
                c,
                CapabilityEntry { local_capable: true, cloud_capable: true, preference: Preference::LocalPreferred },
            );
        }
        // chat 35B-A3B：K3 16G 主推形态本地跑不了 → 仅云。
        entries.insert(
            Capability::ChatLlm35b,
            CapabilityEntry { local_capable: false, cloud_capable: true, preference: Preference::CloudPreferred },
        );
        CapabilityMap { entries }
    }

    /// 查能力 entry。未登记 → 保守默认（cloud-only preferred），绝不 panic。
    pub fn lookup(&self, c: Capability) -> CapabilityEntry {
        self.entries.get(&c).copied().unwrap_or_else(CapabilityEntry::conservative_default)
    }
}

impl Default for CapabilityMap {
    fn default() -> Self {
        Self::builtin()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── golden：内置 baseline 每能力对齐 doc §4 表 ─────────────────────────

    #[test]
    fn builtin_has_all_capabilities() {
        let m = CapabilityMap::builtin();
        for c in Capability::ALL {
            // lookup 永不 panic；登记的都非保守默认。
            let e = m.lookup(c);
            assert!(e.cloud_capable, "{} cloud_capable", c.as_str());
        }
    }

    #[test]
    fn embedding_local_preferred() {
        let e = CapabilityMap::builtin().lookup(Capability::Embedding);
        assert!(e.local_capable && e.cloud_capable);
        assert_eq!(e.preference, Preference::LocalPreferred);
    }

    #[test]
    fn ocr_asr_rerank_local_strong() {
        let m = CapabilityMap::builtin();
        for c in [Capability::Ocr, Capability::Asr, Capability::Rerank] {
            let e = m.lookup(c);
            assert_eq!(e.preference, Preference::LocalStrong, "{} privacy-sensitive → local strong", c.as_str());
            assert!(e.local_capable);
        }
    }

    #[test]
    fn chat_3b_7b_local_preferred() {
        let m = CapabilityMap::builtin();
        for c in [Capability::ChatLlm3b, Capability::ChatLlm7b] {
            let e = m.lookup(c);
            assert!(e.local_capable && e.cloud_capable);
            assert_eq!(e.preference, Preference::LocalPreferred);
        }
    }

    #[test]
    fn chat_35b_cloud_only() {
        // 35B-A3B 在 K3 16G 主推形态本地跑不了 → local_capable=false + cloud preferred。
        let e = CapabilityMap::builtin().lookup(Capability::ChatLlm35b);
        assert!(!e.local_capable, "35B not local-capable on 16G");
        assert!(e.cloud_capable);
        assert_eq!(e.preference, Preference::CloudPreferred);
    }

    // ── 边界：未登记能力保守默认 ───────────────────────────────────────────

    #[test]
    fn empty_map_lookup_returns_conservative_default() {
        let m = CapabilityMap { entries: BTreeMap::new() };
        let e = m.lookup(Capability::Embedding);
        assert_eq!(e, CapabilityEntry::conservative_default());
        assert!(!e.local_capable && e.cloud_capable);
    }

    #[test]
    fn capability_labels_stable_kebab() {
        assert_eq!(Capability::Embedding.as_str(), "embedding");
        assert_eq!(Capability::ChatLlm35b.as_str(), "chat-llm-35b");
        assert_eq!(Capability::Asr.as_str(), "asr");
    }
}
