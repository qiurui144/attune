//! L1 内置 PII 正则检测器。每个 detector 返回 byte 区间列表。
//!
//! 设计原则：
//! - 高 precision 优先：宁可漏，不要乱标。模糊场景留给 L2 NER / L3 LLM
//! - 校验位：身份证、信用卡、IP 各做格式 + 校验位 + 边界检查
//! - 不依赖 lookaround（regex crate 不支持），用"抓出后再 filter"

use regex::Regex;
use std::sync::OnceLock;

/// 模块内宏：把一个 fn 名 + 正则模式 绑成 `fn() -> &'static Regex`。
/// 替代 `once_cell::Lazy`，零依赖（用 std::sync::OnceLock，1.70+ 稳定）。
macro_rules! lazy_regex {
    ($vis:vis fn $name:ident = $pat:expr) => {
        $vis fn $name() -> &'static Regex {
            static RE: OnceLock<Regex> = OnceLock::new();
            RE.get_or_init(|| Regex::new($pat).expect(concat!("compile regex: ", stringify!($name))))
        }
    };
}

lazy_regex!(fn re_id_card =
    r"[1-9]\d{5}(?:19|20)\d{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12]\d|3[01])\d{3}[0-9Xx]"
);
lazy_regex!(fn re_phone = r"(?:\+?86)?1[3-9]\d{9}");
lazy_regex!(fn re_email = r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}");
lazy_regex!(fn re_ipv4 =
    r"(?:(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\.){3}(?:25[0-5]|2[0-4]\d|[01]?\d\d?)"
);
// IPv6: 第一段必须有 hex（避免空匹配），后跟 2-7 段 ":" + (0-4 hex)。
// 允许 hex 段为 0 长度以支持 "::" 缩写形式（如 "2001:db8::1"）。
lazy_regex!(fn re_ipv6 = r"[0-9a-fA-F]{1,4}(?::[0-9a-fA-F]{0,4}){2,7}");
lazy_regex!(fn re_credit_card = r"(?:\d{4}[ \-]?){3}\d{4}");
lazy_regex!(fn re_bank_card = r"\d{16,19}");
lazy_regex!(fn re_url = r#"https?://[^\s<>"'一-鿿]+"#);
lazy_regex!(fn re_mac = r"[0-9A-Fa-f]{2}(?:[:\-][0-9A-Fa-f]{2}){5}");
lazy_regex!(fn re_plate =
    r"[京津沪渝冀豫云辽黑湘皖鲁新苏浙赣鄂桂甘晋蒙陕吉闽贵粤青藏川宁琼][A-Z][A-Z0-9]{5,6}"
);
lazy_regex!(fn re_gps = r"-?\d{1,3}\.\d{2,}\s*,\s*-?\d{1,3}\.\d{2,}");

fn re_api_keys() -> &'static [Regex] {
    static KEYS: OnceLock<Vec<Regex>> = OnceLock::new();
    KEYS.get_or_init(|| {
        vec![
            // OpenAI / Anthropic / 通用 sk-
            Regex::new(r"sk-(?:ant-api03-)?[A-Za-z0-9_\-]{32,}").unwrap(),
            // GitHub PAT / OAuth / server / user
            Regex::new(r"gh[posu]_[A-Za-z0-9]{36,}").unwrap(),
            Regex::new(r"github_pat_[A-Za-z0-9_]{22,}").unwrap(),
            // GitLab
            Regex::new(r"glpat-[A-Za-z0-9_\-]{20}").unwrap(),
            // AWS Access Key
            Regex::new(r"AKIA[0-9A-Z]{16}").unwrap(),
            // Slack bot token
            Regex::new(r"xox[baprs]-[A-Za-z0-9\-]{10,}").unwrap(),
            // HuggingFace
            Regex::new(r"hf_[A-Za-z0-9]{32,}").unwrap(),
            // Google API
            Regex::new(r"AIza[0-9A-Za-z_\-]{35}").unwrap(),
        ]
    })
}

const ID_WEIGHTS: [u32; 17] = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2];
const ID_CHECK_MAP: [char; 11] = ['1', '0', 'X', '9', '8', '7', '6', '5', '4', '3', '2'];

fn id_card_checksum_ok(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 18 {
        return false;
    }
    let mut sum = 0u32;
    for (i, &b) in bytes.iter().take(17).enumerate() {
        if !b.is_ascii_digit() {
            return false;
        }
        sum += (b - b'0') as u32 * ID_WEIGHTS[i];
    }
    let expected = ID_CHECK_MAP[(sum % 11) as usize];
    let actual = bytes[17] as char;
    expected.eq_ignore_ascii_case(&actual)
}

pub fn detect_id_card(text: &str) -> Vec<(usize, usize)> {
    re_id_card()
        .find_iter(text)
        .filter(|m| id_card_checksum_ok(m.as_str()))
        .filter(|m| !is_sandwiched_by_digit(text, m.start(), m.end()))
        .map(|m| (m.start(), m.end()))
        .collect()
}

