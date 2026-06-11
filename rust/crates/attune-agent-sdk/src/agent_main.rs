//! Generic stdin-JSON → Agent::run → stdout-JSON → exit-code entry helper.
//!
//! Single SSOT for the law-pro (and future) agent subprocess ABI. wasm-safe:
//! depends only on serde / serde_json / std (WASI provides stdin/stdout/ExitCode).
//!
//! Each `agent_X` binary becomes a ~8-LOC `main()` that calls `run_agent_stdio`
//! (or `run_agent_stdio_with_llm` prep for LLM agents) with a per-bin
//! `EntryConfig`. The 5 difference axes (input type, agent ctor, red-line
//! exit-2 gate, LLM env, stderr section labels) are all parameters here.

#[allow(unused_imports)] // AgentOutput inferred via Agent::Output in run_agent_with
use crate::{Agent, AgentOutput};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::io::Read;
use std::process::ExitCode;

/// Which `AgentOutput` field a stderr section renders.
#[derive(Clone, Copy)]
pub enum SectionField {
    RedLines,
    MissingEvidence,
    Followups,
}

/// One human-readable stderr section. `bullet` is the EXACT per-item prefix
/// (including trailing spaces) — e.g. red lines use `"⚠️  "` (emoji + two
/// spaces), soft items use `"- "`. Empty fields are skipped (no header).
#[derive(Clone, Copy)]
pub struct Section {
    pub title: &'static str,
    pub field: SectionField,
    pub bullet: &'static str,
}

/// Per-bin entry configuration — the 5th difference axis (stderr labels) plus
/// the red-line exit-2 gate. `input_name` is only used in the empty-stdin
/// diagnostic message ("stdin is empty (expected <Name> JSON)").
pub struct EntryConfig {
    pub red_line_exit2: bool,
    pub sections: &'static [Section],
    pub input_name: &'static str,
}

/// Decoupled exit outcome → exit code. Lets unit tests assert the branch
/// without spawning a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentExit {
    Success,         // 0
    Internal,        // 1 — IO / serialize / Agent::run error
    RedLine,         // 2 — business red line (only when cfg.red_line_exit2)
    ClientError,     // 3 — empty stdin / JSON parse failure
    LlmUnavailable,  // 4 — LLM agent, endpoint unset / provider unavailable
}

impl AgentExit {
    /// Map to the process exit code. Frozen ABI: 0/1/2/3/4.
    pub fn to_exit_code(self) -> ExitCode {
        match self {
            AgentExit::Success => ExitCode::SUCCESS,
            AgentExit::Internal => ExitCode::FAILURE, // == 1
            AgentExit::RedLine => ExitCode::from(2),
            AgentExit::ClientError => ExitCode::from(3),
            AgentExit::LlmUnavailable => ExitCode::from(4),
        }
    }
}

/// Validated LLM env (for `run_agent_stdio_with_llm` callers). The native
/// provider (reqwest) is constructed by the bin, NOT here — leaf stays wasm-safe.
pub struct LlmEnv {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
}

/// Read LLM_ENDPOINT / LLM_API_KEY / LLM_MODEL; empty endpoint → LlmUnavailable.
/// Matches current `agent_fact_extract.rs:34-40` byte-for-byte on the empty path.
pub fn prepare_llm_env(default_model: &str) -> Result<LlmEnv, AgentExit> {
    let endpoint = std::env::var("LLM_ENDPOINT").unwrap_or_default();
    let api_key = std::env::var("LLM_API_KEY").unwrap_or_default();
    let model = std::env::var("LLM_MODEL").unwrap_or_else(|_| default_model.to_string());
    if endpoint.is_empty() {
        eprintln!("LLM_ENDPOINT not set — fact extraction requires an LLM");
        return Err(AgentExit::LlmUnavailable);
    }
    Ok(LlmEnv { endpoint, api_key, model })
}

