//! 通用 plugin 协议层覆盖测试 — 不绑业务领域.
//!
//! 覆盖 6 个通用 plugin 类型作为 plugin protocol 的端到端 fixture:
//! - annotation_angle (ai_annotation_highlights / ai_annotation_risk)
//! - skill (rust_helper)
//! - industry (presales-pro / patent-pro / tech-pro)
//!
//! 与 law-pro 测试的区别:
//! - 通用测试: 验证 plugin_registry / plugin_loader / 装载 / 查询 / chat_trigger 协议层
//! - law-pro 测试: 验证 civil_loan_agent / interest_calculator 业务红线 + audit_trail
//!
//! 跑法: cargo test -p attune-core --test generic_plugins_test
//!
//! 这些 plugin 不依赖外部仓 — 用本测试创建临时 plugin dir, 不依赖 ~/.local/share 已装.

use attune_core::plugin_loader::{LoadedPlugin, PluginManifest};
use attune_core::plugin_registry::PluginRegistry;
use std::fs;
use tempfile::TempDir;

fn write_plugin(root: &std::path::Path, dir_name: &str, yaml: &str) {
    let p = root.join(dir_name);
    fs::create_dir_all(&p).expect("mkdir");
    fs::write(p.join("plugin.yaml"), yaml).expect("write");
}

const ANNOTATION_RISK: &str = r##"
id: ai_annotation_risk
name: AI 风险批注
type: annotation_angle
version: "1.0.0"
description: 文档中风险点的批注角度
label_prefix: "[风险]"
default_color: "#d32f2f"
"##;

const ANNOTATION_HIGHLIGHTS: &str = r##"
id: ai_annotation_highlights
name: AI 高亮批注
type: annotation_angle
version: "1.0.0"
label_prefix: "[要点]"
default_color: "#1976d2"
"##;

const RUST_HELPER: &str = r##"
id: rust_helper
name: Rust 编程助手
type: skill
version: "1.0.0"
chat_trigger:
  enabled: true
  priority: 3
  keywords: ["Rust", "cargo", "trait", "borrow checker"]
  min_keyword_match: 1
  description: "Rust 编程相关查询触发"
"##;

const PRESALES_PRO: &str = r##"
id: presales_pro
name: 售前 Pro
type: industry
version: "0.1.0"
chat_trigger:
  enabled: true
  priority: 5
  keywords: ["报价", "方案", "客户"]
  min_keyword_match: 1
  description: "售前业务查询"
  project_keywords: ["客户", "项目", "方案"]
"##;

const PATENT_PRO: &str = r##"
id: patent_pro
name: 专利 Pro
type: industry
version: "0.1.0"
chat_trigger:
  enabled: true
  priority: 5
  keywords: ["专利", "权利要求", "申请"]
  min_keyword_match: 1
  description: "专利业务触发"
"##;

const TECH_PRO: &str = r##"
id: tech_pro
name: 技术 Pro
type: industry
version: "0.1.0"
chat_trigger:
  enabled: true
  priority: 4
  keywords: ["架构", "性能", "测试"]
  min_keyword_match: 1
"##;

fn write_all_generic(root: &std::path::Path) {
    write_plugin(root, "ai_annotation_risk", ANNOTATION_RISK);
    write_plugin(root, "ai_annotation_highlights", ANNOTATION_HIGHLIGHTS);
    write_plugin(root, "rust_helper", RUST_HELPER);
    write_plugin(root, "presales-pro", PRESALES_PRO);
    write_plugin(root, "patent-pro", PATENT_PRO);
    write_plugin(root, "tech-pro", TECH_PRO);
}

// ── 协议层基础覆盖 ──────────────────────────────────

#[test]
fn registry_scans_all_6_generic_plugins() {
    let tmp = TempDir::new().expect("tmp");
    write_all_generic(tmp.path());
    let (reg, errs) = PluginRegistry::scan(tmp.path()).expect("scan");
    assert!(errs.is_empty(), "scan errors: {errs:?}");
    assert_eq!(reg.plugins().count(), 6);
}

#[test]
fn each_plugin_has_required_id_type_version() {
    let yamls = [
        ANNOTATION_RISK,
        ANNOTATION_HIGHLIGHTS,
        RUST_HELPER,
        PRESALES_PRO,
        PATENT_PRO,
        TECH_PRO,
    ];
    for yaml in yamls {
        let m: PluginManifest = serde_yaml::from_str(yaml).expect("parse");
        assert!(!m.id.is_empty());
        assert!(!m.plugin_type.is_empty());
        assert!(!m.version.is_empty());
    }
}

// ── annotation_angle 类型专项 ──────────────────────

