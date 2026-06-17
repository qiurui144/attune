//! SPKI cert-pinning for cloud `accounts` connections (slice8 §3.2 / §5.6).
//!
//! ## Threat closed
//!
//! Without pinning, a DNS-hijack / corporate-network MITM with any CA-trusted
//! cert (or a mis-issued cert) could impersonate `accounts.engi-stack.com` and
//! intercept license activation / heartbeat signalling. Pinning the server's
//! **SPKI** (Subject Public Key Info) SHA-256 means the client only completes the
//! TLS handshake when the leaf cert's public key is one we baked in at build time.
//!
//! ## Why SPKI, not the whole cert (spec §3.2)
//!
//! Let's Encrypt rotates the leaf cert every ~90 days, but the **same key pair**
//! yields the **same SPKI**. Pinning SPKI survives routine cert rotation; only a
//! CA change or a deliberate key-pair rotation flips the pin (→ §7.2 dual-pin
//! rotation gate). Pin format: `base64(sha256(DER(SubjectPublicKeyInfo)))`.
//!
//! ## Layering — pin is an ADDITIONAL constraint, never a downgrade
//!
//! [`SpkiPinVerifier`] wraps the standard rustls webpki verifier. It runs full
//! chain + hostname + validity verification **first**; only if that passes does
//! it additionally require the leaf SPKI ∈ the pin set. A pin check that skipped
//! chain validation would be strictly weaker than normal TLS — that is an
//! anti-pattern this code deliberately avoids.
//!
//! ## Fail-safe when the pin is not yet provisioned (spec §3.2 / §10.3)
//!
//! The production SPKI must be extracted from the live server (`openssl s_client …`)
//! and pasted into [`ACCOUNTS_SPKI_PINS`] as part of the desktop release / CI pin
//! step. Until a real pin is provisioned the set is **empty**, in which case
//! pinning is **disabled** and the client falls back to standard webpki chain
//! validation (exactly the behaviour of an "old" client per §10.3 — no worse than
//! today, and avoids the §11 R1 "全量断线" catastrophe of shipping a placeholder
//! pin that matches nothing). Once a real pin lands, enforcement activates with no
//! other code change.
//!
//! This empty-set-disables behaviour applies **only** to the SPKI pin (a control
//! provisioned post-ship). It does NOT apply to the W1 plugin anchor allowlist,
//! which is fail-closed and always non-empty (see [`crate::plugin_anchor`]).

use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Pinned SPKI set for `accounts.engi-stack.com`.
///
/// Format: `base64(sha256(DER(SubjectPublicKeyInfo)))` of the TLS **leaf** cert.
///
/// Extraction (also the CI pin-verify step in `desktop-release.yml`):
/// ```text
/// openssl s_client -connect accounts.engi-stack.com:443 </dev/null \
///   | openssl x509 -pubkey -noout | openssl pkey -pubin -outform DER \
///   | openssl dgst -sha256 -binary | base64
/// ```
///
/// Rotation (spec §7.2 R1, isomorphic with the W1 anchor rotation): ≥30 days
/// before a CA / key-pair change, append the next pin here, ship a desktop
/// release carrying `[current, next]`, wait for active upgrade, then drop the old
/// pin in a later release. Upper bound 3 (old + current + new).
///
/// **Empty ⇒ pinning disabled** (fall back to standard webpki validation). The
/// production pin is provisioned at release time, not in source, to avoid a
/// placeholder that would match nothing and brick every shipped client.
pub const ACCOUNTS_SPKI_PINS: &[&str] = &[
    // Provisioned at desktop-release time from the live accounts server (see the
    // extraction command above). Empty in source = pinning disabled / std webpki.
    //
    // "PLACEHOLDER_CURRENT_SPKI_BASE64=",  // current production pin
    // "PLACEHOLDER_NEXT_SPKI_BASE64=",     // rotation window: pre-fill next pin
];

/// Compile-time upper bound on the pin set (spec §5.5: old + current + new).
pub const MAX_PINS: usize = 3;

