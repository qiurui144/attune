//! `SidecarController` — locate + spawn the `community-browser-automation` CLI
//! and speak its verified JSON-over-CLI contract (G1-G6 @ 212c957).
//!
//! ## Contract bound here (do not drift — mirrors the source tool's `cli.py`)
//!
//! - **G1 stdout = single JSON document** with `schema_version` first key, plus
//!   `status` / `url` / `records` / `error` / `error_code`. Parsed into
//!   [`RunResult`]. Logs go to **stderr** (never parsed as data).
//! - **G2 exit codes**: `0` success (`ok`/`logged-in`), `10` needs-human,
//!   `11` restricted, `12` session-expired/needs-login, `2` usage, `1` internal.
//!   Routed to [`RunOutcome`].
//! - **G3 `--credentials-stdin`**: credentials JSON
//!   `{"username":..,"password":..}` is written to the child's **stdin** and the
//!   pipe closed. Credentials never appear in argv / env / logs (§1.4 / L-3).
//! - **G4 `done\n` resume**: for human-in-the-loop `login`, the controller can
//!   write `done\n` to stdin to signal the human finished, so the child captures
//!   the session immediately instead of waiting out `--wait-seconds`.
//! - timeout + kill + temp-state cleanup: every run is bounded; on
//!   timeout/crash the child is killed and the temp `--state` file is removed
//!   and zeroized (it can briefly hold the captured session).
//!
//! ## Locating the binary (cross-platform, Win P0)
//!
//! 1. `ATTUNE_BROWSER_TOOL` env override (absolute path to the launcher);
//! 2. a `community-browser` executable on `PATH` (via the `which` crate);
//! 3. `python -m community_browser_automation` if a bundled interpreter is on
//!    `PATH` and the module imports.
//!
//! If none is found → [`SidecarError::ToolNotFound`] (kebab
//! `browser-tool-not-found`); the capability disables gracefully, never panics.

use std::io::Write;
use std::path::PathBuf;
use crate::process::command_no_window;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::Deserialize;
use zeroize::Zeroize;

/// Contract schema version the controller is written against. A sidecar that
/// reports a different major version is rejected (G1 fast-check).
pub const SUPPORTED_SCHEMA_VERSION: &str = "1";

/// Default per-run wall-clock cap. Login (human-in-the-loop) overrides via the
/// recipe's wait-seconds; scan/run use this.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Hard upper bound on stdout we will buffer (defense against a runaway child).
const MAX_STDOUT_BYTES: usize = 4 * 1024 * 1024;

/// Plaintext credentials, injected over stdin only. Zeroized on drop.
#[derive(Clone, Zeroize)]
#[zeroize(drop)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render credential values, even in Debug/log output (L-3).
        f.debug_struct("Credentials")
            .field("username", &"<redacted>")
            .field("password", &"<redacted>")
            .finish()
    }
}

/// Sidecar subcommand. `Auto` is the only one that carries credentials, and it
/// is **off by default** at the capability layer (L-5) — the controller will
/// only build it when the caller explicitly opts in.
#[derive(Debug, Clone)]
pub enum SidecarCommand {
    /// `scan <recipe>` — probe login/captcha/restriction signals.
    Scan { recipe_path: PathBuf },
    /// `login <recipe> --state <out> --wait-seconds N` — human-in-the-loop.
    Login {
        recipe_path: PathBuf,
        state_path: PathBuf,
        wait_seconds: u32,
    },
    /// `auto <recipe> --state <out> --credentials-stdin` — LLM form fill.
    /// Default-OFF at the capability layer; credentials over stdin only (L-3).
    Auto {
        recipe_path: PathBuf,
        state_path: PathBuf,
        credentials: Credentials,
    },
    /// `run <recipe> --state <in> --query <q>` — reuse session, crawl + extract.
    Run {
        recipe_path: PathBuf,
        state_path: PathBuf,
        query: String,
    },
}

