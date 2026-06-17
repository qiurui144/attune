//! W1 plugin trust-anchor allowlist (cloud slice8 §5.6.1 / §5.6 contract).
//!
//! ## Why this exists — the threat W1 closes
//!
//! Cert-pinning (`cloud_client::ACCOUNTS_SPKI_PINS`) stops a network MITM that
//! tampers with an entitlement's `signing_pubkey_hex` on the wire. It does
//! **not** stop a *compromised accounts server* from serving an attacker's
//! pubkey over a perfectly valid TLS endpoint: the server is the legitimate TLS
//! peer, so the pin matches. With only cert-pinning, a breached server could
//! swap `signing_pubkey_hex` for the attacker's key; desktop auto-install (login
//! flow) would then accept an attacker-signed plugin — arbitrary agent code
//! running with the user's trust.
//!
//! W1 is the **only** control that defends against a compromised server: desktop
//! refuses to trust any signing key that is not in a compile-time-baked
//! allowlist. Cert-pin and W1 are orthogonal and stack — neither alone is
//! sufficient (TB-chain in spec §5.6).
//!
//! ## Contract (spec §5.6.1)
//!
//! - `OFFICIAL_PLUGIN_ANCHORS`: compiled into the read-only code segment, **not**
//!   in the mutable vault. A breached vault cannot widen the trust root.
//! - Rotation: dual-anchor transition `[old, new]`, isomorphic with the SPKI pin
//!   rotation; **upper bound 3** anchors.
//! - W1-B cross-check: before `verify_with_key`, `plugin_sync` must assert the
//!   entitlement's `signing_pubkey_hex` ∈ `OFFICIAL_PLUGIN_ANCHORS`. Miss →
//!   refuse install (fail-closed) and surface an `anchor-not-pinned` reason.
//!
//! ## Fail-closed
//!
//! This is a trust-decision gate: a miss is **rejection**, never a warning that
//! lets install proceed. The official anchor list is non-empty by construction
//! (the law-pro publisher anchor is the W1 source, spec §12), so there is no
//! "empty allowlist disables the check" mode — that would be fail-open and is
//! explicitly forbidden for the anchor gate (unlike the SPKI pin, which is
//! provisioned later; see `cloud_client::ACCOUNTS_SPKI_PINS`).

/// Official plugin signing-key allowlist — the immutable trust root for
/// auto-installed plugins. SSOT mirror of cloud `accounts/config.py`
/// `OFFICIAL_PLUGIN_ANCHORS` (W1-C). Each entry is the 64-char lowercase hex of
/// an Ed25519 verifying key.
///
/// Rotation: prepend/append the new anchor during the transition window (max 3
/// total), ship a desktop release with `[old, new]`, wait for active upgrade,
/// then drop the old anchor in a later release (§5.6.1 W1-D, isomorphic with the
/// §7.2 dual-pin CA-rotation gate).
pub const OFFICIAL_PLUGIN_ANCHORS: &[&str] = &[
    // 官方锚 2026-06-05 — law-pro publisher key (cloud accounts/config.py SSOT).
    "8866ae9b8f0026aaa99902a34fa06223b5e88d5a8f933c7f084342cb9953bcac",
    // "新锚..."  // rotation window: pre-fill the next anchor here (≤ 3 total).
];

/// Compile-time upper bound on the anchor list (spec §5.6.1: "上限 3").
pub const MAX_ANCHORS: usize = 3;

/// W1-B cross-check: is `signing_pubkey_hex` a pinned official anchor?
///
/// Case-insensitive on the hex (callers may pass upper/lower). Returns `true`
/// only on an exact, case-folded match against an entry in
/// [`OFFICIAL_PLUGIN_ANCHORS`]. An empty or malformed input is **not** a match
/// (fail-closed).
pub fn is_official_anchor(signing_pubkey_hex: &str) -> bool {
    let candidate = signing_pubkey_hex.trim();
    if candidate.is_empty() {
        return false;
    }
    OFFICIAL_PLUGIN_ANCHORS
        .iter()
        .any(|anchor| anchor.eq_ignore_ascii_case(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_anchor_matches_exact() {
        assert!(is_official_anchor(
            "8866ae9b8f0026aaa99902a34fa06223b5e88d5a8f933c7f084342cb9953bcac"
        ));
    }

    #[test]
    fn official_anchor_is_case_insensitive() {
        // entitlement may arrive upper-cased from a different serializer.
        assert!(is_official_anchor(
            "8866AE9B8F0026AAA99902A34FA06223B5E88D5A8F933C7F084342CB9953BCAC"
        ));
    }

    #[test]
    fn official_anchor_tolerates_surrounding_whitespace() {
        assert!(is_official_anchor(
            "  8866ae9b8f0026aaa99902a34fa06223b5e88d5a8f933c7f084342cb9953bcac\n"
        ));
    }

    #[test]
    fn unknown_key_is_rejected() {
        // Attacker-substituted pubkey (compromised-server threat) — must miss.
        assert!(!is_official_anchor(
            "0000000000000000000000000000000000000000000000000000000000000000"
        ));
        assert!(!is_official_anchor(
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef00"
        ));
    }

    #[test]
    fn empty_is_rejected_fail_closed() {
        // Fail-closed: a missing/blank signing key never resolves to "trusted".
        assert!(!is_official_anchor(""));
        assert!(!is_official_anchor("   "));
        assert!(!is_official_anchor("\t\n"));
    }

    #[test]
    fn near_miss_off_by_one_char_is_rejected() {
        // One-character flip from the real anchor — must NOT match (no fuzzy).
        assert!(!is_official_anchor(
            "9866ae9b8f0026aaa99902a34fa06223b5e88d5a8f933c7f084342cb9953bcac"
        ));
        // Truncated.
        assert!(!is_official_anchor("8866ae9b"));
        // Trailing extra char.
        assert!(!is_official_anchor(
            "8866ae9b8f0026aaa99902a34fa06223b5e88d5a8f933c7f084342cb9953bcacf"
        ));
    }

    #[test]
    fn anchor_list_within_bound_and_well_formed() {
        // Contract invariants on the baked allowlist itself.
        assert!(
            !OFFICIAL_PLUGIN_ANCHORS.is_empty(),
            "anchor allowlist must be non-empty (fail-closed gate has no trusted keys otherwise)"
        );
        assert!(
            OFFICIAL_PLUGIN_ANCHORS.len() <= MAX_ANCHORS,
            "anchor list exceeds the §5.6.1 upper bound of {MAX_ANCHORS}"
        );
        for a in OFFICIAL_PLUGIN_ANCHORS {
            assert_eq!(a.len(), 64, "anchor {a} is not 64 hex chars (Ed25519 vk)");
            assert!(
                a.chars().all(|c| c.is_ascii_hexdigit()),
                "anchor {a} has a non-hex char"
            );
            assert!(
                a.chars().all(|c| !c.is_ascii_uppercase()),
                "anchor {a} should be stored lowercase (SSOT canonical form)"
            );
        }
    }
}