/// Compute the pin string for a DER-encoded leaf certificate:
/// `base64(sha256(DER(SubjectPublicKeyInfo)))`.
///
/// Re-encodes the parsed SubjectPublicKeyInfo back to DER via `x509-cert`/`der`
/// so the hashed bytes are exactly the canonical SPKI DER that
/// `openssl pkey -pubin -outform DER` emits — pins are therefore comparable with
/// the CI extraction command (verified by a fixture cross-check test). Returns
/// `None` if the cert or its SPKI cannot be parsed/encoded.
pub fn spki_pin_of_cert_der(cert_der: &[u8]) -> Option<String> {
    use x509_cert::der::{Decode, Encode};
    let cert = x509_cert::Certificate::from_der(cert_der).ok()?;
    let spki_der = cert.tbs_certificate.subject_public_key_info.to_der().ok()?;
    let digest = Sha256::digest(&spki_der);
    use base64::Engine;
    Some(base64::engine::general_purpose::STANDARD.encode(digest))
}

/// Is the leaf cert's SPKI in the active pin set?
///
/// Returns `true` when [`ACCOUNTS_SPKI_PINS`] is **empty** (pinning disabled —
/// see module docs §10.3 fail-safe) OR when the cert's SPKI matches a pin.
fn cert_matches_pin(cert_der: &[u8]) -> bool {
    if ACCOUNTS_SPKI_PINS.is_empty() {
        return true; // pinning not provisioned → defer to webpki only
    }
    match spki_pin_of_cert_der(cert_der) {
        Some(pin) => ACCOUNTS_SPKI_PINS.iter().any(|p| *p == pin),
        None => false, // unparseable leaf with pinning enabled → reject
    }
}

/// A rustls [`ServerCertVerifier`] that performs full standard verification, then
/// additionally requires the leaf SPKI to match the pin set.
#[derive(Debug)]
struct SpkiPinVerifier {
    inner: Arc<rustls::client::WebPkiServerVerifier>,
}

impl rustls::client::danger::ServerCertVerifier for SpkiPinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        intermediates: &[rustls::pki_types::CertificateDer<'_>],
        server_name: &rustls::pki_types::ServerName<'_>,
        ocsp_response: &[u8],
        now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        // 1. Standard chain + hostname + validity verification (NOT bypassed).
        let verified =
            self.inner
                .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)?;
        // 2. Additional SPKI pin constraint on the leaf.
        if !cert_matches_pin(end_entity.as_ref()) {
            return Err(rustls::Error::General(
                "SPKI pin mismatch for accounts endpoint".to_string(),
            ));
        }
        Ok(verified)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// Optionally trust an operator-provided CA for self-hosted / LAN cloud, via env
/// `ATTUNE_CLOUD_CA_PEM` (a path to a PEM file, or inline PEM text). The CA is
/// **added** to the trust store; full chain + hostname + validity verification is
/// still enforced. This is NOT a verification bypass — a private/self-signed cloud
/// is reached the same secure way a public one is, just with an extra trusted
/// root. No-op when the env is unset/empty or the PEM yields no certificate.
///
/// (Replaces the former `dev-insecure-tls` skip-all-verification escape hatch:
/// trusting a specific CA is strictly safer than disabling verification.)
fn add_custom_cloud_ca(roots: &mut rustls::RootCertStore) {
    let src = match std::env::var("ATTUNE_CLOUD_CA_PEM") {
        Ok(s) if !s.trim().is_empty() => s,
        _ => return,
    };
    let pem: Vec<u8> = if std::path::Path::new(&src).is_file() {
        match std::fs::read(&src) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("ATTUNE_CLOUD_CA_PEM: cannot read file {src}: {e} — ignoring (public roots only)");
                return;
            }
        }
    } else {
        src.into_bytes()
    };
    let mut added = 0usize;
    for cert in rustls_pemfile::certs(&mut &pem[..]).flatten() {
        if roots.add(cert).is_ok() {
            added += 1;
        }
    }
    if added > 0 {
        eprintln!(
            "ATTUNE_CLOUD_CA_PEM: trusting {added} operator-provided CA cert(s) \
             (full TLS chain/hostname/validity verification still enforced)"
        );
    } else {
        eprintln!("ATTUNE_CLOUD_CA_PEM set but no valid certificate parsed — ignoring (public roots only)");
    }
}

