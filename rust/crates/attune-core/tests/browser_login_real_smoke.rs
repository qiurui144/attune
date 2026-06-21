//! INT-1 real-tool smoke (§1.6 pure-offline-ish: drives the real
//! community-browser-automation CLI, no real login / no member site — just
//! `scan https://example.com` to prove the JSON-over-CLI contract binds to the
//! REAL tool, not a fake script).
//!
//! `#[ignore]` by default (needs the tool installed + a browser). Run with:
//!   ATTUNE_BROWSER_TOOL=/path/to/community-browser \
//!     cargo test -p attune-core --test browser_login_real_smoke -- --ignored --nocapture
//!
//! The wrapper used in the recorded smoke set PYTHONPATH=.../src and invoked
//! `community_browser_automation.cli:main` (= the console_script entry point).

#![cfg(unix)]

use std::path::PathBuf;
use std::time::Duration;

use attune_core::browser_login::{RunOutcome, SidecarCommand, SidecarController};

#[test]
#[ignore = "real tool + browser required; run explicitly with ATTUNE_BROWSER_TOOL set"]
fn real_scan_binds_json_over_cli_contract() {
    let recipe = std::env::var("ATTUNE_SMOKE_RECIPE")
        .unwrap_or_else(|_| "/data/tmp/smoke-recipe.json".to_string());
    let ctrl = SidecarController::locate()
        .expect("ATTUNE_BROWSER_TOOL must point at the community-browser launcher")
        .with_timeout(Duration::from_secs(60));

    let (result, outcome) = ctrl
        .run(SidecarCommand::Scan {
            recipe_path: PathBuf::from(recipe),
        })
        .expect("real scan should complete and parse as a single JSON document");

    // The real tool's G1 stdout deserialized into our RunResult.
    assert_eq!(result.schema_version, "1", "schema_version contract");
    assert!(!result.status.is_empty(), "status present");
    // For a public, no-login page the real tool returns ok / exit 0.
    assert_eq!(outcome, RunOutcome::from_exit_code(0));
    println!(
        "REAL SMOKE OK: status={} url={} schema={}",
        result.status, result.url, result.schema_version
    );
}
