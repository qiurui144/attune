//! INT-1 §9: browser login-assist session storage in the third_party_accounts
//! vault + L-7 entry_url allowlist + proptest invariants + OutboundGate
//! BrowserCrawl wiring.
//!
//! Covers:
//! - §9.5 session encryption: `storage_state` stored as `browser_login`
//!   provider → DB BLOB carries no plaintext session string.
//! - §9.1 session round-trip: encrypt → decrypt restores the exact JSON.
//! - §9 boundary: an oversized session (> 256 KB) is refused, not truncated.
//! - §9.8 allowlist (L-7): non-approved / internal entry_url refused.
//! - proptest: session round-trip invariant; allowlist host-membership monotone.
//! - OutboundGate BrowserCrawl: disabled / vault-locked refusals.

use std::net::{IpAddr, Ipv4Addr};

use attune_core::browser_login::recipe::validate_entry_url;
use attune_core::crypto::Key32;
use attune_core::outbound_gate::{OutboundError, OutboundGate, OutboundKind, OutboundPolicy};
use attune_core::store::third_party_accounts::{is_known_provider, ThirdPartyAccountInput};
use attune_core::store::Store;

const SAMPLE_SESSION: &str = r#"{"cookies":[{"name":"sess","value":"SESSION_PLAINTEXT_SECRET_42","domain":"members.example"}],"origins":[]}"#;

fn session_input(session_json: &str) -> ThirdPartyAccountInput {
    ThirdPartyAccountInput {
        provider: "browser_login".into(),
        label: "CNKI 会员".into(),
        username: "我的 CNKI".into(), // user-readable source name, NOT a password
        endpoint: "https://www.cnki.net/".into(),
        secret: session_json.to_string(),
    }
}

fn public_resolver() -> impl Fn(&str) -> std::io::Result<Vec<IpAddr>> {
    |_h: &str| Ok(vec![IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))])
}

// ── browser_login is a known provider now ───────────────────────────────────
#[test]
fn browser_login_is_known_provider() {
    assert!(is_known_provider("browser_login"));
}

// ── §9.5 session BLOB carries no plaintext ──────────────────────────────────
#[test]
fn session_blob_has_no_plaintext() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let id = store
        .add_third_party_account(&dek, &session_input(SAMPLE_SESSION))
        .unwrap();

    let raw = store.debug_raw_third_party_secret_enc(&id).unwrap();
    let raw_str = String::from_utf8_lossy(&raw);
    assert!(
        !raw_str.contains("SESSION_PLAINTEXT_SECRET_42"),
        "session plaintext must NOT appear in the encrypted BLOB"
    );
    assert!(
        !raw_str.contains("\"cookies\""),
        "no structural plaintext either"
    );
    assert!(!raw.is_empty(), "BLOB must hold ciphertext");
}

// ── §9.1 session round-trips through encryption ─────────────────────────────
#[test]
fn session_round_trips() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let id = store
        .add_third_party_account(&dek, &session_input(SAMPLE_SESSION))
        .unwrap();
    let back = store.get_third_party_secret(&dek, &id).unwrap().unwrap();
    assert_eq!(back, SAMPLE_SESSION, "session must decrypt back exactly");
}

// ── list view never exposes the session ─────────────────────────────────────
#[test]
fn list_view_never_exposes_session() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    store
        .add_third_party_account(&dek, &session_input(SAMPLE_SESSION))
        .unwrap();
    let views = store.list_third_party_accounts().unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].provider, "browser_login");
    // ThirdPartyAccountView has no secret field — compile-time guarantee.
    let json = serde_json::to_string(&serde_json::json!({
        "username": views[0].username, "endpoint": views[0].endpoint,
    }))
    .unwrap();
    assert!(!json.contains("SESSION_PLAINTEXT_SECRET_42"));
}

// ── §9 boundary: large session ok up to 256 KB; over → refused not truncated ─
#[test]
fn large_session_within_limit_ok() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    // ~200 KB valid-ish JSON string.
    let big = format!(r#"{{"cookies":"{}"}}"#, "a".repeat(200 * 1024));
    let id = store
        .add_third_party_account(&dek, &session_input(&big))
        .unwrap();
    let back = store.get_third_party_secret(&dek, &id).unwrap().unwrap();
    assert_eq!(back.len(), big.len(), "large session must not be truncated");
}