/// Build a rustls [`rustls::ClientConfig`] that pins the accounts SPKI on top of
/// standard webpki chain validation, for injection into reqwest via
/// `ClientBuilder::use_preconfigured_tls`.
///
/// Uses the same webpki root store as the rest of the client (`webpki-roots`),
/// so non-accounts hosts (e.g. pluginhub download URLs that share this client)
/// are validated normally; the pin only adds a constraint, never removes roots.
pub fn pinned_client_config() -> rustls::ClientConfig {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    // Self-hosted / LAN cloud support (replaces the old dev-only skip-all hatch):
    // trust an operator-provided private/self-signed CA in ADDITION to public roots.
    add_custom_cloud_ca(&mut roots);

    // Pin the crypto provider explicitly (the crate is built with the `ring`
    // feature, no aws-lc-rs) so this config is self-contained and does not depend
    // on a process-default provider being installed elsewhere.
    let provider = Arc::new(rustls::crypto::ring::default_provider());

    let inner =
        rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
            .build()
            .expect("build webpki verifier from bundled roots");

    let verifier = Arc::new(SpkiPinVerifier { inner });

    rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("rustls safe default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real self-signed P-256 leaf certs (no key material kept). Their SPKI pins
    // were computed with the production `openssl` extraction command (see
    // testdata/cert_pin/PINS.txt) — embedding them here cross-checks that our
    // pure-Rust SPKI extraction matches the CI / openssl extraction byte-for-byte.
    const LEAF_A_DER: &[u8] = include_bytes!("testdata/cert_pin/leaf_a.der");
    const LEAF_B_DER: &[u8] = include_bytes!("testdata/cert_pin/leaf_b.der");
    // openssl x509 -pubkey -noout | openssl pkey -pubin -outform DER | dgst -sha256 -binary | base64
    const LEAF_A_PIN: &str = "ym929iXSKqV9+yc4/GoWrBt26WEnHt/+2KDCeEMlYPs=";
    const LEAF_B_PIN: &str = "/NxNe3XksGl1qtt7Ji+IODYYLaP/Cu/5f4O09Pp1zvY=";

    /// ★ Load-bearing: our Rust SPKI extraction must equal the `openssl` pin used
    /// by the CI pin-verify step (`desktop-release.yml`). If these diverge, a pin
    /// baked from openssl would never match the live extraction → all clients
    /// brick. Matching against the openssl-computed constant is the whole point.
    #[test]
    fn rust_spki_extraction_matches_openssl_pin() {
        assert_eq!(
            spki_pin_of_cert_der(LEAF_A_DER).as_deref(),
            Some(LEAF_A_PIN),
            "Rust SPKI pin must match the openssl-extracted pin for leaf_a"
        );
        assert_eq!(
            spki_pin_of_cert_der(LEAF_B_DER).as_deref(),
            Some(LEAF_B_PIN),
            "Rust SPKI pin must match the openssl-extracted pin for leaf_b"
        );
    }

    /// SPKI extraction is deterministic and base64(sha256) shaped.
    #[test]
    fn spki_pin_is_stable_base64_sha256() {
        let pin1 = spki_pin_of_cert_der(LEAF_A_DER).expect("extract spki");
        let pin2 = spki_pin_of_cert_der(LEAF_A_DER).expect("extract spki again");
        assert_eq!(pin1, pin2, "SPKI pin must be deterministic for the same cert");
        assert_eq!(pin1.len(), 44, "base64(sha256) is 44 chars");
        assert!(pin1.ends_with('='), "32-byte digest base64 has one pad char");
        assert!(pin1
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='));
    }

    /// Two different key pairs ⇒ two different SPKI pins (the property a MITM
    /// relies on violating; we assert it holds).
    #[test]
    fn different_keypairs_have_different_pins() {
        let pin_a = spki_pin_of_cert_der(LEAF_A_DER).unwrap();
        let pin_b = spki_pin_of_cert_der(LEAF_B_DER).unwrap();
        assert_ne!(pin_a, pin_b, "distinct key pairs must yield distinct SPKI pins");
    }

    /// Garbage bytes are not a parseable cert → no pin (and, with pinning enabled,
    /// `cert_matches_pin` would reject).
    #[test]
    fn unparseable_cert_yields_no_pin() {
        assert!(spki_pin_of_cert_der(&[0xde, 0xad, 0xbe, 0xef]).is_none());
        assert!(spki_pin_of_cert_der(&[]).is_none());
        // Truncated DER (first 20 bytes of a real cert) must not parse.
        assert!(spki_pin_of_cert_der(&LEAF_A_DER[..20]).is_none());
    }

    /// Empty pin set ⇒ pinning disabled ⇒ any cert "matches" (defers to webpki).
    /// This is the as-shipped state (pin provisioned at release time, §10.3).
    #[test]
    fn empty_pin_set_defers_to_webpki() {
        assert!(
            ACCOUNTS_SPKI_PINS.is_empty(),
            "as shipped, the pin set is empty (provisioned at release); update this \
             test AND pin_match logic expectations if a production pin is baked in"
        );
        assert!(
            cert_matches_pin(LEAF_A_DER),
            "empty pin set must defer to webpki (return true) per §10.3 fail-safe"
        );
    }

    /// When a pin IS configured, a matching cert passes and a non-matching cert
    /// is rejected. Exercised against a synthetic pin set so the test does not
    /// depend on `ACCOUNTS_SPKI_PINS` being populated in source.
    #[test]
    fn pin_match_logic_allow_vs_reject() {
        // Simulate `cert_matches_pin` with a non-empty pin set = [LEAF_A_PIN].
        let pinned = [LEAF_A_PIN];
        let matches = |der: &[u8]| -> bool {
            match spki_pin_of_cert_der(der) {
                Some(p) => pinned.iter().any(|x| *x == p),
                None => false,
            }
        };

        assert!(matches(LEAF_A_DER), "pinned cert must be allowed");
        assert!(
            !matches(LEAF_B_DER),
            "MITM cert (different key, valid CA chain) must be rejected by the pin"
        );
        assert!(
            !matches(&[0x00, 0x01]),
            "unparseable cert must be rejected when pinning is enabled"
        );
    }

    /// The pin set must stay within the §5.5 upper bound and be well-formed
    /// base64(sha256) when populated.
    #[test]
    fn pin_set_within_bound_and_well_formed() {
        assert!(
            ACCOUNTS_SPKI_PINS.len() <= MAX_PINS,
            "pin set exceeds the §5.5 upper bound of {MAX_PINS}"
        );
        for p in ACCOUNTS_SPKI_PINS {
            assert_eq!(p.len(), 44, "pin {p} is not base64(sha256) length 44");
            assert!(
                p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='),
                "pin {p} has a non-base64 char"
            );
        }
    }

    /// The pinned ClientConfig builds without panicking (bundled webpki roots
    /// resolve; verifier constructs).
    #[test]
    fn pinned_client_config_builds() {
        let _cfg = pinned_client_config();
    }

    /// ATTUNE_CLOUD_CA_PEM: an inline PEM cert is added to the trust store; when
    /// the env is unset it is a no-op. (Single test to avoid env-var races across
    /// parallel test threads.) This is the secure self-host path that replaced the
    /// removed dev-insecure-tls skip-all hatch — the CA is ADDED, verification kept.
    #[test]
    fn custom_cloud_ca_env_add_then_noop() {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(LEAF_A_DER);
        let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
        for chunk in b64.as_bytes().chunks(64) {
            pem.push_str(std::str::from_utf8(chunk).unwrap());
            pem.push('\n');
        }
        pem.push_str("-----END CERTIFICATE-----\n");

        std::env::set_var("ATTUNE_CLOUD_CA_PEM", &pem);
        let mut roots = rustls::RootCertStore::empty();
        add_custom_cloud_ca(&mut roots);
        assert_eq!(roots.len(), 1, "inline PEM CA must be added to the trust store");

        std::env::remove_var("ATTUNE_CLOUD_CA_PEM");
        let mut roots2 = rustls::RootCertStore::empty();
        add_custom_cloud_ca(&mut roots2);
        assert_eq!(roots2.len(), 0, "unset env ⇒ no roots added (no-op)");
    }
}
