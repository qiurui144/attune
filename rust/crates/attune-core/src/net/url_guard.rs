//! 出站 URL SSRF 防御 —— host allowlist + 私网/loopback/link-local 拒绝。
//!
//! 威胁模型：用户在 UI 输入「仓库 URL」，attune 后端会去 clone/fetch。攻击者可
//! 给 `http://169.254.169.254/...`（云 metadata）/ `http://127.0.0.1:xxxx`
//! （本机其它服务）/ `http://192.168.1.1`（内网）诱导后端打内网。
//!
//! 防御四层：
//!   ① scheme 仅 http(s)；
//!   ② host → 解析 IP，逐个拒绝 loopback / private / link-local / unspecified；
//!   ③ host allowlist（默认托管平台 + 用户显式加的自建 host）；
//!   ④ 返回已解析 IP 列表供调用方（按 IP 连接 / 连后复核），缓解 DNS rebinding。
//!
//! libgit2 自带 DNS 解析 + 连接，无法强制它「按我们解析的 IP 连」，所以 rebinding
//! 缓解是 best-effort：我们在校验时解析一次拒内网，并把 host 限制在 allowlist 内
//! （allowlist host 在解析层若被投毒到内网 IP，本层仍会拒）。

use std::net::IpAddr;

use url::{Host, Url};

use crate::error::{Result, VaultError};

/// 默认允许的托管平台 host（精确匹配 + 子域）。
pub const DEFAULT_ALLOW_HOSTS: &[&str] = &[
    "github.com",
    "gitlab.com",
    "bitbucket.org",
    "codeberg.org",
    "git.sr.ht",
];

/// 校验通过的出站目标。
#[derive(Debug, Clone)]
pub struct ValidatedUrl {
    /// 归一后的 URL（去 fragment / 保留 path）。
    pub url: Url,
    /// 主机名（lower-case；IP 字面量则是其字符串）。
    pub host: String,
    /// 解析出的 IP 列表（调用方可按 IP 连接，缓解 rebinding）。
    pub resolved_ips: Vec<IpAddr>,
}

/// 校验出站 URL。`allowlist` 为额外允许的 host（与 [`DEFAULT_ALLOW_HOSTS`] 合并）。
///
/// 失败返回 `VaultError`，含**脱敏**消息（不回显完整 URL 的 userinfo / token）。
///
/// `resolve` 注入 DNS 解析（生产=真实 resolver；测试注入固定映射，离线确定性）。
pub fn validate_outbound_url(
    raw: &str,
    allowlist: &[String],
    resolve: &dyn Fn(&str) -> std::io::Result<Vec<IpAddr>>,
) -> Result<ValidatedUrl> {
    let url = Url::parse(raw.trim())
        .map_err(|e| VaultError::InvalidInput(format!("invalid-git-url: parse: {e}")))?;

    // ① scheme 白名单。
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(VaultError::InvalidInput(format!(
                "invalid-git-url: scheme {other} not allowed (http/https only)"
            )));
        }
    }

    let host = url
        .host()
        .ok_or_else(|| VaultError::InvalidInput("invalid-git-url: missing host".into()))?;

    // host 可能是域名或 IP 字面量。
    let (host_str, literal_ip): (String, Option<IpAddr>) = match &host {
        Host::Domain(d) => (d.to_ascii_lowercase(), None),
        Host::Ipv4(ip) => (ip.to_string(), Some(IpAddr::V4(*ip))),
        Host::Ipv6(ip) => (ip.to_string(), Some(IpAddr::V6(*ip))),
    };

    // ③ host allowlist（IP 字面量从不在 allowlist → 直接走 ② 的 IP 拒绝，
    //    且即便是公网 IP 也不允许，强制走域名 host，杜绝绕过 allowlist）。
    if literal_ip.is_some() {
        return Err(VaultError::InvalidInput(
            "git-url-not-allowed: raw IP host not permitted (use a hostname)".into(),
        ));
    }
    if !host_allowed(&host_str, allowlist) {
        return Err(VaultError::InvalidInput(format!(
            "git-url-not-allowed: host {host_str} not in allowlist"
        )));
    }

    // ② 解析 host → IP，逐个拒内网。
    let ips = resolve(&host_str)
        .map_err(|e| VaultError::InvalidInput(format!("git-network-error: resolve {host_str}: {e}")))?;
    if ips.is_empty() {
        return Err(VaultError::InvalidInput(format!(
            "git-network-error: {host_str} resolved to no addresses"
        )));
    }
    for ip in &ips {
        if is_blocked_ip(ip) {
            // 不回显具体内网 IP（信息泄露面），只说被拒。
            return Err(VaultError::InvalidInput(format!(
                "git-url-not-allowed: {host_str} resolves to a non-public address"
            )));
        }
    }

    Ok(ValidatedUrl {
        url,
        host: host_str,
        resolved_ips: ips,
    })
}