impl SidecarCommand {
    /// Stable subcommand name (diagnostics / audit; no secret).
    pub fn subcommand(&self) -> &'static str {
        match self {
            SidecarCommand::Scan { .. } => "scan",
            SidecarCommand::Login { .. } => "login",
            SidecarCommand::Auto { .. } => "auto",
            SidecarCommand::Run { .. } => "run",
        }
    }

    /// The temp `--state` file this command writes/reads, if any (for cleanup).
    fn state_path(&self) -> Option<&PathBuf> {
        match self {
            SidecarCommand::Scan { .. } => None,
            SidecarCommand::Login { state_path, .. } => Some(state_path),
            SidecarCommand::Auto { state_path, .. } => Some(state_path),
            SidecarCommand::Run { state_path, .. } => Some(state_path),
        }
    }

    /// Build the argv (credentials **never** appear here — they go via stdin).
    fn argv(&self) -> Vec<String> {
        match self {
            SidecarCommand::Scan { recipe_path } => {
                vec!["scan".into(), recipe_path.display().to_string()]
            }
            SidecarCommand::Login {
                recipe_path,
                state_path,
                wait_seconds,
            } => vec![
                "login".into(),
                recipe_path.display().to_string(),
                "--state".into(),
                state_path.display().to_string(),
                "--wait-seconds".into(),
                wait_seconds.to_string(),
            ],
            SidecarCommand::Auto {
                recipe_path,
                state_path,
                ..
            } => vec![
                "auto".into(),
                recipe_path.display().to_string(),
                "--state".into(),
                state_path.display().to_string(),
                "--credentials-stdin".into(),
            ],
            SidecarCommand::Run {
                recipe_path,
                state_path,
                query,
            } => vec![
                "run".into(),
                recipe_path.display().to_string(),
                "--state".into(),
                state_path.display().to_string(),
                "--query".into(),
                query.clone(),
            ],
        }
    }

    /// The stdin payload for this command, if any. `Auto` sends a single JSON
    /// credentials line (G3). `Login` sends nothing here (the `done\n` resume is
    /// signalled separately by the human-in-the-loop driver).
    fn stdin_payload(&self) -> Option<String> {
        match self {
            SidecarCommand::Auto { credentials, .. } => Some(format!(
                "{}\n",
                serde_json::json!({
                    "username": credentials.username,
                    "password": credentials.password,
                })
            )),
            _ => None,
        }
    }
}

/// Parsed sidecar stdout (G1 contract). Mirrors `community_browser RunResult`.
/// `records` are kept as opaque JSON values — never serialized back into a log.
#[derive(Debug, Clone, Deserialize)]
pub struct RunResult {
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
    pub status: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub records: Vec<serde_json::Value>,
    #[serde(default)]
    pub signals: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_code: Option<String>,
}

fn default_schema_version() -> String {
    "1".to_string()
}

/// High-level outcome routed from the exit code + status (G2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    /// Exit 0 — `ok` / `logged-in`. Session captured / crawl succeeded.
    Success,
    /// Exit 10 — `needs-human` (CAPTCHA / MFA / no form). Fall back to manual.
    NeedsHuman,
    /// Exit 11 — `restricted` (paywall / access denied for this account).
    Restricted,
    /// Exit 12 — `session-expired` / `needs-login`. Re-login required.
    SessionExpired,
    /// Exit 2 — usage error (bad args / missing credentials).
    Usage,
    /// Exit 1 (or unmapped) — internal error.
    Internal,
}

impl RunOutcome {
    /// Map the child exit code to an outcome (the G2 table, owned attune-side so
    /// the contract is asserted here even if the child mislabels status text).
    pub fn from_exit_code(code: i32) -> Self {
        match code {
            0 => RunOutcome::Success,
            10 => RunOutcome::NeedsHuman,
            11 => RunOutcome::Restricted,
            12 => RunOutcome::SessionExpired,
            2 => RunOutcome::Usage,
            _ => RunOutcome::Internal,
        }
    }
}

/// SidecarController errors. Kebab-case codes for REST routing (spec §7).
#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    /// No usable sidecar binary located → capability disables (503).
    #[error("browser-tool-not-found: community-browser CLI not located (set ATTUNE_BROWSER_TOOL)")]
    ToolNotFound,
    /// Failed to spawn the located binary.
    #[error("sidecar-spawn-failed: {0}")]
    SpawnFailed(String),
    /// Run exceeded its wall-clock cap; child was killed + temp-state cleaned.
    #[error("sidecar-timeout: run exceeded {0:?}")]
    Timeout(Duration),
    /// stdout was not a single valid JSON document (G1 violation).
    #[error("sidecar-bad-output: {0}")]
    BadOutput(String),
    /// stdout JSON reported an unsupported schema_version (G1 fast-check).
    #[error("sidecar-schema-mismatch: got {got:?}, supported {supported:?}")]
    SchemaMismatch { got: String, supported: String },
    /// I/O while writing stdin / reading stdout.
    #[error("sidecar-io: {0}")]
    Io(String),
}

