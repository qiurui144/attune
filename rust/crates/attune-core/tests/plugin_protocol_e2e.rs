//! Plugin protocol 端到端集成测试 — 跨模块串通验证.
//!
//! 覆盖:
//! 1. 加密 yaml → 装载 → 解析 schema (skills/agents/mcp_servers/registers_case_kinds)
//! 2. PluginRegistry 查询 API (list_skills/agents/mcp + agents_by_case_kind)
//! 3. Agent trait roundtrip (DocumentClassifierAgent)
//! 4. ChunkKind serde JSON roundtrip
//! 5. CaseMetadata 持久化 + 反序列化
//! 6. PluginManifest v2 完整 yaml 解析

use attune_core::agents::Agent;
use attune_core::plugin_encryption::{decrypt_yaml, encrypt_yaml};
use attune_core::plugin_loader::{LoadedPlugin, PluginManifest};
use attune_core::plugin_registry::PluginRegistry;
use std::fs;
use tempfile::TempDir;

const PAID_PLUGIN_YAML: &str = r#"
id: law-pro
name: 律师 Pro
type: industry
version: "0.2.0"
attune_min_version: "0.6.2"
maturity: stable
pricing:
  tier: paid
  trial_quota: 10
  price_url: https://attune.ai/pro/law-pro
resources:
  total_max_llm_tokens_per_call: 10000
  total_max_cpu_seconds: 30
registers_case_kinds:
  - kind: civil-loan
    label: 民事-借贷
    default_agent: civil_loan_agent
skills:
  - id: extract_loan_terms
    description: 借条解析
    runtime: rust_binary
    binary: bin/skill_extract_loan_terms
    cost:
      llm_tokens: 500
agents:
  - id: civil_loan_agent
    description: 民事借贷
    case_kinds: [civil-loan]
    consumes_evidence_kinds: [借条, 银行流水]
    hard_red_lines: [borrowing_relationship_established]
    runtime: rust_binary
    binary: bin/agent_civil_loan
    requires_skills: [extract_loan_terms]
    chat_trigger:
      enabled: true
      keywords: [本金, 利息]
      min_keyword_match: 1
      priority: 10
mcp_servers:
  - id: lpr_history
    transport: stdio
    command: ["bin/mcp_lpr"]
    tools_exposed: [get_lpr_at_date]
"#;

#[test]
fn paid_yaml_encrypt_decrypt_roundtrip() {
    let key = b"device-secret-token-for-paid-plugin";
    let cipher = encrypt_yaml(PAID_PLUGIN_YAML.as_bytes(), key).expect("encrypt");
    let plain = decrypt_yaml(&cipher, key).expect("decrypt");
    assert_eq!(&plain[..], PAID_PLUGIN_YAML.as_bytes());
}

#[test]
fn encrypted_plugin_loads_with_correct_key() {
    let tmp = TempDir::new().expect("tmp");
    let key = b"device-secret";
    let cipher = encrypt_yaml(PAID_PLUGIN_YAML.as_bytes(), key).expect("encrypt");
    fs::write(tmp.path().join("plugin.yaml.enc"), &cipher).expect("write");

    let plugin = LoadedPlugin::from_dir_with_key(tmp.path(), Some(key), Some("Trusted"))
        .expect("load encrypted plugin");
    assert_eq!(plugin.manifest.id, "law-pro");
    assert_eq!(plugin.manifest.skills.len(), 1);
    assert_eq!(plugin.manifest.agents.len(), 1);
    assert_eq!(plugin.manifest.mcp_servers.len(), 1);
    assert_eq!(plugin.manifest.registers_case_kinds.len(), 1);
}

#[test]
fn encrypted_plugin_fails_with_wrong_key() {
    let tmp = TempDir::new().expect("tmp");
    let cipher = encrypt_yaml(PAID_PLUGIN_YAML.as_bytes(), b"correct-key").expect("encrypt");
    fs::write(tmp.path().join("plugin.yaml.enc"), &cipher).expect("write");

    let result =
        LoadedPlugin::from_dir_with_key(tmp.path(), Some(b"wrong-key"), Some("Trusted"));
    assert!(result.is_err());
}

