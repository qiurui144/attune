//! 证据化输出契约 `EvidenceEnvelope` —— 专业 agent 强制结构化证据(不允许只返自然语言)。
//!
//! 横切叠加于 [`crate::AgentOutput`](crate::AgentOutput):envelope 是**元数据层**,与
//! 现有 4 种输出模式(结构化结果 / 标记报告 / 批阅 / 叙述)正交,不替代 computation。
//! 七维:`inputs / steps / evidence / confidence / artifacts / warnings`(+ `capabilities`
//! 能力透明度)。设计见 spec `2026-06-26-pro-agent-evidence-output-contract`。
//!
//! **leaf 不变量**:本模块只能 serde,零 native dep(与本 crate 整体约束一致);校验器
//! `validate_envelope`(grounding 真实性 / 完备性)放 non-leaf `pro-infra`,不在此 leaf。
//! 向后兼容:`AgentOutput.evidence` 为 `Option` + `skip_serializing_if`,`None` 时 wire 不变。

use serde::{Deserialize, Serialize};

/// 证据化信封 schema 版本。未知版本应 graceful deserialize(旧消费者不 crash)。
pub const ENVELOPE_VERSION: u32 = 1;

/// 专业 agent 的强制结构化证据信封(横切于 AgentOutput.computation)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceEnvelope {
    /// schema 版本(向前兼容:未知版本仍可反序列化)。
    #[serde(default = "default_envelope_version")]
    pub envelope_version: u32,
    /// 任务输入快照(脱敏:仅 hash + 摘要,不落原始敏感内容)。
    pub inputs: InputsSnapshot,
    /// 有序推理 / 检索 / 调用步骤。
    #[serde(default)]
    pub steps: Vec<StepRecord>,
    /// 每条结论的 grounding 证据(可机械校验 quoted ∈ 源)。
    #[serde(default)]
    pub evidence: Vec<EvidenceItem>,
    /// 结构化置信度(禁裸 f64:必带 level + rationale)。
    pub confidence: Confidence,
    /// 产出物(导出文件 / 表格 / 图)。
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
    /// 不确定 / 越界 / 需人工复核(红线统一出口:law 提示律师 / medical 无诊断)。
    #[serde(default)]
    pub warnings: Vec<Warning>,
    /// 能力透明度:本次用到的 capability(检索 / VLM / 外部源 / 计算)。
    #[serde(default)]
    pub capabilities: Vec<CapabilityTag>,
}

fn default_envelope_version() -> u32 {
    ENVELOPE_VERSION
}

/// 任务输入快照(脱敏,守隐私)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputsSnapshot {
    /// 输入内容 hash(去重 / 审计锚,不暴露原文)。
    pub hash: String,
    /// 脱敏摘要(人类可读,不含敏感字段原值)。
    pub summary: String,
    /// 场景类型(law / patent / academic / tech / presales / medical / ...)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case_kind: Option<String>,
    /// 输出模式(structured / annotated_report / review / narrative)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<String>,
}

/// 单步推理 / 检索 / 调用记录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepRecord {
    /// 步序(从 0 起)。
    pub order: u32,
    /// 步骤类别。
    pub kind: StepKind,
    /// 步骤说明(做了什么 / 命中什么)。
    pub detail: String,
}

/// 步骤类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    /// 检索(FTS / 向量 / 混合)。
    Retrieve,
    /// 确定性计算。
    Compute,
    /// LLM 调用。
    Llm,
    /// VLM(视觉)调用。
    Vlm,
    /// 外部源(API / 联网,受 OutboundGate)。
    External,
    /// 其它。
    #[serde(other)]
    Other,
}

/// 单条 grounding 证据。`quoted_text` 应可在 `source_ref` 指向的源中机械校验。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceItem {
    /// 证据类别。
    pub kind: EvidenceKind,
    /// 源标识(doc_id / 文献 id / 法条号 / 权利要求号 / 检索命中 id)。
    pub source_ref: String,
    /// 源内偏移(字符 offset 或页/行,可选)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    /// 被引原文(可机械校验 ∈ 源 pack)。
    pub quoted_text: String,
    /// 权威引用(法条全称 / 期刊 + DOI / 标准号,可选)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority_ref: Option<String>,
    /// 是否已通过 grounding 真实性校验(由 non-leaf 校验器置位)。
    #[serde(default)]
    pub verifiable: bool,
}

/// 证据类别(场景化)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    /// 法条(law)。
    Statute,
    /// 权利要求 / 专利(patent)。
    Claim,
    /// 文献(academic)。
    Literature,
    /// API / 标准文档(tech)。
    ApiDoc,
    /// 图像 / 视觉(VLM)。
    Image,
    /// 知识库检索命中。
    Retrieval,
    /// 其它。
    #[serde(other)]
    Other,
}

/// 结构化置信度(禁裸 f64)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Confidence {
    /// 数值置信 0.0 - 1.0。
    pub score: f64,
    /// 分级(供 UI 直显)。
    pub level: ConfidenceLevel,
    /// 置信依据(为何这个分数)。
    pub rationale: String,
    /// 评估基底(grounding 覆盖率 / 模型 tier / 检索质量等)。
    #[serde(default)]
    pub basis: Vec<String>,
}

/// 置信分级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceLevel {
    /// 高。
    High,
    /// 中。
    Medium,
    /// 低。
    Low,
}