pub fn detect_phone(text: &str) -> Vec<(usize, usize)> {
    re_phone()
        .find_iter(text)
        .filter(|m| !is_sandwiched_by_digit(text, m.start(), m.end()))
        .map(|m| (m.start(), m.end()))
        .collect()
}

pub fn detect_email(text: &str) -> Vec<(usize, usize)> {
    re_email()
        .find_iter(text)
        .map(|m| (m.start(), m.end()))
        .collect()
}

pub fn detect_ipv4(text: &str) -> Vec<(usize, usize)> {
    re_ipv4()
        .find_iter(text)
        .filter(|m| !is_sandwiched_by(text, m.start(), m.end(), |c| c.is_ascii_digit() || c == '.'))
        .map(|m| (m.start(), m.end()))
        .collect()
}

pub fn detect_ipv6(text: &str) -> Vec<(usize, usize)> {
    re_ipv6()
        .find_iter(text)
        .filter(|m| {
            let s = m.as_str();
            // 必须包含 :: 或至少 7 个冒号
            s.contains("::") || s.matches(':').count() >= 7
        })
        .filter(|m| s_parse_ipv6(m.as_str()))
        .map(|m| (m.start(), m.end()))
        .collect()
}

fn s_parse_ipv6(s: &str) -> bool {
    use std::net::Ipv6Addr;
    use std::str::FromStr;
    Ipv6Addr::from_str(s).is_ok()
}

fn luhn_ok(s: &str) -> bool {
    let digits: Vec<u32> = s.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() < 13 || digits.len() > 19 {
        return false;
    }
    let mut sum = 0u32;
    let n = digits.len();
    for (i, &d) in digits.iter().enumerate() {
        if (n - 1 - i) % 2 == 1 {
            let doubled = d * 2;
            sum += if doubled > 9 { doubled - 9 } else { doubled };
        } else {
            sum += d;
        }
    }
    sum % 10 == 0
}

pub fn detect_credit_card(text: &str) -> Vec<(usize, usize)> {
    re_credit_card()
        .find_iter(text)
        .filter(|m| luhn_ok(m.as_str()))
        .map(|m| (m.start(), m.end()))
        .collect()
}

pub fn detect_bank_card(text: &str) -> Vec<(usize, usize)> {
    re_bank_card()
        .find_iter(text)
        .filter(|m| !is_sandwiched_by_digit(text, m.start(), m.end()))
        .map(|m| (m.start(), m.end()))
        .collect()
}

pub fn detect_api_key(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for re in re_api_keys() {
        out.extend(re.find_iter(text).map(|m| (m.start(), m.end())));
    }
    out
}

pub fn detect_url(text: &str) -> Vec<(usize, usize)> {
    re_url()
        .find_iter(text)
        .map(|m| (m.start(), m.end()))
        .collect()
}

pub fn detect_mac(text: &str) -> Vec<(usize, usize)> {
    re_mac()
        .find_iter(text)
        .map(|m| (m.start(), m.end()))
        .collect()
}

pub fn detect_plate_number(text: &str) -> Vec<(usize, usize)> {
    re_plate()
        .find_iter(text)
        .map(|m| (m.start(), m.end()))
        .collect()
}

pub fn detect_gps(text: &str) -> Vec<(usize, usize)> {
    re_gps()
        .find_iter(text)
        .filter(|m| gps_in_range(m.as_str()))
        .map(|m| (m.start(), m.end()))
        .collect()
}

fn gps_in_range(s: &str) -> bool {
    let parts: Vec<&str> = s.split(',').map(|p| p.trim()).collect();
    if parts.len() != 2 {
        return false;
    }
    let lat: f64 = match parts[0].parse() {
        Ok(v) => v,
        Err(_) => return false,
    };
    let lon: f64 = match parts[1].parse() {
        Ok(v) => v,
        Err(_) => return false,
    };
    (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)
}

// ============================================================
// helpers: 边界检查（避免长串数字/IP 误命中）
// ============================================================

fn is_sandwiched_by_digit(text: &str, start: usize, end: usize) -> bool {
    is_sandwiched_by(text, start, end, |c| c.is_ascii_digit())
}