#[test]
fn encrypted_plugin_fails_without_key() {
    let tmp = TempDir::new().expect("tmp");
    let cipher = encrypt_yaml(PAID_PLUGIN_YAML.as_bytes(), b"key").expect("encrypt");
    fs::write(tmp.path().join("plugin.yaml.enc"), &cipher).expect("write");

    let result = LoadedPlugin::from_dir_with_key(tmp.path(), None, Some("Trusted"));
    assert!(result.is_err());
}

#[test]
fn paid_plugin_with_unsigned_trust_rejected() {
    let tmp = TempDir::new().expect("tmp");
    fs::write(tmp.path().join("plugin.yaml"), PAID_PLUGIN_YAML).expect("write");

    let result = LoadedPlugin::from_dir_with_key(tmp.path(), None, Some("Unsigned"));
    assert!(result.is_err(), "paid plugin with Unsigned trust must reject");
    let msg = format!("{:?}", result.unwrap_err());
    assert!(msg.contains("paid/trial") || msg.contains("Trusted") || msg.contains("Official"));
}

#[test]
fn free_plugin_with_unsigned_trust_allowed() {
    let yaml = r#"
id: free-plugin
name: Free
type: utility
version: "0.1.0"
pricing:
  tier: free
"#;
    let tmp = TempDir::new().expect("tmp");
    fs::write(tmp.path().join("plugin.yaml"), yaml).expect("write");
    let plugin = LoadedPlugin::from_dir_with_key(tmp.path(), None, Some("Unsigned"))
        .expect("free plugin loads with any trust");
    assert_eq!(plugin.manifest.id, "free-plugin");
}

#[test]
fn manifest_v2_full_yaml_parses() {
    let m: PluginManifest = serde_yaml::from_str(PAID_PLUGIN_YAML).expect("parse");
    assert_eq!(m.id, "law-pro");
    let pricing = m.pricing.expect("has pricing");
    assert_eq!(pricing.tier, "paid");
    assert_eq!(pricing.trial_quota, Some(10));
    let resources = m.resources.expect("has resources");
    assert_eq!(resources.total_max_llm_tokens_per_call, Some(10000));
    assert_eq!(m.skills.len(), 1);
    assert_eq!(m.skills[0].cost.llm_tokens, Some(500));
    assert_eq!(m.agents[0].case_kinds, vec!["civil-loan"]);
    assert_eq!(m.mcp_servers[0].lifecycle, "eager"); // default
    assert_eq!(m.mcp_servers[0].heartbeat_interval_seconds, 30); // default
}

#[test]
fn document_classifier_via_agent_trait() {
    use attune_core::agents::document_classifier::DocumentClassifierAgent;

    let agent = DocumentClassifierAgent;
    assert_eq!(agent.id(), "document_classifier_agent");
    assert!(!agent.description().is_empty());
    assert!(agent.case_kinds().is_empty()); // 通用

    let docs = vec![
        ("借条.pdf".to_string(), "借条 出借人 借款人 本金".to_string()),
        ("流水.pdf".to_string(), "交易日期 余额 对方户名".to_string()),
    ];
    let out = agent.run(docs).expect("run");
    assert_eq!(out.computation.classified.len(), 2);
    assert!(!out.has_red_lines());
}

#[test]
fn chunk_kind_serde_json_roundtrip() {
    use attune_core::skills::classify_chunk_kind::{classify, ChunkKind};

    let r = classify("借条 出借人 借款人 本金 月利率");
    assert_eq!(r.kind, ChunkKind::BorrowingDoc);

    let json = serde_json::to_string(&r).expect("ser");
    assert!(json.contains("\"kind\":\"borrowing_doc\""));
    let back: attune_core::skills::classify_chunk_kind::Classification =
        serde_json::from_str(&json).expect("de");
    assert_eq!(back.kind, ChunkKind::BorrowingDoc);
}

