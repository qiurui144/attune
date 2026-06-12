//! Plugin protocol 端到端集成测试 — 跨模块串通验证.
//!
//! 覆盖:
//! 1. 加密 yaml → 装载 → 解析 schema (skills/agents/mcp_servers/registers_case_kinds)
//! 2. PluginRegistry 查询 API (list_skills/agents/mcp + agents_by_case_kind)
//! 3. Agent trait roundtrip (DocumentClassifierAgent)
//! 4. ChunkKind serde JSON roundtrip
//! 5. CaseMetadata 持久化 + 反序列化 [S4b: #[ignore] — migrated to attune-pro]
//! 6. PluginManifest v2 完整 yaml 解析

use attune_core::agents::Agent;
use attune_core::plugin_encryption::{decrypt_yaml, encrypt_yaml};
use attune_core::plugin_loader::{LoadedPlugin, PluginManifest};
use attune_core::plugin_registry::PluginRegistry;
use attune_core::plugin_sig::Trust;
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
  price_url: https://engi-stack.com/pro/law-pro
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

    let plugin = LoadedPlugin::from_dir_with_key(tmp.path(), Some(key), Some(Trust::ThirdParty))
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
        LoadedPlugin::from_dir_with_key(tmp.path(), Some(b"wrong-key"), Some(Trust::ThirdParty));
    assert!(result.is_err());
}

#[test]
fn encrypted_plugin_fails_without_key() {
    let tmp = TempDir::new().expect("tmp");
    let cipher = encrypt_yaml(PAID_PLUGIN_YAML.as_bytes(), b"key").expect("encrypt");
    fs::write(tmp.path().join("plugin.yaml.enc"), &cipher).expect("write");

    let result = LoadedPlugin::from_dir_with_key(tmp.path(), None, Some(Trust::ThirdParty));
    assert!(result.is_err());
}

