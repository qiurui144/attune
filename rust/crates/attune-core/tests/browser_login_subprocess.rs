//! INT-1 §9 集成 E2E (≥1 subprocess) + 对抗/安全用例 for the browser login-assist
//! SidecarController.
//!
//! These tests spawn a **real subprocess** (a fake sidecar script that emits the
//! G1-G6 JSON-over-CLI contract) and drive the actual
//! [`attune_core::browser_login::SidecarController::run`] path: locate → spawn →
//! stdin credential injection → single-JSON stdout parse → exit-code routing →
//! temp-state cleanup. This exercises the contract binding end-to-end without a
//! real browser / real site (per task: mock subprocess, not a live login).
//!
//! Unix-only (the fake sidecar is a `/bin/sh` script). Windows CLI-spawn is
//! covered by the unit tests in `sidecar.rs` (argv/stdin construction) + the
//! real-tool smoke; a Windows fake-sidecar .cmd is out of scope for this slice.

#![cfg(unix)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use attune_core::browser_login::{
    Credentials, RunOutcome, SidecarCommand, SidecarController, SidecarError, SidecarProgram,
};

/// Write an executable `/bin/sh` fake-sidecar to `dir/name` and return its path.
/// The body is the script after the shebang.
fn write_fake_sidecar(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    {
        let mut f = std::fs::File::create(&path).unwrap();
        write!(f, "#!/bin/sh\n{body}").unwrap();
        f.sync_all().unwrap(); // flush + close so exec doesn't hit ETXTBSY
    }
    let mut perm = std::fs::metadata(&path).unwrap().permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(&path, perm).unwrap();
    path
}

fn controller_for(script: PathBuf) -> SidecarController {
    SidecarController::new(SidecarProgram {
        program: script,
        module_args: vec![],
    })
    .with_timeout(Duration::from_secs(10))
}

// ── happy path: scan → ok (exit 0) ──────────────────────────────────────────
#[test]
fn scan_returns_ok_status_and_success_outcome() {
    let tmp = tempfile::tempdir().unwrap();
    // G1: print a single JSON document; G2: exit 0 for "ok".
    let script = write_fake_sidecar(
        tmp.path(),
        "fake-scan",
        r#"echo '{"schema_version":"1","status":"ok","url":"https://members.example/","records":[],"signals":["logged-in"]}'
exit 0
"#,
    );
    let ctrl = controller_for(script);
    let (result, outcome) = ctrl
        .run(SidecarCommand::Scan {
            recipe_path: tmp.path().join("r.json"),
        })
        .expect("scan should complete");
    assert_eq!(result.status, "ok");
    assert_eq!(outcome, RunOutcome::Success);
    assert_eq!(result.url, "https://members.example/");
}

// ── E2E flow: scan needs-login → login (resume) → run crawls a record ────────
#[test]
fn full_flow_scan_needslogin_then_run_crawls_record() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("session.json");

    // scan → needs-login (exit 12, session-expired/needs-login class).
    let scan = write_fake_sidecar(
        tmp.path(),
        "fake-scan2",
        r#"echo '{"schema_version":"1","status":"needs-login","url":"https://members.example/"}'
exit 12
"#,
    );
    let (r1, o1) = controller_for(scan)
        .run(SidecarCommand::Scan {
            recipe_path: tmp.path().join("r.json"),
        })
        .unwrap();
    assert_eq!(r1.status, "needs-login");
    assert_eq!(o1, RunOutcome::SessionExpired);

    // login → writes a state file, returns logged-in (exit 0). The fake writes a
    // fake storage_state so we can confirm the controller cleans it up afterward.
    let login = write_fake_sidecar(
        tmp.path(),
        "fake-login",
        &format!(
            r#"printf '%s' '{{"cookies":[{{"name":"sess","value":"SESSION_SECRET_TOKEN"}}]}}' > '{}'
echo '{{"schema_version":"1","status":"logged-in","url":"https://members.example/home"}}'
exit 0
"#,
            state.display()
        ),
    );
    let (r2, o2) = controller_for(login)
        .run(SidecarCommand::Login {
            recipe_path: tmp.path().join("r.json"),
            state_path: state.clone(),
            wait_seconds: 1,
        })
        .unwrap();
    assert_eq!(o2, RunOutcome::Success);
    assert_eq!(r2.status, "logged-in");
    // temp-state cleanup (§9 case 10): the controller deletes the working state.
    assert!(!state.exists(), "temp state file must be cleaned up after run");

    // run → reuse session, crawl one record (exit 0).
    let run = write_fake_sidecar(
        tmp.path(),
        "fake-run",
        r#"echo '{"schema_version":"1","status":"ok","url":"https://members.example/article/1","records":[{"title":"会员文章","body":"正文"}]}'
exit 0
"#,
    );
    let (r3, o3) = controller_for(run)
        .run(SidecarCommand::Run {
            recipe_path: tmp.path().join("r.json"),
            state_path: tmp.path().join("session2.json"),
            query: "RISC-V".into(),
        })
        .unwrap();
    assert_eq!(o3, RunOutcome::Success);
    assert_eq!(r3.records.len(), 1);
    assert_eq!(r3.records[0]["title"], "会员文章");
}

// ── needs-human (CAPTCHA/MFA) → exit 10 ─────────────────────────────────────
#[test]
fn needs_human_status_routes_to_needshuman() {
    let tmp = tempfile::tempdir().unwrap();
    let script = write_fake_sidecar(
        tmp.path(),
        "fake-human",
        r#"echo '{"schema_version":"1","status":"needs-human","url":"https://x/","error_code":"captcha"}'
exit 10
"#,
    );
    let (result, outcome) = controller_for(script)
        .run(SidecarCommand::Scan {
            recipe_path: tmp.path().join("r.json"),
        })
        .unwrap();
    assert_eq!(outcome, RunOutcome::NeedsHuman);
    assert_eq!(result.error_code.as_deref(), Some("captcha"));
}