/// How to invoke the sidecar program. Resolved once at controller construction.
#[derive(Debug, Clone)]
pub struct SidecarProgram {
    /// The executable to run (a `community-browser` launcher, or a Python
    /// interpreter when `module_args` is set).
    pub program: PathBuf,
    /// Leading args before the subcommand — e.g. `["-m", "community_browser_automation"]`
    /// when `program` is a Python interpreter. Empty for a direct launcher.
    pub module_args: Vec<String>,
}

/// Controller that owns a resolved [`SidecarProgram`] + per-run policy.
pub struct SidecarController {
    program: SidecarProgram,
    timeout: Duration,
}

impl SidecarController {
    /// Construct with an explicitly resolved program (used by tests + after
    /// [`Self::locate`] succeeds).
    pub fn new(program: SidecarProgram) -> Self {
        SidecarController {
            program,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Override the per-run wall-clock cap.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Locate the sidecar binary cross-platform. Returns
    /// [`SidecarError::ToolNotFound`] if nothing usable is found (graceful
    /// disable, never panic).
    ///
    /// Resolution order: `ATTUNE_BROWSER_TOOL` → `community-browser` on PATH →
    /// `python -m community_browser_automation`.
    pub fn locate() -> Result<Self, SidecarError> {
        Self::locate_with(
            |k| std::env::var(k).ok(),
            |name| which::which(name).ok(),
        )
    }

    /// Locate with injected env + PATH lookup (testable, offline).
    pub fn locate_with(
        get_env: impl Fn(&str) -> Option<String>,
        which: impl Fn(&str) -> Option<PathBuf>,
    ) -> Result<Self, SidecarError> {
        // 1) Explicit override.
        if let Some(p) = get_env("ATTUNE_BROWSER_TOOL") {
            let path = PathBuf::from(p);
            if path.exists() {
                return Ok(Self::new(SidecarProgram {
                    program: path,
                    module_args: vec![],
                }));
            }
        }
        // 2) A `community-browser` launcher on PATH.
        if let Some(p) = which("community-browser") {
            return Ok(Self::new(SidecarProgram {
                program: p,
                module_args: vec![],
            }));
        }
        // 3) `python -m community_browser_automation.cli` (bundled interpreter).
        // NOTE: the package has no top-level `__main__.py`, so
        // `python -m community_browser_automation` fails — we must target the
        // `.cli` submodule, whose `if __name__ == "__main__"` block calls
        // `main()`. Verified against the real tool's `scan` smoke (the bare
        // package target prints "cannot be directly executed").
        for py in ["python3", "python"] {
            if let Some(p) = which(py) {
                return Ok(Self::new(SidecarProgram {
                    program: p,
                    module_args: vec!["-m".into(), "community_browser_automation.cli".into()],
                }));
            }
        }
        Err(SidecarError::ToolNotFound)
    }

    /// The resolved program (for diagnostics; contains no secret).
    pub fn program(&self) -> &SidecarProgram {
        &self.program
    }

    /// Run a sidecar command to completion. Enforces:
    /// - credentials over stdin only (never argv) — asserted by construction;
    /// - timeout + kill + temp-state cleanup;
    /// - G1 single-JSON stdout parse + schema fast-check;
    /// - G2 exit-code → [`RunOutcome`] routing.
    ///
    /// Returns `(RunResult, RunOutcome)` on a clean (even if `needs-human`)
    /// completion; `Err` only on infrastructure failure (spawn/timeout/bad-output).
    pub fn run(&self, cmd: SidecarCommand) -> Result<(RunResult, RunOutcome), SidecarError> {
        let state_path = cmd.state_path().cloned();
        let result = self.run_inner(&cmd);
        // temp-state cleanup runs regardless of outcome (timeout/crash/success):
        // the captured session must not linger as a plaintext file on disk.
        // (The persisted, encrypted copy lives in third_party_accounts.secret_enc;
        // this temp file is only the sidecar's working copy.)
        if let Some(sp) = state_path {
            cleanup_state_file(&sp);
        }
        result
    }

    fn run_inner(&self, cmd: &SidecarCommand) -> Result<(RunResult, RunOutcome), SidecarError> {
        let mut command = command_no_window(&self.program.program);
        command.args(&self.program.module_args);
        command.args(cmd.argv());
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        // L-3 belt-and-suspenders: scrub any inherited credential-shaped env so
        // the child can never read a leaked credential out of the environment.
        // (Credentials only flow over stdin.)
        command.env_remove("ATTUNE_BROWSER_USERNAME");
        command.env_remove("ATTUNE_BROWSER_PASSWORD");

        let mut child = command
            .spawn()
            .map_err(|e| SidecarError::SpawnFailed(e.to_string()))?;

        // Write stdin payload (credentials for Auto) then close the pipe.
        if let Some(mut payload) = cmd.stdin_payload() {
            if let Some(mut stdin) = child.stdin.take() {
                let write_res = stdin.write_all(payload.as_bytes());
                payload.zeroize(); // clear credential bytes from our heap ASAP
                drop(stdin); // close → child sees EOF
                if let Err(e) = write_res {
                    let _ = child.kill();
                    return Err(SidecarError::Io(format!("stdin write: {e}")));
                }
            }
        } else {
            // No payload: close stdin so a child that reads stdin (e.g. login's
            // done-watcher) sees EOF instead of blocking on our pipe.
            drop(child.stdin.take());
        }

        // Bounded wait with periodic poll; kill on timeout.
        let deadline = Instant::now() + self.timeout;
        loop {
            match child.try_wait() {
                Ok(Some(_status)) => break,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(SidecarError::Timeout(self.timeout));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(e) => {
                    let _ = child.kill();
                    return Err(SidecarError::Io(format!("try_wait: {e}")));
                }
            }
        }

        let output = child
            .wait_with_output()
            .map_err(|e| SidecarError::Io(format!("wait_with_output: {e}")))?;

        if output.stdout.len() > MAX_STDOUT_BYTES {
            return Err(SidecarError::BadOutput(format!(
                "stdout exceeded {MAX_STDOUT_BYTES} bytes"
            )));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let exit_code = output.status.code().unwrap_or(-1);

        let result = parse_run_result(&stdout).map_err(|e| match e {
            // Annotate which subcommand produced the malformed output (no secret).
            SidecarError::BadOutput(msg) => {
                SidecarError::BadOutput(format!("{} ({})", msg, cmd.subcommand()))
            }
            other => other,
        })?;

        // G1 fast-check: reject a major-version contract drift.
        if !schema_compatible(&result.schema_version) {
            return Err(SidecarError::SchemaMismatch {
                got: result.schema_version.clone(),
                supported: SUPPORTED_SCHEMA_VERSION.to_string(),
            });
        }

        let outcome = RunOutcome::from_exit_code(exit_code);
        Ok((result, outcome))
    }
}

/// Parse the single-JSON-document stdout (G1). Tolerates leading/trailing
/// whitespace and a possible trailing newline, but requires exactly one JSON
/// value (a child that prints logs on stdout violates the contract).
fn parse_run_result(stdout: &str) -> Result<RunResult, SidecarError> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Err(SidecarError::BadOutput("empty stdout".into()));
    }
    serde_json::from_str::<RunResult>(trimmed)
        .map_err(|e| SidecarError::BadOutput(format!("not a single JSON document: {e}")))
}

/// Major-version compatibility: the contract version "1" is compatible with
/// "1" / "1.x". A different major (e.g. "2") is rejected.
fn schema_compatible(got: &str) -> bool {
    let got_major = got.split('.').next().unwrap_or(got);
    got_major == SUPPORTED_SCHEMA_VERSION
}

/// Remove a temp `--state` file, best-effort overwriting its bytes first so the
/// captured session is not recoverable from the freed blocks.
fn cleanup_state_file(path: &std::path::Path) {
    if !path.exists() {
        return;
    }
    // Best-effort zeroize-on-disk before unlink (the file may hold a session).
    if let Ok(meta) = std::fs::metadata(path) {
        let len = meta.len();
        if len > 0 && len <= MAX_STDOUT_BYTES as u64 {
            let zeros = vec![0u8; len as usize];
            let _ = std::fs::write(path, &zeros);
        }
    }
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_routing_matches_g2_table() {
        assert_eq!(RunOutcome::from_exit_code(0), RunOutcome::Success);
        assert_eq!(RunOutcome::from_exit_code(10), RunOutcome::NeedsHuman);
        assert_eq!(RunOutcome::from_exit_code(11), RunOutcome::Restricted);
        assert_eq!(RunOutcome::from_exit_code(12), RunOutcome::SessionExpired);
        assert_eq!(RunOutcome::from_exit_code(2), RunOutcome::Usage);
        assert_eq!(RunOutcome::from_exit_code(1), RunOutcome::Internal);
        assert_eq!(RunOutcome::from_exit_code(99), RunOutcome::Internal);
    }

    #[test]
    fn parse_valid_single_json() {
        let s = r#"{"schema_version":"1","status":"ok","url":"https://x","records":[{"t":"a"}]}"#;
        let r = parse_run_result(s).unwrap();
        assert_eq!(r.status, "ok");
        assert_eq!(r.records.len(), 1);
    }

    #[test]
    fn parse_empty_stdout_is_bad_output() {
        assert!(matches!(parse_run_result("   "), Err(SidecarError::BadOutput(_))));
    }

    #[test]
    fn parse_non_json_is_bad_output() {
        assert!(matches!(
            parse_run_result("INFO: launching browser\n{...}"),
            Err(SidecarError::BadOutput(_))
        ));
    }

    #[test]
    fn schema_compat_accepts_1_and_rejects_2() {
        assert!(schema_compatible("1"));
        assert!(schema_compatible("1.3"));
        assert!(!schema_compatible("2"));
        assert!(!schema_compatible("2.0"));
    }

    #[test]
    fn argv_never_contains_credentials() {
        // Auto carries credentials, but argv must only have the stdin flag.
        let cmd = SidecarCommand::Auto {
            recipe_path: PathBuf::from("/tmp/r.json"),
            state_path: PathBuf::from("/tmp/s.json"),
            credentials: Credentials {
                username: "alice".into(),
                password: "topsecret".into(),
            },
        };
        let argv = cmd.argv();
        let joined = argv.join(" ");
        assert!(!joined.contains("alice"), "username leaked into argv: {joined}");
        assert!(!joined.contains("topsecret"), "password leaked into argv: {joined}");
        assert!(argv.iter().any(|a| a == "--credentials-stdin"));
    }

    #[test]
    fn auto_stdin_payload_is_credentials_json() {
        let cmd = SidecarCommand::Auto {
            recipe_path: PathBuf::from("/tmp/r.json"),
            state_path: PathBuf::from("/tmp/s.json"),
            credentials: Credentials {
                username: "alice".into(),
                password: "topsecret".into(),
            },
        };
        let payload = cmd.stdin_payload().unwrap();
        let v: serde_json::Value = serde_json::from_str(payload.trim()).unwrap();
        assert_eq!(v["username"], "alice");
        assert_eq!(v["password"], "topsecret");
    }

    #[test]
    fn non_auto_commands_have_no_stdin_payload() {
        let scan = SidecarCommand::Scan {
            recipe_path: PathBuf::from("/tmp/r.json"),
        };
        assert!(scan.stdin_payload().is_none());
        assert!(scan.argv().contains(&"scan".to_string()));
    }

    #[test]
    fn credentials_debug_is_redacted() {
        let c = Credentials {
            username: "alice".into(),
            password: "topsecret".into(),
        };
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("alice"));
        assert!(!dbg.contains("topsecret"));
        assert!(dbg.contains("redacted"));
    }

    #[test]
    fn tool_not_found_when_nothing_resolves() {
        let r = SidecarController::locate_with(|_| None, |_| None);
        assert!(matches!(r, Err(SidecarError::ToolNotFound)));
    }

    #[test]
    fn locate_prefers_env_override_when_exists() {
        // Point the override at a path that always exists (the crate Cargo.toml).
        let abs = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(abs.exists(), "fixture path must exist for this test");
        let abs_str = abs.display().to_string();
        let c = SidecarController::locate_with(
            move |k| {
                if k == "ATTUNE_BROWSER_TOOL" {
                    Some(abs_str.clone())
                } else {
                    None
                }
            },
            |_| None,
        )
        .expect("env override should resolve");
        assert!(c.program().module_args.is_empty());
    }

    #[test]
    fn locate_falls_back_to_python_module() {
        let c = SidecarController::locate_with(
            |_| None,
            |name| {
                if name == "python3" {
                    Some(PathBuf::from("/usr/bin/python3"))
                } else {
                    None
                }
            },
        )
        .unwrap();
        // Must target the `.cli` submodule — the bare package has no __main__.py.
        assert_eq!(
            c.program().module_args,
            vec!["-m", "community_browser_automation.cli"]
        );
    }
}