#[test]
fn paid_plugin_with_unsigned_trust_rejected() {
    let tmp = TempDir::new().expect("tmp");
    fs::write(tmp.path().join("plugin.yaml"), PAID_PLUGIN_YAML).expect("write");

    let result = LoadedPlugin::from_dir_with_key(tmp.path(), None, Some(Trust::Unsigned));
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
    let plugin = LoadedPlugin::from_dir_with_key(tmp.path(), None, Some(Trust::Unsigned))
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

// S4b: CaseMetadata 已迁至 attune-pro/plugins/law-pro/，此测试迁移至该仓。
// [BLOCKED: attune-pro] needs case_metadata_test.rs in law-pro/tests/.
// spec: docs/superpowers/specs/2026-06-02-oss-industry-decoupling.md §4.1 MU-2
#[test]
#[ignore = "S4b: CaseMetadata migrated to attune-pro — test moved to attune-pro/plugins/law-pro/tests/"]
fn case_metadata_with_classified_evidence_persists() {
    // Body removed — attune_core::case_metadata module deleted from OSS in S4b.
    // Equivalent test lives in attune-pro/plugins/law-pro/tests/case_metadata_test.rs.
    // [BLOCKED: attune-pro] receiving side test to be added.
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

/// scan_with_key 装载加密 paid plugin
#[test]
fn registry_scan_with_key_loads_encrypted_paid_plugin() {
    let tmp = TempDir::new().expect("tmp");

    // 写一个加密 paid plugin
    let p = tmp.path().join("law-pro");
    fs::create_dir_all(&p).expect("mkdir");
    let key = b"device-license-key";
    let cipher = encrypt_yaml(PAID_PLUGIN_YAML.as_bytes(), key).expect("encrypt");
    fs::write(p.join("plugin.yaml.enc"), &cipher).expect("write enc");

    // scan() 无 key → 装载失败 (encrypted plugin found but no key)
    let (reg, errs) = PluginRegistry::scan(tmp.path()).expect("scan");
    assert!(reg.plugins().count() == 0);
    assert!(!errs.is_empty());
    assert!(errs[0].contains("encrypted plugin") || errs[0].contains("decrypt_key"));

    // scan_with_key() 提供 key → 装载成功
    let (reg, errs) = PluginRegistry::scan_with_key(tmp.path(), Some(key)).expect("scan");
    assert_eq!(reg.plugins().count(), 1);
    assert!(errs.is_empty(), "errors: {errs:?}");
    let p = reg.plugins().next().unwrap();
    assert_eq!(p.manifest.id, "law-pro");
    assert_eq!(p.manifest.agents.len(), 1);
}

/// agent_runner subprocess env 传递测试。
///
/// 验证 run_agent_subprocess 的 env 参数确实被转发给子进程。
/// 模拟 LLM agent binary：读 LLM_ENDPOINT env，不存在则 exit 4（同 fact_extractor /
/// attune-agent-sdk prepare_llm_env 真实约定 — 裸 `LLM_*` 前缀，非 `ATTUNE_LLM_*`）。
/// 场景 1：传入正确 env → exit 0 + stdout 含 endpoint。
/// 场景 2：不传 env     → exit 4（"LLM_ENDPOINT not set"，即 P1:3 bug 复现）。
///
/// Unix-only: mock binary is a `#!/bin/sh` script that Windows cannot exec
/// (error 193 "not a valid Win32 application"). The env-propagation path under
/// test is platform-agnostic Rust code, so Unix coverage is sufficient.
#[cfg(unix)]
#[test]
fn agent_runner_subprocess_passes_llm_env_to_binary() {
    use attune_core::agent_runner::run_agent_subprocess;
    use attune_core::plugin_registry::PluginRegistry;
    use std::time::Duration;

    let sh = which::which("sh").unwrap_or_else(|_| std::path::PathBuf::from("/bin/sh"));
    if !sh.exists() {
        eprintln!("skip: sh not found");
        return;
    }

    // 写 plugin 目录结构（PluginRegistry::scan 约定）
    let tmp = TempDir::new().expect("tmp");
    let plugin_dir = tmp.path().join("llm-echo-plugin");
    let bin_dir = plugin_dir.join("bin");
    fs::create_dir_all(&bin_dir).expect("mkdir bin");

    // plugin.yaml — 声明 agent binary = bin/run_llm_echo_agent
    let plugin_yaml = r#"
id: llm-echo-plugin
name: LLM Echo Test Plugin
type: industry
version: "0.1.0"
attune_min_version: "0.6.0"
maturity: stable
pricing:
  tier: free
agents:
  - id: llm_echo_agent
    description: "Echo the LLM_ENDPOINT env var"
    runtime: rust_binary
    binary: bin/run_llm_echo_agent
"#;
    fs::write(plugin_dir.join("plugin.yaml"), plugin_yaml).expect("write plugin.yaml");

    // mock binary：读 LLM_ENDPOINT（真实 agent 约定，非 ATTUNE_LLM_*）；不存在则 exit 4
    let script_path = bin_dir.join("run_llm_echo_agent");
    let script = r#"#!/bin/sh
if [ -z "$LLM_ENDPOINT" ]; then
    echo "LLM_ENDPOINT not set" >&2
    exit 4
fi
echo "{\"endpoint\":\"$LLM_ENDPOINT\"}"
exit 0
"#;
    fs::write(&script_path, script).expect("write script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script_path).expect("meta").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).expect("chmod");
    }

    // scan plugins root（tmp）→ 自动装载 llm-echo-plugin
    let (reg, errs) = PluginRegistry::scan(tmp.path()).expect("scan");
    assert!(errs.is_empty(), "plugin scan errors: {errs:?}");
    assert!(
        reg.list_agents().iter().any(|(_, a)| a.id == "llm_echo_agent"),
        "llm_echo_agent should be registered"
    );

    // 场景 1：传 env → exit 0，stdout 含 endpoint
    let env_with_llm = vec![
        ("LLM_PROVIDER".to_string(), "openai_compat".to_string()),
        ("LLM_ENDPOINT".to_string(), "https://api.deepseek.com/v1".to_string()),
        ("LLM_MODEL".to_string(), "deepseek-chat".to_string()),
        ("LLM_API_KEY".to_string(), "sk-test".to_string()),
    ];
    let result = run_agent_subprocess(
        &reg,
        "llm_echo_agent",
        &plugin_dir,
        "{}",
        env_with_llm,
        Duration::from_secs(5),
    )
    .expect("run with env");
    assert_eq!(result.exit_code, 0, "exit with env: {} | {}", result.stdout, result.stderr);
    assert!(
        result.stdout.contains("deepseek.com"),
        "stdout should contain endpoint: {}",
        result.stdout
    );

    // 场景 2：不传 env → exit 4（复现 P1:3 bug）
    let result_no_env = run_agent_subprocess(
        &reg,
        "llm_echo_agent",
        &plugin_dir,
        "{}",
        vec![],
        Duration::from_secs(5),
    )
    .expect("run without env");
    assert_eq!(
        result_no_env.exit_code, 4,
        "exit without env should be 4 (LLM_ENDPOINT not set): {} | {}",
        result_no_env.stdout,
        result_no_env.stderr
    );
    assert!(
        result_no_env.stderr.contains("LLM_ENDPOINT not set"),
        "stderr: {}",
        result_no_env.stderr
    );
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