// ── restricted (paywall) → exit 11 ──────────────────────────────────────────
#[test]
fn restricted_status_routes_to_restricted() {
    let tmp = tempfile::tempdir().unwrap();
    let script = write_fake_sidecar(
        tmp.path(),
        "fake-restricted",
        r#"echo '{"schema_version":"1","status":"restricted","url":"https://x/"}'
exit 11
"#,
    );
    let (_r, outcome) = controller_for(script)
        .run(SidecarCommand::Scan {
            recipe_path: tmp.path().join("r.json"),
        })
        .unwrap();
    assert_eq!(outcome, RunOutcome::Restricted);
}

// ── error path: bad JSON on stdout → BadOutput ──────────────────────────────
#[test]
fn non_json_stdout_is_bad_output_error() {
    let tmp = tempfile::tempdir().unwrap();
    let script = write_fake_sidecar(
        tmp.path(),
        "fake-bad",
        r#"echo 'INFO: launching browser, this is a log line not JSON'
exit 0
"#,
    );
    let err = controller_for(script)
        .run(SidecarCommand::Scan {
            recipe_path: tmp.path().join("r.json"),
        })
        .unwrap_err();
    assert!(matches!(err, SidecarError::BadOutput(_)), "got {err:?}");
}

// ── error path: schema mismatch (major version 2) → SchemaMismatch ──────────
#[test]
fn unsupported_schema_version_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let script = write_fake_sidecar(
        tmp.path(),
        "fake-schema2",
        r#"echo '{"schema_version":"2","status":"ok","url":"https://x/"}'
exit 0
"#,
    );
    let err = controller_for(script)
        .run(SidecarCommand::Scan {
            recipe_path: tmp.path().join("r.json"),
        })
        .unwrap_err();
    assert!(matches!(err, SidecarError::SchemaMismatch { .. }), "got {err:?}");
}

// ── timeout: child sleeps past the cap → killed + Timeout error ─────────────
#[test]
fn run_that_exceeds_timeout_is_killed() {
    let tmp = tempfile::tempdir().unwrap();
    let script = write_fake_sidecar(
        tmp.path(),
        "fake-hang",
        r#"sleep 30
echo '{"schema_version":"1","status":"ok","url":"https://x/"}'
exit 0
"#,
    );
    let ctrl = SidecarController::new(SidecarProgram {
        program: script,
        module_args: vec![],
    })
    .with_timeout(Duration::from_millis(300));
    let start = std::time::Instant::now();
    let err = ctrl
        .run(SidecarCommand::Scan {
            recipe_path: tmp.path().join("r.json"),
        })
        .unwrap_err();
    assert!(matches!(err, SidecarError::Timeout(_)), "got {err:?}");
    assert!(start.elapsed() < Duration::from_secs(5), "must not wait out the child");
}

// ── tool-not-found: spawn a non-existent program → SpawnFailed ──────────────
#[test]
fn missing_program_yields_spawn_failed() {
    let ctrl = SidecarController::new(SidecarProgram {
        program: PathBuf::from("/nonexistent/attune-fake-sidecar-xyz"),
        module_args: vec![],
    });
    let err = ctrl
        .run(SidecarCommand::Scan {
            recipe_path: PathBuf::from("/tmp/r.json"),
        })
        .unwrap_err();
    assert!(matches!(err, SidecarError::SpawnFailed(_)), "got {err:?}");
}

// ── SECURITY §9.2: credentials over stdin are NOT visible in argv ────────────
// The fake `auto` sidecar dumps its own argv to a side file; we assert the
// credential values never appear there (they must arrive over stdin only).
#[test]
fn auto_credentials_never_appear_in_argv() {
    let tmp = tempfile::tempdir().unwrap();
    let argv_dump = tmp.path().join("argv.txt");
    let stdin_dump = tmp.path().join("stdin.txt");
    let state = tmp.path().join("s.json");
    // Script records its argv + the stdin it received, then emits ok.
    let script = write_fake_sidecar(
        tmp.path(),
        "fake-auto",
        &format!(
            r#"echo "$@" > '{argv}'
cat > '{stdin}'
echo '{{"schema_version":"1","status":"logged-in","url":"https://x/"}}'
exit 0
"#,
            argv = argv_dump.display(),
            stdin = stdin_dump.display(),
        ),
    );
    let (_r, outcome) = controller_for(script)
        .run(SidecarCommand::Auto {
            recipe_path: tmp.path().join("r.json"),
            state_path: state,
            credentials: Credentials {
                username: "alice_user".into(),
                password: "p@ssw0rd_SECRET".into(),
            },
        })
        .unwrap();
    assert_eq!(outcome, RunOutcome::Success);

    let argv = std::fs::read_to_string(&argv_dump).unwrap();
    assert!(!argv.contains("alice_user"), "username leaked into argv: {argv}");
    assert!(!argv.contains("p@ssw0rd_SECRET"), "password leaked into argv: {argv}");
    assert!(argv.contains("--credentials-stdin"), "stdin flag must be present");

    // Conversely, credentials DID arrive over stdin (the only allowed channel).
    let stdin = std::fs::read_to_string(&stdin_dump).unwrap();
    assert!(stdin.contains("alice_user"), "credentials must arrive over stdin");
    assert!(stdin.contains("p@ssw0rd_SECRET"));
    let v: serde_json::Value = serde_json::from_str(stdin.trim()).unwrap();
    assert_eq!(v["username"], "alice_user");
    assert_eq!(v["password"], "p@ssw0rd_SECRET");
}