fn is_sandwiched_by<F: Fn(char) -> bool>(text: &str, start: usize, end: usize, pred: F) -> bool {
    let bytes = text.as_bytes();
    let prev_is = if start == 0 {
        false
    } else {
        bytes
            .get(start - 1)
            .map(|&b| pred(b as char))
            .unwrap_or(false)
    };
    let next_is = bytes.get(end).map(|&b| pred(b as char)).unwrap_or(false);
    prev_is || next_is
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract(text: &str, f: impl Fn(&str) -> Vec<(usize, usize)>) -> Vec<&str> {
        f(text).into_iter().map(|(s, e)| &text[s..e]).collect()
    }

    #[test]
    fn id_card_valid() {
        // 11010119900307125X — ISO 7064 mod 11-2: sum=178, 178%11=2, 末位 'X'
        let v = extract("身份证 11010119900307125X 已实名", detect_id_card);
        assert_eq!(v, vec!["11010119900307125X"]);
    }

    #[test]
    fn id_card_bad_checksum_rejected() {
        // 末位故意错（正确末位应是 'X'，这里用 0）
        let v = extract("身份证 110101199003071250 假", detect_id_card);
        assert!(v.is_empty(), "bad checksum should be rejected, got {v:?}");
    }

    #[test]
    fn id_card_inside_long_digits_rejected() {
        let v = extract("123411010119900307125X567", detect_id_card);
        assert!(v.is_empty());
    }

    #[test]
    fn phone_basic() {
        let v = extract("呼叫 13812345678", detect_phone);
        assert_eq!(v, vec!["13812345678"]);
    }

    #[test]
    fn phone_with_country_code() {
        let v = extract("call +8613812345678 now", detect_phone);
        assert_eq!(v, vec!["+8613812345678"]);
    }

    #[test]
    fn phone_inside_longer_digits_rejected() {
        let v = extract("01138123456789", detect_phone);
        assert!(
            v.is_empty(),
            "phone should not match inside longer digit run"
        );
    }

    #[test]
    fn email_basic() {
        let v = extract("contact a.b+filter@sub.example.com please", detect_email);
        assert_eq!(v, vec!["a.b+filter@sub.example.com"]);
    }

    #[test]
    fn email_chinese_context() {
        let v = extract("邮箱：user@example.com，请回复", detect_email);
        assert_eq!(v, vec!["user@example.com"]);
    }

    #[test]
    fn ipv4_basic() {
        let v = extract("server 192.168.1.1 on", detect_ipv4);
        assert_eq!(v, vec!["192.168.1.1"]);
    }

    #[test]
    fn ipv4_invalid_octet_rejected() {
        let v = extract("not 999.999.999.999", detect_ipv4);
        assert!(v.is_empty());
    }

    #[test]
    fn ipv6_basic() {
        let v = extract("addr 2001:db8::1 here", detect_ipv6);
        assert_eq!(v, vec!["2001:db8::1"]);
    }

    #[test]
    fn ipv6_invalid_rejected() {
        let v = extract("not gggg::1 zzz", detect_ipv6);
        assert!(v.is_empty());
    }

    #[test]
    fn credit_card_luhn_pass() {
        // Visa test 4111111111111111
        let v = extract("card 4111 1111 1111 1111 done", detect_credit_card);
        assert_eq!(v, vec!["4111 1111 1111 1111"]);
    }

    #[test]
    fn credit_card_luhn_fail() {
        let v = extract("not 4111 1111 1111 1112", detect_credit_card);
        assert!(v.is_empty(), "bad luhn should be rejected");
    }

    #[test]
    fn bank_card_16_digits() {
        let v = extract("银行卡 6225881234567890 充值", detect_bank_card);
        assert_eq!(v, vec!["6225881234567890"]);
    }

    #[test]
    fn api_key_openai() {
        let key = "sk-abcdefghijklmnopqrstuvwxyz0123456789";
        let text = format!("OPENAI_KEY={key} done");
        let v = extract(&text, detect_api_key);
        assert_eq!(v, vec![key]);
    }

    #[test]
    fn api_key_github_pat() {
        let key = "ghp_abcdefghijklmnopqrstuvwxyzABCDEF1234";
        let text = format!("GH_TOKEN={key} done");
        let v = extract(&text, detect_api_key);
        assert_eq!(v, vec![key]);
    }

    #[test]
    fn api_key_aws() {
        let v = extract("AWS_ACCESS=AKIAIOSFODNN7EXAMPLE done", detect_api_key);
        assert_eq!(v, vec!["AKIAIOSFODNN7EXAMPLE"]);
    }

    #[test]
    fn url_https() {
        let v = extract("see https://example.com/path?q=1 for info", detect_url);
        assert_eq!(v, vec!["https://example.com/path?q=1"]);
    }

    #[test]
    fn mac_colon() {
        let v = extract("mac aa:bb:cc:dd:ee:ff is", detect_mac);
        assert_eq!(v, vec!["aa:bb:cc:dd:ee:ff"]);
    }

    #[test]
    fn mac_dash() {
        let v = extract("MAC AA-BB-CC-DD-EE-FF found", detect_mac);
        assert_eq!(v, vec!["AA-BB-CC-DD-EE-FF"]);
    }

    #[test]
    fn plate_oil_car() {
        let v = extract("车牌 京A12345 进入", detect_plate_number);
        assert_eq!(v, vec!["京A12345"]);
    }

    #[test]
    fn plate_new_energy() {
        let v = extract("新能源 沪AD12345 上牌", detect_plate_number);
        assert_eq!(v, vec!["沪AD12345"]);
    }

    #[test]
    fn gps_basic() {
        let v = extract("location 39.9042, 116.4074 here", detect_gps);
        assert_eq!(v, vec!["39.9042, 116.4074"]);
    }

    #[test]
    fn gps_out_of_range_rejected() {
        let v = extract("not 200.5, 300.7 invalid", detect_gps);
        assert!(v.is_empty());
    }
}
