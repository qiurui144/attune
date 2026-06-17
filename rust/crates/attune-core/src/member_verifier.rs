//! Member-paid verification — server-side proof that a "paid" claim is real.
//!
//! ## Why this exists (C1 paywall-bypass fix, §5.2.0b adversarial review)
//!
//! `POST /api/v1/member/login-token` used to set [`MemberState::Paid`](crate::member_session::MemberState::Paid)
//! purely on a client-asserted `{tier:"paid", license_id:"<any-non-empty>"}` — a non-empty
//! string was the ONLY check. doc-intel is the first feature to gate **billable cloud-LLM
//! spend** on `MemberState::is_paid()`, so a forged claim translated directly into spend on
//! someone else's gateway token. The Paid state must be **earned by verification**, not asserted.
//!
//! There is no Ed25519-signed offline license artifact in attune: a `License.license_key` is an
//! opaque string minted by the cloud accounts server, and the authoritative check is "does this
//! license actually belong to the authenticated session, and is it un-revoked?". This module
//! provides that check via the persisted cloud session.
//!
//! ## Contract (fail-closed — the load-bearing security property)
//!
//! [`MemberVerifier::verify_paid`] returns `Ok(())` **only** when the claimed `license_id` is
//! confirmed against the server-side source of truth. EVERY other path — empty license, no
//! persisted cloud session, network failure, license absent from the account, revoked license —
//! returns `Err(..)`. The caller MUST NOT grant Paid on `Err`. There is no fail-open branch.

use crate::cloud_client::CloudClient;

/// Reasons a paid claim could not be verified. All variants ⇒ the caller must NOT grant Paid.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MemberVerifyError {
    /// The request carried no (or an empty) `license_id`.
    #[error("missing-license-id: a paid claim requires a non-empty license_id")]
    MissingLicenseId,
    /// No persisted cloud session on this device — a paid claim cannot be verified.
    /// The credentialed path to Paid is `POST /member/login-password`.
    #[error("no-cloud-session: no authenticated cloud session to verify the license against")]
    NoCloudSession,
    /// The cloud could not be reached / returned an error. **Fail closed.**
    #[error("verification-unavailable: could not reach the cloud to verify the license: {0}")]
    Unavailable(String),
    /// The session is valid but the claimed license is not present on the account.
    #[error("license-not-on-account: the claimed license is not owned by this session")]
    LicenseNotOnAccount,
    /// The license exists but is revoked.
    #[error("license-revoked: the claimed license has been revoked")]
    LicenseRevoked,
}

/// Server-side verifier for a "paid" membership claim.
///
/// Implementors confirm the claim against an authoritative source. The trait exists so tests can
/// inject a verifier that performs a *real* (deterministic, offline) match — NOT a blanket
/// `return Ok` — so the member-gate path is genuinely exercised without a live cloud.
pub trait MemberVerifier: Send + Sync {
    /// Verify that `license_id` is a real, un-revoked license owned by the current session.
    ///
    /// `Ok(())` ⇒ the caller may grant Paid. Any `Err` ⇒ the caller MUST NOT grant Paid.
    fn verify_paid(&self, account_id: &str, license_id: &str) -> Result<(), MemberVerifyError>;
}

/// Loader for the persisted cloud session, abstracted so the production verifier is unit-testable
/// without touching the real `config_dir()/cloud-session.json` on disk.
pub trait CloudSessionSource: Send + Sync {
    /// Returns `(cloud_url, session_token)` if a persisted, non-empty session exists.
    fn load(&self) -> Option<(String, String)>;
}

/// Production [`CloudSessionSource`] — reads `config_dir()/cloud-session.json` (same file
/// `plugin_sync`/`login_token` already use).
pub struct DiskCloudSession;

impl CloudSessionSource for DiskCloudSession {
    fn load(&self) -> Option<(String, String)> {
        let path = crate::platform::config_dir().join("cloud-session.json");
        let json = std::fs::read_to_string(&path).ok()?;
        let sess: serde_json::Value = serde_json::from_str(&json).ok()?;
        let cloud_url = sess.get("cloud_url").and_then(|v| v.as_str())?;
        let session = sess
            .get("session")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())?;
        Some((cloud_url.to_string(), session.to_string()))
    }
}

/// The production verifier: confirms the license against the cloud accounts server using the
/// persisted session. **Fail-closed** on every error path.
pub struct CloudMemberVerifier<S: CloudSessionSource = DiskCloudSession> {
    session: S,
}

impl Default for CloudMemberVerifier<DiskCloudSession> {
    fn default() -> Self {
        Self { session: DiskCloudSession }
    }
}

impl<S: CloudSessionSource> CloudMemberVerifier<S> {
    pub fn new(session: S) -> Self {
        Self { session }
    }
}

