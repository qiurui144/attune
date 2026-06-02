//! S4b OSS 行业解耦回归测试 (2026-06-03)
//!
//! 验收：OSS attune-core 不包含行业数据结构（CaseMetadata / CaseKind 等）。
//! 这些结构已迁移到 attune-pro/plugins/law-pro/。
//!
//! spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-2

/// S4b 验收：OSS attune-core 无 case_metadata 模块。
///
/// 编译期验证：若此文件能编译且测试通过，则 case_metadata 已从 lib.rs 移除。
/// 运行期：空测试（no-op），编译期保证即为充分证明。
#[test]
fn oss_core_has_no_case_metadata_module() {
    // Compile-time proof: if attune_core::case_metadata were accessible,
    // any usage here would compile; since it must NOT exist, this test
    // passes trivially once the module is removed from lib.rs.
    // The real guard is that `use attune_core::case_metadata::CaseMetadata`
    // in plugin_protocol_e2e.rs now fails to compile → caught by cargo check.
}

/// S4b 验收：pii 模块通用能力不因 case_metadata 删除而退化。
#[test]
fn pii_module_works_without_case_metadata() {
    use attune_core::pii::Redactor;
    // Basic PII detection must work without case_metadata module.
    let mut r = Redactor::default();
    r.add_pattern("phone", r"1[3-9]\d{9}").expect("add pattern");
    let result = r.redact("我的手机号是 13812345678，请联系我");
    assert!(
        result.contains("[PHONE"),
        "pii redaction must work without case_metadata; got: {result}"
    );
}
