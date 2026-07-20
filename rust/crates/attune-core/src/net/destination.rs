//! Outbound destination classification shared by LLM, embedding, and scheduler callers.
//!
//! Only IP literals in a non-public range and the exact hostname `localhost` are
//! considered local. Named hosts fail closed because DNS may resolve them to a
//! public destination (or change after validation).

use std::net::IpAddr;

/// Return whether an HTTP(S) URL points at the local machine or a private/link-local network.
///
/// The URL is parsed before inspecting its host, so userinfo, paths, and query strings cannot
/// smuggle a local-looking substring into a public hostname.
pub fn is_local_network_url(raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }

    let host = host.trim_start_matches('[').trim_end_matches(']');
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            ip.is_loopback() || ip.is_unspecified() || ip.is_private() || ip.is_link_local()
        }
        Ok(IpAddr::V6(ip)) => {
            let first = ip.octets()[0];
            ip.is_loopback()
                || ip.is_unspecified()
                || first & 0xfe == 0xfc // fc00::/7 unique-local
                || (first == 0xfe && ip.octets()[1] & 0xc0 == 0x80) // fe80::/10 link-local
        }
        Err(_) => false,
    }
}

/// Return whether a URL is safe to use as a local-scheduler base.
///
/// This is intentionally stricter than [`is_local_network_url`]: scheduler
/// clients append privileged API paths and may send prompts or document bytes,
/// so a base with userinfo/query/fragment would change the request target when
/// naively joined. Link-local/unspecified IPs are also excluded to avoid cloud
/// metadata and bind-address SSRF targets.
pub fn is_safe_local_scheduler_url(raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let host = host.trim_start_matches('[').trim_end_matches(']');
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private(),
        Ok(IpAddr::V6(ip)) => {
            let first = ip.octets()[0];
            ip.is_loopback() || first & 0xfe == 0xfc // fc00::/7 unique-local
        }
        Err(_) => false,
    }
}

/// Safely append a scheduler API path to a validated base URL.
pub fn join_local_scheduler_url(base: &str, api_path: &str) -> Option<String> {
    if !is_safe_local_scheduler_url(base)
        || !api_path.starts_with('/')
        || api_path.contains(['?', '#'])
    {
        return None;
    }
    let mut url = url::Url::parse(base).ok()?;
    let base_path = url.path().trim_end_matches('/');
    let joined = format!("{base_path}{api_path}");
    url.set_path(&joined);
    url.set_query(None);
    url.set_fragment(None);
    Some(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::{is_local_network_url, is_safe_local_scheduler_url, join_local_scheduler_url};

    #[test]
    fn accepts_exact_local_and_private_destinations() {
        for endpoint in [
            "http://localhost:8090/v1",
            "http://127.0.0.5:8090/v1",
            "http://0.0.0.0:8090/v1",
            "http://10.2.3.4:8090/v1",
            "http://172.31.2.3:8090/v1",
            "http://192.168.1.2:8090/v1",
            "http://169.254.1.2:8090/v1",
            "http://[::1]:8090/v1",
            "http://[fd00::2]:8090/v1",
            "http://[fe80::2]:8090/v1",
        ] {
            assert!(is_local_network_url(endpoint), "{endpoint}");
        }
    }

    #[test]
    fn rejects_public_named_and_disguised_destinations() {
        for endpoint in [
            "https://api.openai.com/v1",
            "http://8.8.8.8:8090/v1",
            "http://localhost.evil.test:8090/v1",
            "http://127.0.0.1.evil.test:8090/v1",
            "http://10.0.0.1@evil.test:8090/v1",
            "http://172.2.0.1:8090/v1",
            "ftp://127.0.0.1/model",
            "not a url",
        ] {
            assert!(!is_local_network_url(endpoint), "{endpoint}");
        }
    }

    #[test]
    fn scheduler_destination_rejects_ambiguous_and_metadata_targets() {
        for endpoint in [
            "http://user@127.0.0.1:8090",
            "http://127.0.0.1:8090/admin?",
            "http://127.0.0.1:8090/admin#fragment",
            "http://0.0.0.0:8090",
            "http://169.254.169.254:80/latest",
            "http://[fe80::2]:8090",
            "http://8.8.8.8:8090",
        ] {
            assert!(!is_safe_local_scheduler_url(endpoint), "{endpoint}");
        }
        for endpoint in [
            "http://localhost:8090",
            "http://127.0.0.1:8090/v1",
            "http://10.2.3.4:8090/prefix",
            "http://[::1]:8090",
            "http://[fd00::2]:8090",
        ] {
            assert!(is_safe_local_scheduler_url(endpoint), "{endpoint}");
        }
    }

    #[test]
    fn scheduler_url_join_preserves_safe_prefix_as_path() {
        assert_eq!(
            join_local_scheduler_url("http://127.0.0.1:8090/prefix/", "/models").as_deref(),
            Some("http://127.0.0.1:8090/prefix/models")
        );
        assert!(join_local_scheduler_url("http://127.0.0.1:8090/admin?", "/models").is_none());
    }
}