#[test]
fn annotation_angle_plugins_have_label_prefix_and_color() {
    let tmp = TempDir::new().expect("tmp");
    write_plugin(tmp.path(), "ann_risk", ANNOTATION_RISK);
    write_plugin(tmp.path(), "ann_high", ANNOTATION_HIGHLIGHTS);
    let (reg, _) = PluginRegistry::scan(tmp.path()).expect("scan");

    for p in reg.plugins() {
        if p.manifest.plugin_type == "annotation_angle" {
            assert!(!p.manifest.label_prefix.is_empty(), "label_prefix missing for {}", p.manifest.id);
            assert!(!p.manifest.default_color.is_empty(), "default_color missing");
        }
    }
}

// ── skill 类型 + chat_trigger ──────────────────────

#[test]
fn rust_helper_matches_chat_keywords() {
    let tmp = TempDir::new().expect("tmp");
    write_plugin(tmp.path(), "rust_helper", RUST_HELPER);
    let (reg, _) = PluginRegistry::scan(tmp.path()).expect("scan");

    let m = reg.match_chat_trigger("如何用 Rust 实现 trait 多态?").expect("match");
    assert_eq!(m.plugin_id, "rust_helper");
    assert!(m.keyword_hits >= 2); // Rust + trait
}

#[test]
fn chat_trigger_no_keywords_no_match() {
    let tmp = TempDir::new().expect("tmp");
    write_plugin(tmp.path(), "rust_helper", RUST_HELPER);
    let (reg, _) = PluginRegistry::scan(tmp.path()).expect("scan");

    assert!(reg.match_chat_trigger("今天天气怎样").is_none());
}

// ── industry 类型 (presales / patent / tech) ──────

#[test]
fn industry_plugins_chat_trigger_priorities() {
    let tmp = TempDir::new().expect("tmp");
    write_all_generic(tmp.path());
    let (reg, _) = PluginRegistry::scan(tmp.path()).expect("scan");

    // "方案" 仅 presales 命中 (presales priority=5)
    let m = reg.match_chat_trigger("帮我写方案").expect("match");
    assert_eq!(m.plugin_id, "presales_pro");

    // "架构" 仅 tech 命中
    let m = reg.match_chat_trigger("讨论一下架构").expect("match");
    assert_eq!(m.plugin_id, "tech_pro");

    // "专利申请" 仅 patent 命中
    let m = reg.match_chat_trigger("帮我看专利申请").expect("match");
    assert_eq!(m.plugin_id, "patent_pro");
}

#[test]
fn project_recommender_keywords_aggregated() {
    let tmp = TempDir::new().expect("tmp");
    write_all_generic(tmp.path());
    let (reg, _) = PluginRegistry::scan(tmp.path()).expect("scan");

    let kws = reg.all_chat_trigger_project_keywords();
    // presales 提供 "客户" "项目" "方案"
    assert!(kws.contains(&"客户"));
    assert!(kws.contains(&"项目"));
    assert!(kws.contains(&"方案"));
}

// ── chat_trigger 优先级 ────────────────────────────

#[test]
fn higher_priority_wins_on_overlap() {
    let high = r##"
id: high_pri
type: industry
version: "1.0.0"
name: high
chat_trigger:
  enabled: true
  priority: 100
  keywords: ["客户"]
  min_keyword_match: 1
"##;
    let tmp = TempDir::new().expect("tmp");
    write_plugin(tmp.path(), "high", high);
    write_plugin(tmp.path(), "presales-pro", PRESALES_PRO);
    let (reg, _) = PluginRegistry::scan(tmp.path()).expect("scan");

    // 都命中 "客户", 但 high_pri priority=100 > presales priority=5
    let m = reg.match_chat_trigger("跟客户聊").expect("match");
    assert_eq!(m.plugin_id, "high_pri");
}

// ── 隔离: 通用 plugin 不互相干扰 ────────────────────

#[test]
fn empty_oss_distribution_has_no_plugins() {
    let tmp = TempDir::new().expect("tmp");
    let (reg, errs) = PluginRegistry::scan(tmp.path()).expect("scan");
    assert!(errs.is_empty());
    assert_eq!(reg.plugins().count(), 0);
    assert!(reg.list_skills().is_empty());
    assert!(reg.list_agents().is_empty());
    assert!(reg.list_mcp_servers().is_empty());
    assert!(reg.all_registered_case_kinds().is_empty());
}

// ── 边界: 损坏 yaml 不影响其他 plugin ────────────────

#[test]
fn corrupt_yaml_logged_but_others_still_load() {
    let tmp = TempDir::new().expect("tmp");
    write_plugin(tmp.path(), "rust_helper", RUST_HELPER);
    write_plugin(tmp.path(), "corrupt", "id: corrupt\ntype:\n  - this\n  is: invalid_yaml::");
    let (reg, errs) = PluginRegistry::scan(tmp.path()).expect("scan");
    // corrupt 进 errs, rust_helper 仍装
    assert_eq!(reg.plugins().count(), 1);
    assert_eq!(reg.plugins().next().unwrap().manifest.id, "rust_helper");
    assert!(!errs.is_empty(), "corrupt yaml should produce error");
}