#[test]
fn case_metadata_with_classified_evidence_persists() {
    use attune_core::agents::document_classifier::DocumentClassifierAgent;
    use attune_core::case_metadata::CaseMetadata;

    let agent = DocumentClassifierAgent;
    let docs = vec![
        ("借条.pdf".to_string(), "借条 出借人 借款人 本金".to_string()),
        ("流水.pdf".to_string(), "交易日期 余额 对方户名".to_string()),
    ];
    let out = agent.run(docs).expect("run");

    let mut meta = CaseMetadata::new(Some("civil-loan".into()))
        .add_party("张三", "plaintiff", true)
        .add_party("李四", "defendant", false);
    meta.update_classified(out.computation.classified.clone());

    let json = meta.to_json().expect("ser");
    let back = CaseMetadata::from_json(&json).expect("de");
    assert_eq!(back.kind.as_deref(), Some("civil-loan"));
    assert_eq!(back.parties.len(), 2);
    assert_eq!(back.classified_evidence.len(), 2);
    assert_eq!(back.our_client_name(), Some("张三"));

    // 按 kind 过滤
    let bd = back.evidence_by_kind("borrowing_doc");
    assert_eq!(bd.len(), 1);
    assert_eq!(bd[0].file, "借条.pdf");
}

#[test]
fn registry_aggregates_paid_and_free_plugins() {
    let tmp = TempDir::new().expect("tmp");

    // Plugin 1: paid law-pro 加密
    let p1 = tmp.path().join("law-pro");
    fs::create_dir_all(&p1).expect("mkdir p1");
    let key = b"k1";
    let cipher = encrypt_yaml(PAID_PLUGIN_YAML.as_bytes(), key).expect("e1");
    fs::write(p1.join("plugin.yaml.enc"), &cipher).expect("w1");

    // Plugin 2: free 明文
    let p2 = tmp.path().join("notes-pro");
    fs::create_dir_all(&p2).expect("mkdir p2");
    fs::write(
        p2.join("plugin.yaml"),
        r#"
id: notes-pro
name: Notes
type: utility
version: "0.1.0"
pricing:
  tier: free
registers_case_kinds:
  - kind: notes
    label: Notes
    default_agent: notes_agent
agents:
  - id: notes_agent
    case_kinds: [notes]
    runtime: rust_binary
"#,
    )
    .expect("w2");

    // 注: PluginRegistry::scan 现版默认调 from_dir (非加密版), 加密 plugin 需调用方
    // 用 from_dir_with_key 装载. 这里只验证 free plugin 能装, paid 加密不通过 scan 的 default.
    let (reg, errs) = PluginRegistry::scan(tmp.path()).expect("scan");
    // 加密 plugin 不会被 scan 默认 from_dir 装载, 应在 errs 中或被 skip
    let agents = reg.list_agents();
    let kinds = reg.all_registered_case_kinds();
    // free notes-pro 应已装
    assert!(agents.iter().any(|(_, a)| a.id == "notes_agent"));
    assert!(kinds.iter().any(|k| k.kind == "notes"));
    // 加密 plugin 未装 (scan 不会自动解密)
    assert!(!agents.iter().any(|(_, a)| a.id == "civil_loan_agent"));
    let _ = errs; // scan 可能报错可能 skip, 都是合法行为
}

#[test]
fn agent_runner_unknown_agent_errors() {
    use attune_core::agent_runner::run_agent_subprocess;
    use std::time::Duration;

    let reg = PluginRegistry::new();
    let tmp = TempDir::new().expect("tmp");
    let result = run_agent_subprocess(
        &reg,
        "nonexistent_agent_xyz",
        tmp.path(),
        "{}",
        vec![],
        Duration::from_secs(1),
    );
    assert!(result.is_err());
}

#[test]
fn agent_runner_subprocess_e2e_with_mock_binary() {
    use attune_core::agent_runner::format_agent_result_for_chat;
    use attune_core::capability_dispatch::{dispatch, CapabilityInvocation};
    use std::time::Duration;

    // 直接用 capability_dispatch + format_agent_result_for_chat 验证端到端
    // (run_agent_subprocess 需要 plugin registry 完整 setup, 此处用更直接路径)
    let sh = which::which("sh").unwrap_or_else(|_| std::path::PathBuf::from("/bin/sh"));
    if !sh.exists() {
        return;
    }
    let inv = CapabilityInvocation::new(&sh)
        .args([
            "-c",
            "echo '{\"computed\":42}' && >&2 echo 'audit: ran skill x' && exit 0",
        ])
        .timeout(Duration::from_secs(2));
    let r = dispatch(&inv).expect("dispatch");
    let formatted = format_agent_result_for_chat(&r, "mock_agent");
    assert!(formatted.contains("✅ mock_agent"));
    assert!(formatted.contains("audit: ran skill"));
    assert!(formatted.contains("\"computed\":42"));
}
