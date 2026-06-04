//! attune-agent-sdk — WASM-safe agent leaf crate.
//!
//! 承载 `Agent` trait + `AgentOutput<T>` + 一个零 native 依赖的轻量错误类型
//! (`AgentError`/`AgentResult`),使确定性 agent(纯计算,如利息/诉讼时效)可编
//! `wasm32-wasip1`,享受 v1.1.0 WASM runtime "一包通吃所有平台" 的分发能力。
//!
//! 不变量 —— 本 crate **只能** 依赖 `serde` + `thiserror`(纯 proc-macro,零运行期
//! native dep)。**禁止** 回流 `rusqlite` / `tokio` / `reqwest` / `usearch` /
//! `tantivy` / `hdbscan` / `socket2` / `serde_yaml` / `chrono` 等 native-only crate
//! —— 任一引入即破坏 wasm 可编性(`attune-core` 当年正是被这些拖进整树才编不了
//! wasm)。CI 的 `cargo build -p attune-agent-sdk --target wasm32-wasip1` 守卫此约束。
//! `serde_json`(纯 Rust,wasm-safe)于 2026-06-03 从 dev-dep 提升为正式依赖,
//! 供 `agent_main` stdio 助手使用 —— 不破坏 wasm 可编性,守卫仍绿。
//!
//! `attune-core` 反过来依赖本 crate 并 `pub use` re-export `Agent`/`AgentOutput`,
//! 保 `attune_core::agents::{Agent, AgentOutput}` 路径不变;`From<AgentError> for
//! VaultError` 在 attune-core 侧定义(方向 core→leaf 单一,无环)。

pub mod agent_main;

use serde::{Deserialize, Serialize};

/// 统一 agent 输出 schema。
///
/// JSON wire 形态(6 字段、字段名、顺序)与抽取前 `attune-core` 本地定义逐字节一致,
/// 已装 subprocess / wasm agent 的输出契约不变。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentOutput<T> {
    /// agent 业务自定义输出 (借贷 = 金额; 婚姻 = 分割比例; 分类 = 证据列表)
    pub computation: T,
    /// 可审计推理链
    pub audit_trail: String,
    /// 硬阻塞: 任一不满足业务红线 → reject
    pub red_lines_violated: Vec<String>,
    /// 软追问: 缺失证据 (不阻塞)
    pub missing_evidence: Vec<String>,
    /// 后续行动建议 (调用方提示用户)
    pub followups: Vec<String>,
    /// 整体置信度 0.0 - 1.0
    pub confidence: f64,
}

impl<T> AgentOutput<T> {
    /// 检查是否有业务红线被违反
    pub fn has_red_lines(&self) -> bool {
        !self.red_lines_violated.is_empty()
    }
    /// 检查是否需要后续追问 (软或硬)
    pub fn needs_attention(&self) -> bool {
        self.has_red_lines() || !self.missing_evidence.is_empty()
    }
}

/// Agent 统一接口. 内置 + 外部 plugin agent 都实现此 trait (内部直调) 或通过
/// capability_dispatch subprocess 走 binary 模式 (跨进程 / 跨 plugin).
///
/// `Input` 业务自定义 (分类 agent = 文档列表; 借贷 agent = 证据集); `Output` 业务自定义.
pub trait Agent {
    type Input;
    type Output;

    /// agent 唯一 id (与 plugin.yaml agents[].id 对应)
    fn id(&self) -> &str;

    /// 简短描述
    fn description(&self) -> &str;

    /// 此 agent 能处理的案件类型 (空 = 任意)
    fn case_kinds(&self) -> &[&str];

    /// 主入口: 接受输入 → 输出 AgentOutput
    fn run(&self, input: Self::Input) -> AgentResult<AgentOutput<Self::Output>>;
}

/// WASM-safe 错误类型。
///
/// 关键:**不含** `#[from] rusqlite::Error` / `serde_yaml::Error`(那是 native
/// `VaultError` 编不了 wasm 的根因)。序列化错误以 `String` 承载,不在签名里暴露
/// `serde_json::Error` 等具体类型。`#[non_exhaustive]` 允许向后兼容增变体
/// (新增变体时 attune-core 的 `From<AgentError> for VaultError` 需补 arm)。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AgentError {
    /// 对应 native `VaultError::InvalidInput`
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// 计算 / 业务内部错
    #[error("computation error: {0}")]
    Computation(String),
    /// 序列化失败的字符串化(leaf 不依赖 serde_json 具体类型,存 String)
    #[error("serialization error: {0}")]
    Serialization(String),
    /// 业务红线触发(映射 exit code 2)
    #[error("red line violated: {0}")]
    RedLine(String),
}