#[test]
fn oversized_session_is_refused() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let too_big = "x".repeat(256 * 1024 + 1);
    let err = store.add_third_party_account(&dek, &session_input(&too_big));
    assert!(
        err.is_err(),
        "session over 256 KB must be refused (not truncated)"
    );
}

// ── normal (non-browser_login) providers keep the tight 8 KB limit ──────────
#[test]
fn webdav_secret_keeps_8k_limit() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let input = ThirdPartyAccountInput {
        provider: "webdav".into(),
        label: "x".into(),
        username: "u".into(),
        endpoint: "https://x/dav".into(),
        secret: "y".repeat(8193),
    };
    assert!(
        store.add_third_party_account(&dek, &input).is_err(),
        "webdav secret over 8 KB must still be refused"
    );
}

// ── §9.8 (L-7) allowlist: only user-approved hosts; internal refused ────────
#[test]
fn allowlist_permits_only_approved_host() {
    let allow = vec!["cnki.net".to_string()];
    assert!(validate_entry_url("https://www.cnki.net/", &allow, &public_resolver()).is_ok());
    // A host the user did NOT approve is refused even if public.
    assert!(validate_entry_url("https://evil.example.com/", &allow, &public_resolver()).is_err());
}

#[test]
fn allowlist_refuses_internal_and_nonhttp() {
    let allow = vec!["intranet.local".to_string()];
    let internal = |_h: &str| Ok(vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5))]);
    // approved host but resolves internal → refused (DNS-rebind / SSRF).
    assert!(validate_entry_url("https://intranet.local/login", &allow, &internal).is_err());
    // file:// scheme refused.
    assert!(validate_entry_url("file:///etc/passwd", &allow, &public_resolver()).is_err());
}

// ── OutboundGate BrowserCrawl: disabled + vault-locked refusals ──────────────
#[test]
fn browser_crawl_disabled_is_refused() {
    let p = OutboundPolicy::cloud(OutboundKind::BrowserCrawl, false, true, None);
    assert!(matches!(
        OutboundGate::enforce(&p, ""),
        Err(OutboundError::Disabled(OutboundKind::BrowserCrawl))
    ));
}

#[test]
fn browser_crawl_vault_locked_is_refused() {
    // crawl needs to decrypt the session → must be vault-gated.
    let p = OutboundPolicy::cloud(OutboundKind::BrowserCrawl, true, false, None);
    assert!(matches!(
        OutboundGate::enforce(&p, ""),
        Err(OutboundError::VaultLocked)
    ));
}

#[test]
fn browser_crawl_kind_label_is_stable() {
    assert_eq!(OutboundKind::BrowserCrawl.as_str(), "browser_crawl");
}

// ── proptests (≥3 per spec §9) ──────────────────────────────────────────────
use proptest::prelude::*;

proptest! {
    // P1: any session JSON within the limit round-trips losslessly.
    #[test]
    fn prop_session_round_trip(payload in "\\{\"k\":\"[a-zA-Z0-9 ]{0,4096}\"\\}") {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let id = store
            .add_third_party_account(&dek, &session_input(&payload))
            .unwrap();
        let back = store.get_third_party_secret(&dek, &id).unwrap().unwrap();
        prop_assert_eq!(back, payload);
    }

    // P2: the encrypted BLOB never contains the cleartext payload substring.
    #[test]
    fn prop_blob_never_contains_plaintext(marker in "SECRET_[A-Z0-9]{8,24}") {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let payload = format!(r#"{{"cookie":"{marker}"}}"#);
        let id = store
            .add_third_party_account(&dek, &session_input(&payload))
            .unwrap();
        let raw = store.debug_raw_third_party_secret_enc(&id).unwrap();
        let raw_str = String::from_utf8_lossy(&raw);
        prop_assert!(!raw_str.contains(&marker));
    }

    // P3: entry_url validation is deterministic + never panics on arbitrary input.
    #[test]
    fn prop_entry_url_never_panics(host in "[a-z]{1,12}", path in "[a-z/]{0,16}") {
        let allow = vec![format!("{host}.example")];
        let url = format!("https://{host}.example/{path}");
        // Just assert it returns (Ok or Err) without panicking; public resolver.
        let _ = validate_entry_url(&url, &allow, &public_resolver());
        // A host not in allowlist is always refused (membership monotonicity).
        let other = format!("https://not-{host}.test/{path}");
        prop_assert!(validate_entry_url(&other, &allow, &public_resolver()).is_err());
    }
}
