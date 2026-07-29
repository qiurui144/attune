//! Login-assist recipe shape + L-7 `entry_url` validation.
//!
//! A recipe describes a site's login flow + extraction rules. It is the JSON
//! handed to the sidecar (`scan`/`login`/`auto`/`run`). **It never carries
//! credential values** — only env-var *names* (mirroring the source tool's
//! `CredentialSpec` env-var-only model). attune injects real credentials over
//! stdin at run time (see [`super::sidecar`]); the recipe written to disk and
//! the recipe stored in DB are credential-free by construction.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::net::url_guard::{system_resolve, validate_outbound_url};

/// Recipe validation / serialization errors. Kebab-case codes for REST routing.
#[derive(Debug, thiserror::Error)]
pub enum RecipeError {
    /// `entry_url` failed L-7 validation (non-http(s) / internal / not-allowlisted).
    #[error("invalid-entry-url: {0}")]
    InvalidEntryUrl(String),
    /// The recipe JSON appears to embed a credential value (defense-in-depth).
    #[error("recipe-contains-credential: recipe JSON must not embed credential values")]
    RecipeContainsCredential,
    /// JSON (de)serialization failure.
    #[error("recipe-json: {0}")]
    Json(String),
}

/// Site login/captcha/success signal selectors (mirrors source tool `SiteSignals`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SiteSignals {
    #[serde(default)]
    pub login_selectors: Vec<String>,
    #[serde(default)]
    pub captcha_selectors: Vec<String>,
    #[serde(default)]
    pub restricted_text: Vec<String>,
    #[serde(default)]
    pub success_selectors: Vec<String>,
    #[serde(default)]
    pub success_url_contains: Vec<String>,
}

/// A single extraction rule (CSS selector → field, mirrors source tool `ExtractRule`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtractRule {
    pub name: String,
    pub selector: String,
    #[serde(default = "default_attr")]
    pub attr: String,
    #[serde(default)]
    pub many: bool,
}

fn default_attr() -> String {
    "text".to_string()
}

/// Credential **env-var-name** spec — never holds values (L-3).
///
/// attune does not rely on env at all; this exists only to satisfy the source
/// tool's recipe schema. The real credential injection path is stdin
/// ([`super::sidecar::Credentials`]). When attune drives `auto`, it passes
/// `--credentials-stdin` and these fields are ignored by the CLI.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialSpec {
    #[serde(default)]
    pub username_env: String,
    #[serde(default)]
    pub password_env: String,
}

/// The login-assist recipe handed to the sidecar. Credential-free.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoginRecipe {
    pub id: String,
    pub name: String,
    pub start_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub login_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<CredentialSpec>,
    #[serde(default)]
    pub signals: SiteSignals,
    #[serde(default)]
    pub extract: Vec<ExtractRule>,
}

impl LoginRecipe {
    /// Serialize to JSON, asserting first that no credential value leaked in.
    ///
    /// The "no credential" check is **structural**: a [`CredentialSpec`] only
    /// holds env-var *names*. This guard is defense-in-depth against a future
    /// field accidentally carrying a value.
    pub fn to_json(&self) -> Result<String, RecipeError> {
        // Structural guard: credentials, if present, must be env-name-only.
        if let Some(c) = &self.credentials {
            // Heuristic: env var names are SCREAMING_SNAKE_CASE identifiers; a
            // value with spaces / '@' / ':' / long mixed-case is suspicious.
            if looks_like_secret_value(&c.username_env) || looks_like_secret_value(&c.password_env)
            {
                return Err(RecipeError::RecipeContainsCredential);
            }
        }
        serde_json::to_string(self).map_err(|e| RecipeError::Json(e.to_string()))
    }

    /// Parse a recipe from JSON.
    pub fn from_json(s: &str) -> Result<Self, RecipeError> {
        serde_json::from_str(s).map_err(|e| RecipeError::Json(e.to_string()))
    }
}