/// agent 结果别名(替代 native `attune_core::error::Result`)。
pub type AgentResult<T> = Result<T, AgentError>;

#[cfg(test)]
mod tests {
    use super::*;

    fn make_output<T>(comp: T, red: Vec<String>, missing: Vec<String>) -> AgentOutput<T> {
        AgentOutput {
            computation: comp,
            audit_trail: String::new(),
            red_lines_violated: red,
            missing_evidence: missing,
            followups: vec![],
            confidence: 1.0,
        }
    }

    // ---- 迁入的 8 个 AgentOutput golden 测试(抽取前 attune-core mod.rs) ----

    #[test]
    fn has_red_lines_empty_is_false() {
        let o = make_output(42, vec![], vec![]);
        assert!(!o.has_red_lines());
    }

    #[test]
    fn has_red_lines_with_one_violation_is_true() {
        let o = make_output(42, vec!["red1".into()], vec![]);
        assert!(o.has_red_lines());
    }

    #[test]
    fn needs_attention_with_red_line() {
        let o = make_output(42, vec!["red".into()], vec![]);
        assert!(o.needs_attention());
    }

    #[test]
    fn needs_attention_with_missing_evidence() {
        let o = make_output(42, vec![], vec!["missing1".into()]);
        assert!(o.needs_attention());
    }

    #[test]
    fn needs_attention_when_clean_is_false() {
        let o = make_output(42, vec![], vec![]);
        assert!(!o.needs_attention());
    }

    #[test]
    fn needs_attention_with_both() {
        let o = make_output(42, vec!["red".into()], vec!["m".into()]);
        assert!(o.needs_attention());
    }

    // serde roundtrip for AgentOutput<T>
    #[test]
    fn agent_output_serde_roundtrip() {
        let o = AgentOutput {
            computation: serde_json::json!({"result": "ok"}),
            audit_trail: "step 1\nstep 2".into(),
            red_lines_violated: vec!["red".into()],
            missing_evidence: vec!["m1".into(), "m2".into()],
            followups: vec!["follow".into()],
            confidence: 0.85,
        };
        let json = serde_json::to_string(&o).expect("ser");
        assert!(json.contains("\"confidence\":0.85"));
        let back: AgentOutput<serde_json::Value> = serde_json::from_str(&json).expect("de");
        assert_eq!(back.red_lines_violated.len(), 1);
        assert_eq!(back.missing_evidence.len(), 2);
        assert_eq!(back.confidence, 0.85);
    }

    // generic T: 验证 String / Vec / Custom struct 都能 work
    #[test]
    fn agent_output_generic_over_types() {
        let s: AgentOutput<String> = make_output("hello".into(), vec![], vec![]);
        assert_eq!(s.computation, "hello");
        let v: AgentOutput<Vec<i32>> = make_output(vec![1, 2, 3], vec![], vec![]);
        assert_eq!(v.computation.len(), 3);
    }

    // ---- JSON wire 不变断言(关键 — 防字段顺序/命名漂移破坏现有契约) ----

    #[test]
    fn agent_output_json_wire_byte_exact() {
        // 固定值序列化必须逐字节等于抽取前期望:6 字段同序、同名。serde derive 按
        // struct 声明顺序输出字段。confidence 取 0.5 避免浮点格式化歧义。
        let o = AgentOutput {
            computation: 7i32,
            audit_trail: "trail".to_string(),
            red_lines_violated: vec!["r".to_string()],
            missing_evidence: vec!["m".to_string()],
            followups: vec!["f".to_string()],
            confidence: 0.5,
        };
        let json = serde_json::to_string(&o).expect("ser");
        assert_eq!(
            json,
            r#"{"computation":7,"audit_trail":"trail","red_lines_violated":["r"],"missing_evidence":["m"],"followups":["f"],"confidence":0.5}"#
        );
    }

    // ---- 边界 case ≥5 ----