/// Production entry: reads real stdin, writes real stdout/stderr.
/// `make` is called only AFTER input parses (so an LLM provider built by the
/// bin is not constructed on a client error — matches current ordering for
/// deterministic bins; LLM bins do provider build before calling this, per §4).
pub fn run_agent_stdio<A, F>(make: F, cfg: &EntryConfig) -> AgentExit
where
    A: Agent,
    A::Input: DeserializeOwned,
    A::Output: Serialize,
    F: FnOnce() -> A,
{
    let stdin = std::io::stdin();
    let mut lock = stdin.lock();
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    run_agent_with(&mut lock, &mut out, &mut err, make, cfg)
}

/// Testable core: I/O via injected `Read`/`Write` seams.
pub fn run_agent_with<A, F, R, W, E>(
    mut reader: R,
    out: &mut W,
    err: &mut E,
    make: F,
    cfg: &EntryConfig,
) -> AgentExit
where
    A: Agent,
    A::Input: DeserializeOwned,
    A::Output: Serialize,
    F: FnOnce() -> A,
    R: Read,
    W: std::io::Write,
    E: std::io::Write,
{
    let mut buf = String::new();
    if let Err(e) = reader.read_to_string(&mut buf) {
        let _ = writeln!(err, "stdin read error: {e}");
        return AgentExit::Internal;
    }
    if buf.trim().is_empty() {
        let _ = writeln!(err, "stdin is empty (expected {} JSON)", cfg.input_name);
        return AgentExit::ClientError;
    }

    let input: A::Input = match serde_json::from_str(&buf) {
        Ok(i) => i,
        Err(e) => {
            let _ = writeln!(err, "input JSON parse error: {e}");
            return AgentExit::ClientError;
        }
    };

    let agent = make();
    let output = match agent.run(input) {
        Ok(o) => o,
        Err(e) => {
            let _ = writeln!(err, "agent run error: {e}");
            return AgentExit::Internal;
        }
    };

    let json = match serde_json::to_string_pretty(&output) {
        Ok(j) => j,
        Err(e) => {
            let _ = writeln!(err, "serialize output: {e}");
            return AgentExit::Internal;
        }
    };
    let _ = writeln!(out, "{json}");

    // stderr: audit_trail, then each configured section (skipping empties).
    let _ = writeln!(err, "{}", output.audit_trail);
    for section in cfg.sections {
        let items: &[String] = match section.field {
            SectionField::RedLines => &output.red_lines_violated,
            SectionField::MissingEvidence => &output.missing_evidence,
            SectionField::Followups => &output.followups,
        };
        if items.is_empty() {
            continue;
        }
        let _ = writeln!(err, "\n=== {} ===", section.title);
        for item in items {
            let _ = writeln!(err, "{}{item}", section.bullet);
        }
    }

    if cfg.red_line_exit2 && output.has_red_lines() {
        AgentExit::RedLine
    } else {
        AgentExit::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentError, AgentResult};
    use serde::{Deserialize, Serialize};

    // A mock Agent whose behavior is driven by the input, so each branch is testable.
    #[derive(Debug, Deserialize)]
    struct MockInput {
        cmd: String, // "ok" | "redline" | "runerr"
    }
    #[derive(Debug, Serialize, Deserialize)]
    struct MockOutput {
        echoed: String,
    }
    struct MockAgent;
    impl Agent for MockAgent {
        type Input = MockInput;
        type Output = MockOutput;
        fn id(&self) -> &str { "mock" }
        fn description(&self) -> &str { "mock agent for helper tests" }
        fn case_kinds(&self) -> &[&str] { &[] }
        fn run(&self, input: Self::Input) -> AgentResult<AgentOutput<Self::Output>> {
            match input.cmd.as_str() {
                "runerr" => Err(AgentError::Computation("forced error".into())),
                "redline" => Ok(AgentOutput {
                    computation: MockOutput { echoed: "rl".into() },
                    audit_trail: "trail".into(),
                    red_lines_violated: vec!["violated".into()],
                    missing_evidence: vec![],
                    followups: vec![],
                    confidence: 1.0,
                }),
                _ => Ok(AgentOutput {
                    computation: MockOutput { echoed: "ok".into() },
                    audit_trail: "trail".into(),
                    red_lines_violated: vec![],
                    missing_evidence: vec![],
                    followups: vec![],
                    confidence: 1.0,
                }),
            }
        }
    }

    const TEST_CFG: EntryConfig = EntryConfig {
        red_line_exit2: true,
        sections: &[],
        input_name: "MockInput",
    };

    // Drive the helper with an injected reader so we don't touch real stdin.
    // run_agent_with provides the seam; run_agent_stdio is the thin std::io::stdin wrapper.
    fn run_with(stdin: &str, cfg: &EntryConfig) -> (AgentExit, Vec<u8>, Vec<u8>) {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let exit = run_agent_with(
            stdin.as_bytes(),
            &mut out,
            &mut err,
            || MockAgent,
            cfg,
        );
        (exit, out, err)
    }

    #[test]
    fn success_path_returns_success_and_emits_stdout_json() {
        let (exit, out, err) = run_with(r#"{"cmd":"ok"}"#, &TEST_CFG);
        assert!(matches!(exit, AgentExit::Success));
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("\"echoed\": \"ok\""), "stdout was: {s}");
        // pretty-printed JSON ends with newline from println!
        assert!(s.ends_with('\n'));
        let e = String::from_utf8(err).unwrap();
        assert!(e.starts_with("trail\n"));
    }

    #[test]
    fn empty_stdin_returns_client_error() {
        let (exit, out, _err) = run_with("   \n  ", &TEST_CFG);
        assert!(matches!(exit, AgentExit::ClientError));
        assert!(out.is_empty(), "no stdout on client error");
    }

    #[test]
    fn bad_json_returns_client_error() {
        let (exit, _out, _err) = run_with("{not json", &TEST_CFG);
        assert!(matches!(exit, AgentExit::ClientError));
    }

    #[test]
    fn run_error_returns_internal() {
        let (exit, out, _err) = run_with(r#"{"cmd":"runerr"}"#, &TEST_CFG);
        assert!(matches!(exit, AgentExit::Internal));
        assert!(out.is_empty(), "no stdout when run errors");
    }

    #[test]
    fn red_line_with_gate_enabled_returns_redline() {
        let (exit, out, _err) = run_with(r#"{"cmd":"redline"}"#, &TEST_CFG);
        assert!(matches!(exit, AgentExit::RedLine));
        // stdout IS still emitted on red-line (matches current bins: JSON printed, then exit 2)
        assert!(!out.is_empty());
    }

    #[test]
    fn red_line_with_gate_disabled_returns_success() {
        const NO_GATE: EntryConfig = EntryConfig {
            red_line_exit2: false,
            sections: &[],
            input_name: "MockInput",
        };
        let (exit, _out, _err) = run_with(r#"{"cmd":"redline"}"#, &NO_GATE);
        assert!(matches!(exit, AgentExit::Success));
    }

    #[test]
    fn exit_code_mapping() {
        assert_eq!(exit_code_value(AgentExit::Success), 0);
        assert_eq!(exit_code_value(AgentExit::Internal), 1);
        assert_eq!(exit_code_value(AgentExit::RedLine), 2);
        assert_eq!(exit_code_value(AgentExit::ClientError), 3);
        assert_eq!(exit_code_value(AgentExit::LlmUnavailable), 4);
    }

    // helper for the mapping test: ExitCode is opaque, so assert via the i32 the
    // enum is documented to map to.
    fn exit_code_value(e: AgentExit) -> u8 {
        match e {
            AgentExit::Success => 0,
            AgentExit::Internal => 1,
            AgentExit::RedLine => 2,
            AgentExit::ClientError => 3,
            AgentExit::LlmUnavailable => 4,
        }
    }

    #[test]
    fn sections_emitted_in_order_with_exact_bullets() {
        const CFG: EntryConfig = EntryConfig {
            red_line_exit2: false,
            sections: &[
                Section { title: "RL", field: SectionField::RedLines, bullet: "⚠️  " },
                Section { title: "MISS", field: SectionField::MissingEvidence, bullet: "- " },
                Section { title: "FOLLOW", field: SectionField::Followups, bullet: "- " },
            ],
            input_name: "MockInput",
        };
        // Build an output with all three populated via a custom agent inline.
        struct A3;
        impl Agent for A3 {
            type Input = MockInput;
            type Output = MockOutput;
            fn id(&self) -> &str { "a3" }
            fn description(&self) -> &str { "" }
            fn case_kinds(&self) -> &[&str] { &[] }
            fn run(&self, _i: Self::Input) -> AgentResult<AgentOutput<Self::Output>> {
                Ok(AgentOutput {
                    computation: MockOutput { echoed: "x".into() },
                    audit_trail: "AUDIT".into(),
                    red_lines_violated: vec!["r1".into()],
                    missing_evidence: vec!["m1".into()],
                    followups: vec!["f1".into()],
                    confidence: 1.0,
                })
            }
        }
        let mut out = Vec::new();
        let mut err = Vec::new();
        let _ = run_agent_with("{\"cmd\":\"ok\"}".as_bytes(), &mut out, &mut err, || A3, &CFG);
        let e = String::from_utf8(err).unwrap();
        assert_eq!(
            e,
            "AUDIT\n\n=== RL ===\n⚠️  r1\n\n=== MISS ===\n- m1\n\n=== FOLLOW ===\n- f1\n"
        );
    }

    #[test]
    fn empty_section_is_skipped() {
        const CFG: EntryConfig = EntryConfig {
            red_line_exit2: false,
            sections: &[Section { title: "MISS", field: SectionField::MissingEvidence, bullet: "- " }],
            input_name: "MockInput",
        };
        // MockAgent "ok" has empty missing_evidence → section header must NOT appear.
        let (_exit, _out, err) = run_with(r#"{"cmd":"ok"}"#, &CFG);
        let e = String::from_utf8(err).unwrap();
        assert_eq!(e, "trail\n");
    }

    #[test]
    fn prepare_llm_env_empty_endpoint_is_llm_unavailable() {
        // Ensure LLM_ENDPOINT unset for this test.
        std::env::remove_var("LLM_ENDPOINT");
        let r = prepare_llm_env("qwen2.5");
        assert!(matches!(r, Err(AgentExit::LlmUnavailable)));
    }

    proptest::proptest! {
        // P1: any AgentOutput round-trips through stdout unchanged (no mangling).
        #[test]
        fn prop_stdout_is_valid_pretty_json(echoed in "\\PC{0,50}") {
            struct PA(String);
            impl Agent for PA {
                type Input = MockInput;
                type Output = MockOutput;
                fn id(&self) -> &str { "pa" }
                fn description(&self) -> &str { "" }
                fn case_kinds(&self) -> &[&str] { &[] }
                fn run(&self, _i: Self::Input) -> AgentResult<AgentOutput<Self::Output>> {
                    Ok(AgentOutput {
                        computation: MockOutput { echoed: self.0.clone() },
                        audit_trail: String::new(),
                        red_lines_violated: vec![],
                        missing_evidence: vec![],
                        followups: vec![],
                        confidence: 1.0,
                    })
                }
            }
            let mut out = Vec::new();
            let mut err = Vec::new();
            let _ = run_agent_with("{\"cmd\":\"ok\"}".as_bytes(), &mut out, &mut err, || PA(echoed.clone()), &TEST_CFG);
            let s = String::from_utf8(out).unwrap();
            let parsed: AgentOutput<MockOutput> = serde_json::from_str(s.trim_end()).unwrap();
            proptest::prop_assert_eq!(parsed.computation.echoed, echoed);
        }

        // P2: exit never panics for arbitrary stdin bytes that are valid UTF-8.
        #[test]
        fn prop_arbitrary_input_no_panic(s in "\\PC{0,200}") {
            let mut out = Vec::new();
            let mut err = Vec::new();
            let _ = run_agent_with(s.as_bytes(), &mut out, &mut err, || MockAgent, &TEST_CFG);
        }

        // P3: empty/whitespace-only stdin is always ClientError regardless of whitespace mix.
        #[test]
        fn prop_whitespace_only_is_client_error(ws in "[ \\t\\n\\r]{0,30}") {
            let mut out = Vec::new();
            let mut err = Vec::new();
            let exit = run_agent_with(ws.as_bytes(), &mut out, &mut err, || MockAgent, &TEST_CFG);
            proptest::prop_assert!(matches!(exit, AgentExit::ClientError));
        }
    }
}
