//! WASM capability gate — 跨平台 agent 分发 golden + 边界 + 错误 + E2E。
//!
//! 验证 reference wasm fixture(`tests/fixtures/wasm/echo_calc_agent.wasm`)经
//! attune-core WasmRunner 执行,输出与 **独立计算的 native 基线逐字节一致**
//! (per spec §9 golden diff=0 + Agent 验证铁律 ground truth 独立)。
//!
//! 六类下限覆盖:golden(≥10)/ proptest(≥3)/ 边界(≥5)/ 错误(≥3)/ E2E(≥1)。
//! 整个文件 feature-gated:无 wasm-runtime feature 时为空(`--no-default-features` 编译过)。
#![cfg(feature = "wasm-runtime")]

use attune_core::capability_dispatch::{
    dispatch_capability, CapabilityInvocation, CapabilityRuntime,
};
use std::path::PathBuf;
use std::time::Duration;

fn fixture_wasm() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wasm/echo_calc_agent.wasm")
}

/// 独立 native ground truth — **不调 wasm**,直接按契约算期望 stdout(spec §9)。
/// 与 fixture 源逻辑独立实现(整数运算),用于 golden diff=0 对照。
fn native_expected(a: i64, b: i64, op: &str) -> String {
    let value: i64 = match op {
        "add" => a.wrapping_add(b),
        "sub" => a.wrapping_sub(b),
        "mul" => a.wrapping_mul(b),
        _ => panic!("unsupported op in GT"),
    };
    format!(
        r#"{{"ok":true,"result":{{"value":{value}}},"audit_trail":["op={op}"],"red_lines_violated":[]}}"#
    )
}

fn run_wasm(stdin: &str, timeout_ms: u64) -> attune_core::capability_dispatch::CapabilityResult {
    let inv = CapabilityInvocation::new(fixture_wasm())
        .stdin(stdin)
        .timeout(Duration::from_millis(timeout_ms));
    dispatch_capability(CapabilityRuntime::Wasm, &inv).expect("wasm dispatch")
}

// ─────────────────────── golden: diff=0 vs native baseline (≥10) ───────────────────────