/// Whether a string looks like a credential *value* rather than an env-var name.
/// Env var names are `[A-Z0-9_]+`; anything else (spaces, lowercase mix, '@',
/// ':', '/') is treated as a leaked value and refused.
fn looks_like_secret_value(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    !s.chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// L-7: validate a crawl `entry_url` against a user-approved host allowlist +
/// SSRF guard. Reuses [`validate_outbound_url`] (http(s)-only, raw-IP refused,
/// loopback/private/link-local/metadata refused, DNS-rebind mitigated).
///
/// `allowlist` is the set of hosts the user explicitly approved for this source
/// (defense-in-depth: attune validates even though the sidecar self-checks).
/// `resolve` is injected (production = [`system_resolve`]; tests inject a fixed
/// map for offline determinism).
pub fn validate_entry_url(
    entry_url: &str,
    allowlist: &[String],
    resolve: &dyn Fn(&str) -> std::io::Result<Vec<IpAddr>>,
) -> Result<(), RecipeError> {
    validate_outbound_url(entry_url, allowlist, resolve)
        .map(|_| ())
        .map_err(|e| RecipeError::InvalidEntryUrl(e.to_string()))
}

/// Convenience: validate with the production system resolver.
pub fn validate_entry_url_prod(entry_url: &str, allowlist: &[String]) -> Result<(), RecipeError> {
    validate_entry_url(entry_url, allowlist, &system_resolve)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn resolves_to(ip: IpAddr) -> impl Fn(&str) -> std::io::Result<Vec<IpAddr>> {
        move |_h: &str| Ok(vec![ip])
    }
    fn public_ip() -> impl Fn(&str) -> std::io::Result<Vec<IpAddr>> {
        resolves_to(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)))
    }

    fn sample_recipe() -> LoginRecipe {
        LoginRecipe {
            id: "cnki".into(),
            name: "CNKI".into(),
            start_url: "https://www.cnki.net/".into(),
            login_url: String::new(),
            credentials: Some(CredentialSpec {
                username_env: "CNKI_USERNAME".into(),
                password_env: "CNKI_PASSWORD".into(),
            }),
            signals: SiteSignals {
                login_selectors: vec!["text=登录".into()],
                success_selectors: vec!["text=退出".into()],
                ..Default::default()
            },
            extract: vec![ExtractRule {
                name: "title".into(),
                selector: "a.fz14".into(),
                attr: "text".into(),
                many: true,
            }],
        }
    }

    #[test]
    fn recipe_round_trips_through_json() {
        let r = sample_recipe();
        let json = r.to_json().unwrap();
        let back = LoginRecipe::from_json(&json).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn recipe_json_never_contains_credential_value() {
        // The env-name-only recipe serializes with no value-like strings.
        let r = sample_recipe();
        let json = r.to_json().unwrap();
        assert!(json.contains("CNKI_USERNAME"), "env name kept");
        assert!(!json.contains("password123"), "no value leaks");
        // A recipe that smuggled a real value into the env field is refused.
        let mut bad = sample_recipe();
        bad.credentials = Some(CredentialSpec {
            username_env: "alice@example.com".into(), // value, not env name
            password_env: "PASS".into(),
        });
        assert!(matches!(
            bad.to_json(),
            Err(RecipeError::RecipeContainsCredential)
        ));
    }

    #[test]
    fn entry_url_allowlisted_public_host_ok() {
        let allow = vec!["cnki.net".to_string()];
        assert!(validate_entry_url("https://www.cnki.net/", &allow, &public_ip()).is_ok());
    }

    #[test]
    fn entry_url_not_in_allowlist_refused() {
        // No allowlist entry → refused (L-7: only user-approved sources).
        let err = validate_entry_url("https://evil.example.com/", &[], &public_ip());
        assert!(matches!(err, Err(RecipeError::InvalidEntryUrl(_))));
    }

    #[test]
    fn entry_url_loopback_refused() {
        let allow = vec!["localhost".to_string()];
        for u in [
            "http://127.0.0.1/login",
            "http://localhost/login",
            "http://169.254.169.254/latest/meta-data",
            "http://192.168.1.1/login",
        ] {
            assert!(
                validate_entry_url(
                    u,
                    &allow,
                    &resolves_to(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)))
                )
                .is_err(),
                "internal/loopback must be refused: {u}"
            );
        }
    }

    #[test]
    fn entry_url_file_scheme_refused() {
        let allow = vec!["x".to_string()];
        assert!(validate_entry_url("file:///etc/passwd", &allow, &public_ip()).is_err());
    }

    #[test]
    fn entry_url_raw_ip_refused() {
        let allow = vec!["8.8.8.8".to_string()];
        // raw IP host is always refused (force hostname → allowlist meaningful).
        assert!(validate_entry_url("https://8.8.8.8/login", &allow, &public_ip()).is_err());
    }
}