impl<S: CloudSessionSource> MemberVerifier for CloudMemberVerifier<S> {
    fn verify_paid(&self, _account_id: &str, license_id: &str) -> Result<(), MemberVerifyError> {
        if license_id.trim().is_empty() {
            return Err(MemberVerifyError::MissingLicenseId);
        }
        // No persisted cloud session ⇒ nothing to verify against ⇒ fail closed.
        let (cloud_url, session) = self.session.load().ok_or(MemberVerifyError::NoCloudSession)?;

        // Ask the authoritative source: list the account's licenses. Any transport/HTTP error is
        // treated as "unavailable" and fails closed — an attacker who blocks the cloud must NOT be
        // rewarded with Paid.
        let client = CloudClient::with_session(&cloud_url, &session);
        let licenses = client
            .list_licenses()
            .map_err(|e| MemberVerifyError::Unavailable(e.to_string()))?;

        // Match by license_key OR numeric id (login_token receives the cloud-issued license id as a
        // string; login_password sets `license_id = selected.id.to_string()`). Match both shapes.
        let claimed = license_id.trim();
        let found = licenses.iter().find(|lic| {
            lic.license_key == claimed
                || lic.id.to_string() == claimed
                || lic.license_id.map(|i| i.to_string()).as_deref() == Some(claimed)
        });
        let lic = found.ok_or(MemberVerifyError::LicenseNotOnAccount)?;

        // Revoked licenses never grant Paid.
        if lic.revoked_at.is_some() {
            return Err(MemberVerifyError::LicenseRevoked);
        }
        Ok(())
    }
}

/// A verifier that approves a single explicitly-whitelisted `license_id` — for tests and the
/// eval harness. It performs a REAL match (`claimed == expected`), so a test that reaches Paid
/// still goes through a verification step rather than a blanket client claim. A forged or empty
/// license is rejected exactly like production.
///
/// This deliberately lives in non-test code (not `#[cfg(test)]`) so integration tests in other
/// crates can construct it; it grants Paid for ONE known license only.
pub struct WhitelistMemberVerifier {
    expected_license: String,
}

impl WhitelistMemberVerifier {
    /// Approve exactly `expected_license` (and nothing else).
    pub fn new(expected_license: impl Into<String>) -> Self {
        Self { expected_license: expected_license.into() }
    }
}

impl MemberVerifier for WhitelistMemberVerifier {
    fn verify_paid(&self, _account_id: &str, license_id: &str) -> Result<(), MemberVerifyError> {
        let claimed = license_id.trim();
        if claimed.is_empty() {
            return Err(MemberVerifyError::MissingLicenseId);
        }
        if claimed == self.expected_license {
            Ok(())
        } else {
            Err(MemberVerifyError::LicenseNotOnAccount)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session source that yields nothing — models "no persisted cloud session on this device".
    struct NoSession;
    impl CloudSessionSource for NoSession {
        fn load(&self) -> Option<(String, String)> {
            None
        }
    }

    /// A session source pointing at an unreachable host — models "cloud unreachable / blocked".
    struct UnreachableSession;
    impl CloudSessionSource for UnreachableSession {
        fn load(&self) -> Option<(String, String)> {
            Some(("http://127.0.0.1:1".into(), "session-not-real".into()))
        }
    }

    #[test]
    fn empty_license_id_is_rejected_missing() {
        let v = CloudMemberVerifier::new(NoSession);
        assert_eq!(
            v.verify_paid("acct", "").unwrap_err(),
            MemberVerifyError::MissingLicenseId
        );
        assert_eq!(
            v.verify_paid("acct", "   ").unwrap_err(),
            MemberVerifyError::MissingLicenseId,
            "whitespace-only is empty"
        );
    }

    #[test]
    fn no_cloud_session_fails_closed() {
        // The exact production scenario in the eval harness (tempdir HOME, no cloud-session.json):
        // a non-empty license claim with no session MUST be rejected — never granted Paid.
        let v = CloudMemberVerifier::new(NoSession);
        assert_eq!(
            v.verify_paid("acct", "lic-test").unwrap_err(),
            MemberVerifyError::NoCloudSession
        );
    }

    #[test]
    fn cloud_unreachable_fails_closed_not_open() {
        // An attacker who firewalls the cloud host must NOT be rewarded with Paid: a transport
        // error maps to Unavailable (Err), not a silent grant.
        let v = CloudMemberVerifier::new(UnreachableSession);
        let err = v.verify_paid("acct", "lic-claimed").unwrap_err();
        assert!(
            matches!(err, MemberVerifyError::Unavailable(_)),
            "unreachable cloud must fail closed (Unavailable), got {err:?}"
        );
    }

    #[test]
    fn whitelist_verifier_approves_only_the_expected_license() {
        let v = WhitelistMemberVerifier::new("lic-known");
        // The one whitelisted license passes — a real match, not a blanket Ok.
        assert!(v.verify_paid("acct", "lic-known").is_ok());
        assert!(v.verify_paid("acct", " lic-known ").is_ok(), "trims whitespace");
        // Anything else is rejected exactly like production.
        assert_eq!(
            v.verify_paid("acct", "lic-other").unwrap_err(),
            MemberVerifyError::LicenseNotOnAccount
        );
        assert_eq!(
            v.verify_paid("acct", "").unwrap_err(),
            MemberVerifyError::MissingLicenseId
        );
    }
}