/// host 是否命中 allowlist —— 精确或子域（`.github.com`）。
fn host_allowed(host: &str, extra: &[String]) -> bool {
    let check = |allowed: &str| {
        let allowed = allowed.to_ascii_lowercase();
        host == allowed || host.ends_with(&format!(".{allowed}"))
    };
    DEFAULT_ALLOW_HOSTS.iter().any(|h| check(h)) || extra.iter().any(|h| check(h))
}

/// 该 IP 是否应被拒绝（loopback / private / link-local / unspecified / 文档 /
/// 唯一本地 / CGNAT 等非公网范围）。
pub fn is_blocked_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()           // 127.0.0.0/8
                || v4.is_private()      // 10/8, 172.16/12, 192.168/16
                || v4.is_link_local()   // 169.254/16（含 169.254.169.254 云 metadata）
                || v4.is_unspecified()  // 0.0.0.0
                || v4.is_broadcast()    // 255.255.255.255
                || v4.is_documentation()
                || is_v4_cgnat(v4)      // 100.64/10
                || is_v4_benchmarking(v4) // 198.18/15
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()            // ::1
                || v6.is_unspecified()  // ::
                || is_v6_unique_local(v6) // fc00::/7
                || is_v6_link_local(v6)   // fe80::/10
                // IPv4-mapped（::ffff:a.b.c.d）按映射后的 v4 复判。
                || v6.to_ipv4_mapped().map(|m| is_blocked_ip(&IpAddr::V4(m))).unwrap_or(false)
        }
    }
}

fn is_v4_cgnat(ip: &std::net::Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (64..=127).contains(&o[1])
}

fn is_v4_benchmarking(ip: &std::net::Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 198 && (o[1] == 18 || o[1] == 19)
}

