//! Skill 评估 framework
//!
//! 给 skill 注册一组 (input, expected) 用例 + runner 闭包，跑出
//! pass/fail + 延迟统计报告。用于回归测试任何 skill 实现，包括
//! attune-pro 行业 skill 的本地 eval、CI 卡点等。
//!
//! 设计原则：
//! - 输入/输出都是 `serde_json::Value`，与 skill 调用协议天然对齐
//! - runner 闭包独立于框架，便于 mock / 真 LLM / RPC 等灵活替换
//! - 异步：runner 返回 Future，框架内部按用例顺序串行执行（避免共享 LLM provider 时的速率/状态干扰）

use crate::error::Result;
use std::future::Future;

/// 单条评估用例
#[derive(Debug, Clone)]
pub struct SkillEvalCase {
    /// runner 接收的输入参数（约定 skill input schema）
    pub input: serde_json::Value,
    /// 期望的输出（与 runner 返回值做深度等价比较）
    pub expected: serde_json::Value,
    /// 单 case 允许的最大延迟（毫秒）；超过判失败
    pub max_latency_ms: u64,
}

/// 评估结果
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillEvalReport {
    pub skill_id: String,
    pub cases_total: usize,
    pub cases_passed: usize,
    /// 所有 case 平均延迟（毫秒）
    pub avg_latency_ms: f64,
    /// 单 case 实测最大延迟（毫秒）
    pub max_latency_ms: u64,
    /// 失败描述（"case #i: expected ..., got ..."）
    pub failures: Vec<String>,
}

/// 跑一组评估用例。
///
/// `runner` 是一个闭包：拿输入 `serde_json::Value` → 异步返回 `Result<serde_json::Value>`。
/// 比较 actual vs expected 用 `serde_json::Value` 的 `PartialEq`（深度等价）。
///
/// 任何 runner 返回 `Err`、或输出不等、或延迟超限，都判失败。
pub async fn evaluate_skill<F, Fut>(
    skill_id: &str,
    cases: &[SkillEvalCase],
    runner: F,
) -> SkillEvalReport
where
    F: Fn(serde_json::Value) -> Fut,
    Fut: Future<Output = Result<serde_json::Value>>,
{
    let mut failures: Vec<String> = Vec::new();
    let mut passed = 0usize;
    let mut total_latency_ms: u128 = 0;
    let mut observed_max_ms: u64 = 0;

    for (i, case) in cases.iter().enumerate() {
        let started = std::time::Instant::now();
        let res = runner(case.input.clone()).await;
        let elapsed_ms = started.elapsed().as_millis();
        total_latency_ms += elapsed_ms;
        let elapsed_u64 = elapsed_ms as u64;
        if elapsed_u64 > observed_max_ms {
            observed_max_ms = elapsed_u64;
        }

        match res {
            Err(e) => {
                failures.push(format!("case #{}: runner error: {}", i, e));
            }
            Ok(actual) => {
                if elapsed_u64 > case.max_latency_ms {
                    failures.push(format!(
                        "case #{}: latency {} ms exceeds limit {} ms",
                        i, elapsed_u64, case.max_latency_ms
                    ));
                } else if actual != case.expected {
                    failures.push(format!(
                        "case #{}: expected {}, got {}",
                        i, case.expected, actual
                    ));
                } else {
                    passed += 1;
                }
            }
        }
    }

    let avg_latency_ms = if cases.is_empty() {
        0.0
    } else {
        total_latency_ms as f64 / cases.len() as f64
    };

    SkillEvalReport {
        skill_id: skill_id.to_string(),
        cases_total: cases.len(),
        cases_passed: passed,
        avg_latency_ms,
        max_latency_ms: observed_max_ms,
        failures,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn all_pass() {
        let cases = vec![
            SkillEvalCase {
                input: json!({"x": 1}),
                expected: json!({"y": 2}),
                max_latency_ms: 1000,
            },
            SkillEvalCase {
                input: json!({"x": 3}),
                expected: json!({"y": 6}),
                max_latency_ms: 1000,
            },
        ];
        let report = evaluate_skill("double", &cases, |input| async move {
            let x = input["x"].as_i64().unwrap();
            Ok(json!({"y": x * 2}))
        })
        .await;
        assert_eq!(report.skill_id, "double");
        assert_eq!(report.cases_total, 2);
        assert_eq!(report.cases_passed, 2);
        assert!(report.failures.is_empty());
    }

    #[tokio::test]
    async fn partial_fail() {
        let cases = vec![
            SkillEvalCase {
                input: json!({"x": 1}),
                expected: json!({"y": 2}),
                max_latency_ms: 1000,
            },
            SkillEvalCase {
                // 错误期望，应失败
                input: json!({"x": 3}),
                expected: json!({"y": 999}),
                max_latency_ms: 1000,
            },
        ];
        let report = evaluate_skill("double", &cases, |input| async move {
            let x = input["x"].as_i64().unwrap();
            Ok(json!({"y": x * 2}))
        })
        .await;
        assert_eq!(report.cases_total, 2);
        assert_eq!(report.cases_passed, 1);
        assert_eq!(report.failures.len(), 1);
        assert!(report.failures[0].contains("case #1"));
    }

    #[tokio::test]
    async fn latency_exceed() {
        let cases = vec![SkillEvalCase {
            input: json!({}),
            expected: json!({"ok": true}),
            // 设个极小限制保证超
            max_latency_ms: 0,
        }];
        let report = evaluate_skill("slow", &cases, |_| async move {
            // 故意 sleep 让超时触发
            tokio::time::sleep(std::time::Duration::from_millis(15)).await;
            Ok(json!({"ok": true}))
        })
        .await;
        assert_eq!(report.cases_total, 1);
        assert_eq!(report.cases_passed, 0);
        assert_eq!(report.failures.len(), 1);
        assert!(report.failures[0].contains("latency"));
    }
}