/// 产出物(桥接 export::Artifact;leaf 只存元数据,不依赖 export crate)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    /// 产出物名。
    pub name: String,
    /// 类型(xlsx / docx / pdf / md / csv / table / image / ...)。
    pub kind: String,
    /// 引用(产出物 id / 相对路径 / 下载 token;不内联大内容)。
    pub reference: String,
}

/// 警告 / 红线 / 需人工复核(统一出口)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Warning {
    /// 警告类别。
    pub kind: WarningKind,
    /// 严重度。
    pub severity: Severity,
    /// 说明(可操作)。
    pub message: String,
    /// 是否必须人工复核(law 律师签字 / medical 临床确认)。
    #[serde(default)]
    pub needs_human: bool,
}

/// 警告类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningKind {
    /// 业务红线(硬约束;通常 needs_human=true)。
    RedLine,
    /// 结论不确定。
    Uncertain,
    /// 超出 agent 能力 / 授权范围。
    OutOfScope,
    /// 数据缺失。
    MissingData,
    /// 其它。
    #[serde(other)]
    Other,
}

/// 严重度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// 提示。
    Info,
    /// 警告。
    Warn,
    /// 严重(通常阻塞 / 必复核)。
    Critical,
}

/// 能力标签(本次用到的 capability;与 Capability Registry 的 id 对齐)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityTag {
    /// capability id(与 Capability Registry 一致,如 "vlm" / "web_search" / "retrieval")。
    pub id: String,
    /// 是否实际生效(false=声明可用但本次未触发)。
    #[serde(default = "default_true")]
    pub used: bool,
}

fn default_true() -> bool {
    true
}

impl EvidenceEnvelope {
    /// 最小完备性自检(leaf 侧的轻量检查;深度 grounding 真实性校验在 non-leaf pro-infra)。
    ///
    /// 返回缺失维度列表(空 = 结构完备)。**不**校验 grounding 真实性(那需读源 pack)。
    pub fn completeness_gaps(&self) -> Vec<&'static str> {
        let mut gaps = Vec::new();
        if self.inputs.hash.is_empty() {
            gaps.push("inputs.hash");
        }
        if self.confidence.rationale.trim().is_empty() {
            gaps.push("confidence.rationale");
        }
        // evidence 可为空(纯计算 agent 无外部 grounding),但若有结论性 computation 应有依据;
        // 该业务判定留 non-leaf 校验器(它知道 agent 类别)。leaf 只查结构字段非空。
        gaps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> EvidenceEnvelope {
        EvidenceEnvelope {
            envelope_version: ENVELOPE_VERSION,
            inputs: InputsSnapshot {
                hash: "abc123".into(),
                summary: "贷款利息计算(本金脱敏)".into(),
                case_kind: Some("law".into()),
                output_mode: Some("structured".into()),
            },
            steps: vec![StepRecord {
                order: 0,
                kind: StepKind::Retrieve,
                detail: "FTS 命中 3 条相关法条".into(),
            }],
            evidence: vec![EvidenceItem {
                kind: EvidenceKind::Statute,
                source_ref: "民法典:680".into(),
                offset: None,
                quoted_text: "禁止高利放贷,借款的利率不得违反国家有关规定".into(),
                authority_ref: Some("《中华人民共和国民法典》第六百八十条".into()),
                verifiable: true,
            }],
            confidence: Confidence {
                score: 0.92,
                level: ConfidenceLevel::High,
                rationale: "全部结论均有法条 grounding".into(),
                basis: vec!["grounding_coverage=1.0".into()],
            },
            artifacts: vec![],
            warnings: vec![],
            capabilities: vec![CapabilityTag {
                id: "retrieval".into(),
                used: true,
            }],
        }
    }

    #[test]
    fn roundtrip_serde() {
        let env = sample();
        let json = serde_json::to_string(&env).unwrap();
        let back: EvidenceEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(env, back);
    }

    #[test]
    fn unknown_step_and_evidence_kind_graceful() {
        // 旧消费者遇到未来新增的 kind 不应 crash(serde other 兜底)。
        let j = r#"{"order":1,"kind":"some_future_kind","detail":"x"}"#;
        let s: StepRecord = serde_json::from_str(j).unwrap();
        assert_eq!(s.kind, StepKind::Other);
        let je = r#"{"kind":"future_evkind","source_ref":"r","quoted_text":"q"}"#;
        let e: EvidenceItem = serde_json::from_str(je).unwrap();
        assert_eq!(e.kind, EvidenceKind::Other);
        assert!(!e.verifiable); // default false
    }

    #[test]
    fn missing_envelope_version_defaults() {
        // 老/精简 JSON 无 envelope_version → 默认 ENVELOPE_VERSION。
        let j = r#"{"inputs":{"hash":"h","summary":"s"},"confidence":{"score":0.5,"level":"low","rationale":"r"}}"#;
        let env: EvidenceEnvelope = serde_json::from_str(j).unwrap();
        assert_eq!(env.envelope_version, ENVELOPE_VERSION);
        assert!(env.steps.is_empty());
        assert!(env.evidence.is_empty());
    }

    #[test]
    fn completeness_gaps_flags_empty_fields() {
        let mut env = sample();
        assert!(env.completeness_gaps().is_empty());
        env.inputs.hash = String::new();
        env.confidence.rationale = "  ".into();
        let gaps = env.completeness_gaps();
        assert!(gaps.contains(&"inputs.hash"));
        assert!(gaps.contains(&"confidence.rationale"));
    }
}
