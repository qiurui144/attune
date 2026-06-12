//! Update feed endpoint resolution for the Tauri updater.
//!
//! WHY this module exists: the updater endpoint was hardcoded to GitHub
//! (`github.com/.../latest/download/latest.json`), which is slow / unreachable
//! for CN users. We make the feed configurable via `ATTUNE_UPDATE_FEED_URL`
//! while keeping GitHub as the default, and let Tauri's multi-endpoint fallback
//! try the company mirror first then GitHub.
//!
//! Signature trust root is NOT touched here: the minisign pubkey lives in
//! `tauri.conf.json` and is verified by tauri-plugin-updater against whichever
//! endpoint serves `latest.json`. Changing the endpoint cannot weaken signature
//! verification — a mirror serving a tampered `latest.json` still fails the
//! minisign check (anti-poisoning).

/// Official GitHub release feed — the always-present fallback / default.
pub const DEFAULT_GITHUB_FEED: &str =
    "https://github.com/qiurui144/attune/releases/latest/download/latest.json";

/// Env var that, when set to a non-empty URL, prepends a company-mirror feed
/// ahead of the GitHub default (Tauri tries endpoints in order).
pub const FEED_ENV_VAR: &str = "ATTUNE_UPDATE_FEED_URL";

/// Resolve the ordered list of update-feed endpoints.
///
/// - `override_feed = None` or empty/whitespace  → `[GitHub]` (default, back-compat).
/// - `override_feed = Some("https://dl.../latest.json")` → `[mirror, GitHub]`
///   so the mirror is tried first and GitHub is the fallback.
///
/// If the override equals the GitHub default it is NOT duplicated.
/// Whitespace is trimmed; an all-whitespace value is treated as unset.
pub fn resolve_endpoints(override_feed: Option<&str>) -> Vec<String> {
    match override_feed.map(str::trim).filter(|s| !s.is_empty()) {
        Some(mirror) if mirror != DEFAULT_GITHUB_FEED => {
            vec![mirror.to_string(), DEFAULT_GITHUB_FEED.to_string()]
        }
        // override unset, empty, or identical to default → just the default
        _ => vec![DEFAULT_GITHUB_FEED.to_string()],
    }
}

/// Resolve endpoints from the process environment (`ATTUNE_UPDATE_FEED_URL`).
/// Convenience wrapper around [`resolve_endpoints`] for the runtime call sites.
pub fn resolve_endpoints_from_env() -> Vec<String> {
    let from_env = std::env::var(FEED_ENV_VAR).ok();
    resolve_endpoints(from_env.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_github_when_unset() {
        let eps = resolve_endpoints(None);
        assert_eq!(eps, vec![DEFAULT_GITHUB_FEED.to_string()]);
    }

    #[test]
    fn empty_override_falls_back_to_github() {
        assert_eq!(resolve_endpoints(Some("")), vec![DEFAULT_GITHUB_FEED]);
        assert_eq!(resolve_endpoints(Some("   ")), vec![DEFAULT_GITHUB_FEED]);
    }

    #[test]
    fn company_mirror_is_tried_before_github() {
        let mirror = "https://dl.engi-stack.com/attune/latest.json";
        let eps = resolve_endpoints(Some(mirror));
        // mirror first, github as ordered fallback
        assert_eq!(eps, vec![mirror.to_string(), DEFAULT_GITHUB_FEED.to_string()]);
        assert_eq!(eps.len(), 2);
        assert_eq!(eps[0], mirror, "mirror must be first for CN-fast path");
        assert_eq!(eps[1], DEFAULT_GITHUB_FEED, "github must remain as fallback");
    }

    #[test]
    fn override_equal_to_default_is_not_duplicated() {
        let eps = resolve_endpoints(Some(DEFAULT_GITHUB_FEED));
        assert_eq!(eps, vec![DEFAULT_GITHUB_FEED.to_string()]);
        assert_eq!(eps.len(), 1, "no duplicate github endpoint");
    }

    #[test]
    fn whitespace_around_mirror_is_trimmed() {
        let eps = resolve_endpoints(Some("  https://dl.engi-stack.com/attune/latest.json  "));
        assert_eq!(eps[0], "https://dl.engi-stack.com/attune/latest.json");
    }

    #[test]
    fn github_always_present_as_trust_anchored_fallback() {
        // Regardless of override, GitHub (signed by the same minisign key) is
        // always reachable as a fallback so a flaky/unreachable mirror never
        // bricks updates. The signature pubkey is endpoint-independent
        // (lives in tauri.conf.json), so this list change cannot weaken trust.
        for ov in [None, Some(""), Some("https://mirror.example/latest.json")] {
            let eps = resolve_endpoints(ov);
            assert!(
                eps.iter().any(|e| e == DEFAULT_GITHUB_FEED),
                "github fallback must always be present for override {ov:?}"
            );
        }
    }
}