#[test]
fn golden_wasm_output_byte_equal_native_baseline() {
    // 10 真实 case + 1 sentinel(故意不同,验证测试本身能抓差异)
    let cases: &[(i64, i64, &str)] = &[
        (1, 2, "add"),
        (100, 200, "add"),
        (0, 0, "add"),
        (-5, 3, "add"),
        (10, 4, "sub"),
        (3, 9, "sub"),
        (-7, -7, "sub"),
        (6, 7, "mul"),
        (12, 0, "mul"),
        (-3, 4, "mul"),
    ];
    for (a, b, op) in cases {
        let stdin = format!(r#"{{"a":{a},"b":{b},"op":"{op}"}}"#);
        let r = run_wasm(&stdin, 5000);
        assert_eq!(r.exit_code, 0, "case ({a},{b},{op}) exit");
        let expected = native_expected(*a, *b, op);
        assert_eq!(
            r.stdout.trim(),
            expected,
            "GOLDEN DIFF != 0 for ({a},{b},{op}): wasm={:?} native={:?}",
            r.stdout.trim(),
            expected
        );
    }

    // sentinel: 1 个 case 故意用错误 GT,断言它确实不等(证明 diff 检测有效)
    let stdin = r#"{"a":1,"b":1,"op":"add"}"#;
    let r = run_wasm(stdin, 5000);
    let wrong_gt = native_expected(1, 1, "mul"); // value=1 vs add value=2
    assert_ne!(r.stdout.trim(), wrong_gt, "sentinel: diff check must catch mismatch");
}

// ─────────────────────── 错误/异常 (≥3) ───────────────────────

#[test]
fn error_redline_exits_2_with_violation() {
    let r = run_wasm(r#"{"op":"redline"}"#, 5000);
    assert_eq!(r.exit_code, 2, "redline → exit 2");
    assert!(r.is_red_line());
    assert!(r.stdout.contains("red_lines_violated"));
}

#[test]
fn error_trap_maps_to_exit_1() {
    let r = run_wasm(r#"{"op":"trap"}"#, 5000);
    assert_eq!(r.exit_code, 1, "trap → exit 1");
    assert!(!r.timed_out);
    assert!(r.stderr.contains("wasm-trap") || !r.stderr.is_empty());
}

#[test]
fn error_bad_input_exits_1() {
    // 缺 a/b 的合法 op
    let r = run_wasm(r#"{"op":"add"}"#, 5000);
    assert_eq!(r.exit_code, 1, "bad-input → exit 1");
}

#[test]
fn error_unknown_op_exits_1() {
    let r = run_wasm(r#"{"op":"frobnicate"}"#, 5000);
    assert_eq!(r.exit_code, 1);
}

// ─────────────────────── 边界 (≥5) ───────────────────────

#[test]
fn boundary_empty_stdin() {
    // 空 stdin → 无 op → bad-input exit 1(不 panic 宿主)
    let r = run_wasm("", 5000);
    assert_eq!(r.exit_code, 1);
}

#[test]
fn boundary_large_stdin_does_not_crash_host() {
    // 10MB stdin padding + 合法字段在尾部
    let mut big = String::with_capacity(10 * 1024 * 1024 + 64);
    big.push_str(r#"{"pad":""#);
    big.push_str(&"x".repeat(10 * 1024 * 1024));
    big.push_str(r#"","a":2,"b":3,"op":"add"}"#);
    let r = run_wasm(&big, 10_000);
    // 仍应正确解析尾部字段
    assert_eq!(r.exit_code, 0, "stderr={}", r.stderr);
    assert!(r.stdout.contains("\"value\":5"));
}

#[test]
fn boundary_timeout_kills_infinite_loop() {
    // 死循环 wasm + 短 timeout → epoch interrupt → timed_out, exit -1
    let r = run_wasm(r#"{"op":"loop"}"#, 300);
    assert!(r.timed_out, "loop must time out; got {r:?}");
    assert_eq!(r.exit_code, -1);
}

#[test]
fn boundary_missing_wasm_file_is_error() {
    let inv = CapabilityInvocation::new("/nonexistent/path/missing.wasm")
        .stdin("{}")
        .timeout(Duration::from_millis(1000));
    let err = dispatch_capability(CapabilityRuntime::Wasm, &inv).unwrap_err();
    assert!(err.to_string().contains("not found"), "got {err}");
}

#[test]
fn boundary_invalid_wasm_module_is_error() {
    // 写个非法 .wasm(随便字节)→ wasm-module-invalid
    let tmp = tempfile::TempDir::new().unwrap();
    let bad = tmp.path().join("bad.wasm");
    std::fs::write(&bad, b"this is not wasm").unwrap();
    let inv = CapabilityInvocation::new(&bad)
        .stdin("{}")
        .timeout(Duration::from_millis(1000));
    let err = dispatch_capability(CapabilityRuntime::Wasm, &inv).unwrap_err();
    assert!(err.to_string().contains("wasm-module-invalid"), "got {err}");
}

// ─────────────────────── 集成 E2E: scan + dispatch via plugin (≥1) ───────────────────────

#[test]
fn e2e_plugin_with_wasm_agent_scans_and_dispatches() {
    use attune_core::plugin_registry::PluginRegistry;

    let tmp = tempfile::TempDir::new().unwrap();
    let plugin_dir = tmp.path().join("calc-plugin");
    std::fs::create_dir_all(plugin_dir.join("wasm")).unwrap();
    // 拷贝 fixture wasm 进 plugin dir
    std::fs::copy(fixture_wasm(), plugin_dir.join("wasm/echo_calc_agent.wasm")).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.yaml"),
        r#"
id: calc-plugin
name: Calc Plugin
type: industry
version: "1.0.0"
min_attune_version: "0.0.1"
agents:
  - id: echo_calc_agent
    runtime: wasm
    wasm: wasm/echo_calc_agent.wasm
    wasi_caps: ["stdio"]
"#,
    )
    .unwrap();

    // scan: version gate 放行 + agent 解析
    let (reg, warnings) = PluginRegistry::scan(tmp.path()).expect("scan");
    assert!(
        warnings.is_empty(),
        "no incompatible warning expected: {warnings:?}"
    );
    assert!(reg.get_plugin("calc-plugin").is_some());

    // dispatch via agent_runner (统一入口,wasm lane)
    let r = attune_core::agent_runner::run_agent_subprocess(
        &reg,
        "echo_calc_agent",
        &plugin_dir,
        r#"{"a":40,"b":2,"op":"add"}"#,
        vec![],
        Duration::from_millis(5000),
    )
    .expect("run agent");
    assert_eq!(r.exit_code, 0, "stderr={}", r.stderr);
    assert!(r.stdout.contains("\"value\":42"), "stdout={}", r.stdout);
}

#[test]
fn e2e_plugin_requiring_future_version_is_rejected_by_scan() {
    use attune_core::plugin_registry::PluginRegistry;

    let tmp = tempfile::TempDir::new().unwrap();
    let plugin_dir = tmp.path().join("future-calc");
    std::fs::create_dir_all(plugin_dir.join("wasm")).unwrap();
    std::fs::copy(fixture_wasm(), plugin_dir.join("wasm/echo_calc_agent.wasm")).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.yaml"),
        r#"
id: future-calc
name: Future Calc
type: industry
version: "1.0.0"
min_attune_version: "99.0.0"
agents:
  - id: echo_calc_agent
    runtime: wasm
    wasm: wasm/echo_calc_agent.wasm
"#,
    )
    .unwrap();

    let (reg, warnings) = PluginRegistry::scan(tmp.path()).expect("scan");
    assert!(reg.get_plugin("future-calc").is_none(), "must be skipped");
    assert!(
        warnings.iter().any(|w| w.starts_with("[incompatible]")),
        "expected incompatible warning: {warnings:?}"
    );
}

// ─────────────────────── proptest (≥3): 随机合法输入 wasm==native ───────────────────────

mod props {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(40))]

        #[test]
        fn prop_add_matches_native(a in -1_000_000i64..1_000_000, b in -1_000_000i64..1_000_000) {
            let stdin = format!(r#"{{"a":{a},"b":{b},"op":"add"}}"#);
            let r = run_wasm(&stdin, 5000);
            prop_assert_eq!(r.exit_code, 0);
            prop_assert_eq!(r.stdout.trim(), native_expected(a, b, "add"));
        }

        #[test]
        fn prop_mul_matches_native(a in -100_000i64..100_000, b in -100_000i64..100_000) {
            let stdin = format!(r#"{{"a":{a},"b":{b},"op":"mul"}}"#);
            let r = run_wasm(&stdin, 5000);
            prop_assert_eq!(r.exit_code, 0);
            prop_assert_eq!(r.stdout.trim(), native_expected(a, b, "mul"));
        }

        #[test]
        fn prop_exit_code_always_in_contract(a in any::<i64>(), b in any::<i64>()) {
            // 任意 i64(含溢出边界)→ wrapping 算,exit ∈ {0,1,2,-1},不 panic 宿主
            let stdin = format!(r#"{{"a":{a},"b":{b},"op":"sub"}}"#);
            let r = run_wasm(&stdin, 5000);
            prop_assert!(matches!(r.exit_code, -1..=2), "exit={}", r.exit_code);
        }
    }
}
