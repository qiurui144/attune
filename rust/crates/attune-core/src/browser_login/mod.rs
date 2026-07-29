//! Browser login-assist (INT-1) — attune-side integration of the
//! `community-browser-automation` Python sidecar.
//!
//! This module is the **attune-side controller + capability** for the
//! login-walled content crawler. The heavy lifting (Playwright browser drive,
//! LLM form detection, CAPTCHA/MFA human fallback) lives in the bundled Python
//! tool; attune only **spawns it as a subprocess** and speaks the verified
//! JSON-over-CLI contract (G1-G6, community-browser-automation @ 212c957).
//!
//! ## What lives here
//!
//! - [`sidecar::SidecarController`] — locate + spawn the CLI, inject credentials
//!   over **stdin** (never argv/log, §1.4 / L-3), parse the single-JSON stdout
//!   into [`RunResult`], route by exit code, enforce timeout + kill + temp-state
//!   cleanup, and resume a human-in-the-loop login via `done\n`.
//! - [`recipe`] — recipe shape + L-7 `entry_url` SSRF/allowlist validation
//!   (reuses [`crate::net::url_guard`], defense-in-depth: attune re-validates
//!   even though the sidecar self-checks).
//!
//! ## Security posture (spec §⭐ + §9)
//!
//! - **auto-login default OFF** (L-5): the controller's default path is `scan` /
//!   `login` (human-in-the-loop). `auto` (LLM form fill) is only reachable when
//!   the caller explicitly opts in per-source.
//! - **credentials over stdin only** (L-3): [`Credentials`] is injected via the
//!   `--credentials-stdin` flag; it is never placed in argv, recipe JSON, env
//!   that survives the process, or any log line.
//! - **OutboundGate allowlist** (L-7): crawl targets must pass
//!   [`recipe::validate_entry_url`] against a user-approved host allowlist; raw
//!   IPs / internal / loopback / `file://` are refused.
//! - **session encrypted at rest**: the captured `storage_state` is stored as a
//!   `browser_login` row in `third_party_accounts.secret_enc` (AES-256-GCM dek);
//!   plaintext never touches disk or logs.

pub mod recipe;
pub mod sidecar;

pub use recipe::{validate_entry_url, LoginRecipe, RecipeError};
pub use sidecar::{
    Credentials, RunOutcome, RunResult, SidecarCommand, SidecarController, SidecarError,
    SidecarProgram,
};