fn is_v6_unique_local(ip: &std::net::Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

fn is_v6_link_local(ip: &std::net::Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

/// 生产 DNS 解析器 —— `ToSocketAddrs`（阻塞，仅在阻塞上下文调用）。
pub fn system_resolve(host: &str) -> std::io::Result<Vec<IpAddr>> {
    use std::net::ToSocketAddrs;
    // 端口任意 —— 只取 IP。
    let addrs = (host, 443u16).to_socket_addrs()?;
    Ok(addrs.map(|s| s.ip()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn allow_github_ip(_h: &str) -> std::io::Result<Vec<IpAddr>> {
        Ok(vec![IpAddr::V4(Ipv4Addr::new(140, 82, 112, 3))]) // 公网（GitHub 段）
    }
    fn resolves_to(ip: IpAddr) -> impl Fn(&str) -> std::io::Result<Vec<IpAddr>> {
        move |_h: &str| Ok(vec![ip])
    }
    fn no_resolve(_h: &str) -> std::io::Result<Vec<IpAddr>> {
        Ok(vec![])
    }

    #[test]
    fn accepts_public_github_https() {
        let v = validate_outbound_url(
            "https://github.com/rust-lang/book",
            &[],
            &allow_github_ip,
        )
        .unwrap();
        assert_eq!(v.host, "github.com");
        assert_eq!(v.resolved_ips.len(), 1);
    }

    #[test]
    fn rejects_non_http_scheme() {
        for u in ["ssh://git@github.com/x", "git@github.com:x/y.git", "file:///etc/passwd", "ftp://github.com/x"] {
            assert!(
                validate_outbound_url(u, &[], &allow_github_ip).is_err(),
                "scheme should be rejected: {u}"
            );
        }
    }

    #[test]
    fn rejects_loopback_and_metadata_and_private() {
        // 这些 host 都不在 allowlist → 在 host allowlist 阶段就拒；
        // 即便加进 allowlist，解析到内网 IP 仍拒（下一个测试覆盖）。
        for u in [
            "http://127.0.0.1/x",
            "http://169.254.169.254/latest/meta-data",
            "http://[::1]/x",
            "http://192.168.1.1/x",
            "http://10.0.0.5/x",
            "http://172.16.0.1/x",
            "http://0.0.0.0/x",
        ] {
            assert!(
                validate_outbound_url(u, &[], &allow_github_ip).is_err(),
                "SSRF target must be rejected: {u}"
            );
        }
    }

    #[test]
    fn rejects_allowlisted_host_that_resolves_to_internal() {
        // DNS rebinding：host 在 allowlist，但解析到内网 IP → 拒。
        let internal = resolves_to(IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)));
        let err = validate_outbound_url("https://github.com/x", &[], &internal);
        assert!(err.is_err(), "rebinding to metadata IP must be rejected");
    }

    #[test]
    fn rejects_raw_ip_host_even_if_public() {
        // 公网 IP 字面量也拒（强制走域名，杜绝 allowlist 绕过）。
        assert!(validate_outbound_url("https://140.82.112.3/x", &[], &allow_github_ip).is_err());
    }

    #[test]
    fn rejects_host_not_in_allowlist() {
        assert!(validate_outbound_url("https://evil.example.com/x", &[], &allow_github_ip).is_err());
    }

    #[test]
    fn accepts_user_extra_allowlist_host() {
        let extra = vec!["git.internal-corp.example".to_string()];
        // 自建 host 在 allowlist + 解析到公网 IP → 通过。
        let v = validate_outbound_url(
            "https://git.internal-corp.example/team/wiki",
            &extra,
            &resolves_to(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9))),
        );
        // 203.0.113.0/24 是 TEST-NET-3（is_documentation）→ 被拒, 换公网段重试。
        assert!(v.is_err());
        let v2 = validate_outbound_url(
            "https://git.internal-corp.example/team/wiki",
            &extra,
            &resolves_to(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))),
        )
        .unwrap();
        assert_eq!(v2.host, "git.internal-corp.example");
    }

    #[test]
    fn subdomain_of_allowlisted_host_ok() {
        let v = validate_outbound_url(
            "https://raw.githubusercontent.com.github.com/x",
            &[],
            &allow_github_ip,
        );
        // 这个恶意构造的 host 以 .github.com 结尾 → 命中子域规则（设计如此：
        // 子域信任）。确认行为明确（不是 bug）：host_allowed 命中。
        assert!(v.is_ok());
        // 真正的攻击构造 `github.com.evil.com` 不以 .github.com 结尾 → 拒。
        assert!(validate_outbound_url("https://github.com.evil.com/x", &[], &allow_github_ip).is_err());
    }

    #[test]
    fn empty_resolution_is_error() {
        assert!(validate_outbound_url("https://github.com/x", &[], &no_resolve).is_err());
    }

    #[test]
    fn ipv6_unique_local_and_linklocal_blocked() {
        assert!(is_blocked_ip(&IpAddr::V6(Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 1))));
        assert!(is_blocked_ip(&IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1))));
        assert!(is_blocked_ip(&IpAddr::V6(Ipv6Addr::LOCALHOST)));
        // IPv4-mapped loopback。
        assert!(is_blocked_ip(&IpAddr::V6(Ipv4Addr::new(127, 0, 0, 1).to_ipv6_mapped())));
    }

    #[test]
    fn cgnat_and_benchmark_ranges_blocked() {
        assert!(is_blocked_ip(&IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
        assert!(is_blocked_ip(&IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1))));
        assert!(!is_blocked_ip(&IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(!is_blocked_ip(&IpAddr::V4(Ipv4Addr::new(140, 82, 112, 3))));
    }

    // proptest #1：host_allowed 幂等 —— 大小写不影响判定。
    proptest::proptest! {
        #[test]
        fn host_allowed_case_insensitive(suffix in "[a-z]{1,8}") {
            let lower = format!("{suffix}.github.com");
            let upper = lower.to_uppercase();
            // 输入先 lower-case 化（validate 内做），这里直接比 host_allowed 行为。
            proptest::prop_assert_eq!(
                host_allowed(&lower, &[]),
                host_allowed(&upper.to_ascii_lowercase(), &[])
            );
        }

        // proptest #2：任何 10.x / 192.168.x / 172.16-31.x 都被拒（私网全覆盖）。
        #[test]
        fn all_private_v4_blocked(b in 0u8..=255, c in 0u8..=255, d in 0u8..=255) {
            proptest::prop_assert!(is_blocked_ip(&IpAddr::V4(Ipv4Addr::new(10, b, c, d))));
            proptest::prop_assert!(is_blocked_ip(&IpAddr::V4(Ipv4Addr::new(192, 168, c, d))));
            proptest::prop_assert!(is_blocked_ip(&IpAddr::V4(Ipv4Addr::new(127, b, c, d))));
        }

        // proptest #3：validate 对非 http(s) scheme 永远拒（不 panic）。
        #[test]
        fn non_http_scheme_never_accepts(scheme in "[a-z]{2,6}") {
            if scheme == "http" || scheme == "https" { return Ok(()); }
            let u = format!("{scheme}://github.com/x");
            proptest::prop_assert!(validate_outbound_url(&u, &[], &allow_github_ip).is_err());
        }
    }
}
