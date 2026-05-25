//! D5.7 — CLI smoke tests for `attune ocr` / `attune transcribe`.
//!
//! These tests verify the CLI's **exit-code contract** without requiring a running
//! OCR/ASR engine. They use `assert_cmd` to spawn the `attune` binary as a subprocess
//! and check exit code + stderr/stdout for known error paths:
//!
//!   exit 0 → success
//!   exit 1 → user input error (file missing, unknown profile)
//!   exit 3 → engine failure (no model loaded — most CI environments)
//!
//! These run in CI without any external dependencies (no ollama, no whisper-cli,
//! no PP-OCR model). Happy-path OCR/ASR is exercised by `office_happy_path.rs`
//! and the golden-gate tests, which gracefully skip when engines aren't available.

use assert_cmd::Command;
use predicates::prelude::*;

/// Build the `attune` binary and return a fresh `Command`. `assert_cmd` does the
/// `CARGO_BIN_EXE_<bin>` discovery for us.
fn attune_cmd() -> Command {
    Command::cargo_bin("attune").expect("attune binary should build")
}

// ─── `attune ocr` smoke ──────────────────────────────────────────────────────

/// `attune ocr <missing-file>` → exit 1, stderr mentions "image file not found".
#[test]
fn ocr_missing_image_exits_with_user_input_error() {
    attune_cmd()
        .args(["ocr", "/definitely/does/not/exist/__attune_smoke__.png"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("image file not found"));
}

/// `attune ocr --profile recipt <file>` → exit 1, stderr suggests "receipt".
///
/// Even though the file path is non-existent, the profile validation happens
/// _first_, so the user sees the typo suggestion before the file-not-found error.
/// (Both bail exit 1, but stderr should mention the profile typo.)
#[test]
fn ocr_typo_profile_suggests_nearest_match() {
    attune_cmd()
        .args(["ocr", "--profile", "recipt", "/tmp/__nonexistent__.png"])
        .assert()
        .failure()
        .code(1)
        .stderr(
            predicate::str::contains("unknown profile: 'recipt'")
                .and(predicate::str::contains("did you mean 'receipt'")),
        );
}

/// `attune ocr --profile completely_bogus <file>` → exit 1, stderr lists valid profiles
/// (no close match within edit distance 2).
#[test]
fn ocr_unknown_profile_lists_valid_set() {
    attune_cmd()
        .args(["ocr", "--profile", "completely_bogus_xyz", "/tmp/__nonexistent__.png"])
        .assert()
        .failure()
        .code(1)
        .stderr(
            predicate::str::contains("unknown profile: 'completely_bogus_xyz'")
                .and(predicate::str::contains("valid profiles:"))
                .and(predicate::str::contains("document"))
                .and(predicate::str::contains("receipt")),
        );
}

/// `attune ocr --help` → exit 0, mentions key flags.
#[test]
fn ocr_help_succeeds_and_documents_flags() {
    attune_cmd()
        .args(["ocr", "--help"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("--profile")
                .and(predicate::str::contains("--json"))
                .and(predicate::str::contains("--bbox")),
        );
}

// ─── `attune transcribe` smoke ──────────────────────────────────────────────

/// `attune transcribe <missing-audio>` → exit 1, stderr mentions "audio file not found".
#[test]
fn transcribe_missing_audio_exits_with_user_input_error() {
    attune_cmd()
        .args(["transcribe", "/definitely/does/not/exist/__attune_smoke__.wav"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("audio file not found"));
}

/// `attune transcribe --help` → exit 0, mentions key flags.
#[test]
fn transcribe_help_succeeds_and_documents_flags() {
    attune_cmd()
        .args(["transcribe", "--help"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("--diarization")
                .and(predicate::str::contains("--json"))
                .and(predicate::str::contains("--wait")),
        );
}

// ─── exit-code map sanity ────────────────────────────────────────────────────

/// `attune --help` → exit 0; binary itself is reachable.
#[test]
fn binary_is_buildable_and_help_works() {
    attune_cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Attune CLI"));
}

// ─── vault-import smoke (#61 regression) ────────────────────────────────────

/// `attune vault-import <missing-dir>` must NOT print "already exists".
/// Regression for #61: previously Vault::open_default() ran first, auto-created
/// an empty vault.db, and the import guard fired even on a fresh HOME.
///
/// We use a non-existent src dir so the process exits 1 with "not a directory",
/// which proves the guard ran BEFORE any vault.db was touched.
#[test]
fn vault_import_missing_src_does_not_report_already_exists() {
    // Isolate data_dir by overriding HOME so platform::data_dir() resolves
    // to a fresh, empty temp directory (no pre-existing vault.db).
    let tmp = tempfile::tempdir().expect("tempdir");
    attune_cmd()
        .env("HOME", tmp.path())
        .args(["vault-import", "/definitely/does/not/exist/__attune_import_smoke__"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("not a directory")
                .and(predicate::str::contains("already exists").not()),
        );
}