    #[test]
    fn boundary_empty_computation_string() {
        let o: AgentOutput<String> = make_output(String::new(), vec![], vec![]);
        let json = serde_json::to_string(&o).unwrap();
        let back: AgentOutput<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.computation, "");
    }

    #[test]
    fn boundary_huge_audit_trail() {
        let big = "x".repeat(100_000);
        let mut o: AgentOutput<i32> = make_output(0, vec![], vec![]);
        o.audit_trail = big.clone();
        let json = serde_json::to_string(&o).unwrap();
        let back: AgentOutput<i32> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.audit_trail.len(), big.len());
    }

    #[test]
    fn boundary_confidence_extremes() {
        for c in [0.0f64, 1.0f64] {
            let mut o: AgentOutput<i32> = make_output(0, vec![], vec![]);
            o.confidence = c;
            let json = serde_json::to_string(&o).unwrap();
            let back: AgentOutput<i32> = serde_json::from_str(&json).unwrap();
            assert_eq!(back.confidence, c);
        }
    }

    #[test]
    fn boundary_empty_and_full_red_lines() {
        let empty: AgentOutput<i32> = make_output(0, vec![], vec![]);
        assert!(!empty.has_red_lines());
        let full: AgentOutput<i32> =
            make_output(0, vec!["a".into(), "b".into(), "c".into()], vec![]);
        assert!(full.has_red_lines());
        assert_eq!(full.red_lines_violated.len(), 3);
    }

    #[test]
    fn boundary_unicode_audit_trail() {
        let mut o: AgentOutput<i32> = make_output(0, vec![], vec![]);
        o.audit_trail = "中文推理链 🔒 步骤一→步骤二".into();
        let json = serde_json::to_string(&o).unwrap();
        let back: AgentOutput<i32> = serde_json::from_str(&json).unwrap();
        assert!(back.audit_trail.contains("中文推理链"));
        assert!(back.audit_trail.contains("🔒"));
    }

    // ---- 异常 / 错误 ≥3 ----

    #[test]
    fn agent_error_variants_display() {
        assert_eq!(
            AgentError::InvalidInput("empty".into()).to_string(),
            "invalid input: empty"
        );
        assert_eq!(
            AgentError::Computation("overflow".into()).to_string(),
            "computation error: overflow"
        );
        assert_eq!(
            AgentError::Serialization("bad json".into()).to_string(),
            "serialization error: bad json"
        );
        assert_eq!(
            AgentError::RedLine("usury cap".into()).to_string(),
            "red line violated: usury cap"
        );
    }

    #[test]
    fn agent_error_is_std_error() {
        // AgentError 实现 std::error::Error(thiserror derive),可走 ? / Box<dyn Error>。
        fn assert_error<E: std::error::Error>(_: &E) {}
        assert_error(&AgentError::Computation("x".into()));
    }

    #[test]
    fn agent_result_propagates_via_question_mark() {
        fn inner() -> AgentResult<i32> {
            Err(AgentError::RedLine("blocked".into()))
        }
        fn outer() -> AgentResult<i32> {
            let v = inner()?;
            Ok(v + 1)
        }
        assert!(matches!(outer(), Err(AgentError::RedLine(_))));
    }

    // ---- 属性测试 ≥3 ----

    proptest::proptest! {
        // P1: AgentOutput<i64> serde roundtrip 任意字段值
        #[test]
        fn prop_agent_output_serde_roundtrip(
            comp in proptest::prelude::any::<i64>(),
            audit in ".*",
            red in proptest::collection::vec("\\PC*", 0..5),
            missing in proptest::collection::vec("\\PC*", 0..5),
            conf in 0.0f64..=1.0f64,
        ) {
            let o = AgentOutput {
                computation: comp,
                audit_trail: audit,
                red_lines_violated: red.clone(),
                missing_evidence: missing.clone(),
                followups: vec![],
                confidence: conf,
            };
            let json = serde_json::to_string(&o).unwrap();
            let back: AgentOutput<i64> = serde_json::from_str(&json).unwrap();
            proptest::prop_assert_eq!(back.computation, comp);
            proptest::prop_assert_eq!(back.red_lines_violated.len(), red.len());
            proptest::prop_assert_eq!(back.missing_evidence.len(), missing.len());
        }

        // P2: needs_attention 不变量 — red_lines 非空 ⟹ needs_attention 必 true
        #[test]
        fn prop_needs_attention_invariant(
            red in proptest::collection::vec("\\PC*", 0..5),
            missing in proptest::collection::vec("\\PC*", 0..5),
        ) {
            let o: AgentOutput<i32> = make_output(0, red.clone(), missing.clone());
            let expected = !red.is_empty() || !missing.is_empty();
            proptest::prop_assert_eq!(o.needs_attention(), expected);
            if !red.is_empty() {
                proptest::prop_assert!(o.needs_attention());
                proptest::prop_assert!(o.has_red_lines());
            }
        }

        // P3: 任意 confidence f64(含极端值)序列化不 panic
        #[test]
        fn prop_confidence_any_finite_no_panic(conf in proptest::prelude::any::<f64>()) {
            let mut o: AgentOutput<i32> = make_output(0, vec![], vec![]);
            o.confidence = conf;
            // 非有限 f64(NaN/Inf)serde_json 会 Err 而非 panic — 两种结果都可接受,只要不 panic。
            let _ = serde_json::to_string(&o);
        }
    }
}
