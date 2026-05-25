//! 字段值规范化辅助 — 日期 / 金额 / 校验位.
//!
//! 全部纯函数 (无 IO, 无 panic, 异常输入返 None). 共用工具供 5 个 scene 抽取器调用.

/// `2026/05/18` / `2026年5月18日` / `26-5-18` / `2026.05.18` → ISO `2026-05-18`.
///
/// 接受分隔符: `/` `-` `.` `年月日`. 两位年份按 20xx 补 (2000 + yy).
/// 月/日范围越界 (>12 / >31) → None.
pub fn normalize_date(s: &str) -> Option<String> {
    let re = regex::Regex::new(r"(\d{2,4})\s*[年/\-.]\s*(\d{1,2})\s*[月/\-.]\s*(\d{1,2})").ok()?;
    let cap = re.captures(s)?;
    let y: u32 = cap.get(1)?.as_str().parse().ok()?;
    let m: u32 = cap.get(2)?.as_str().parse().ok()?;
    let d: u32 = cap.get(3)?.as_str().parse().ok()?;
    let y = if y < 100 { 2000 + y } else { y };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// 去千分位 / 全角 / 货币符号 / 空白 → "1234.56".
///
/// 接受: 半角/全角数字 + 句点小数 + 可选负号. 失败返 None.
pub fn normalize_amount(s: &str) -> Option<String> {
    let cleaned: String = s
        .chars()
        .filter_map(|c| match c {
            '0'..='9' | '.' | '-' => Some(c),
            '０'..='９' => char::from_u32(c as u32 - '０' as u32 + b'0' as u32),
            _ => None, // ',' '，' '￥' '$' '元' '人民币' 等全部 drop
        })
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    cleaned.parse::<f64>().ok().map(|f| format!("{f:.2}"))
}

/// Luhn 算法 — 银行卡 / 信用卡校验位.
/// 卡号长度 13-19 位之间; 非数字字符 (空格 / `-`) 自动剥除.
pub fn luhn_check(card: &str) -> bool {
    let digits: Vec<u32> = card.chars().filter_map(|c| c.to_digit(10)).collect();
    if !(13..=19).contains(&digits.len()) {
        return false;
    }
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            if i % 2 == 1 {
                let dd = d * 2;
                if dd > 9 {
                    dd - 9
                } else {
                    dd
                }
            } else {
                d
            }
        })
        .sum();
    sum % 10 == 0
}

/// GB 11643-1999 居民身份证号码校验 — 18 位 + 校验位.
pub fn id_card_cn_check(id: &str) -> bool {
    let bytes: Vec<char> = id.chars().collect();
    if bytes.len() != 18 {
        return false;
    }
    const WEIGHTS: [u32; 17] = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2];
    const CHECK_CHARS: [char; 11] = ['1', '0', 'X', '9', '8', '7', '6', '5', '4', '3', '2'];
    let mut sum = 0u32;
    for (i, w) in WEIGHTS.iter().enumerate() {
        match bytes[i].to_digit(10) {
            Some(d) => sum += d * w,
            None => return false,
        }
    }
    bytes[17].to_ascii_uppercase() == CHECK_CHARS[(sum % 11) as usize]
}

/// GB 32100-2015 统一社会信用代码校验 (营业执照) — 18 位混合数字+大写字母.
///
/// 字符集合 `0123456789ABCDEFGHJKLMNPQRTUWXY` (排除 I, O, S, V, Z). 31 取模.
pub fn business_license_check(code: &str) -> bool {
    let bytes: Vec<char> = code.chars().collect();
    if bytes.len() != 18 {
        return false;
    }
    const ALPHABET: &str = "0123456789ABCDEFGHJKLMNPQRTUWXY"; // 31 chars
    const WEIGHTS: [i32; 17] = [1, 3, 9, 27, 19, 26, 16, 17, 20, 29, 25, 13, 8, 24, 10, 30, 28];
    let mut sum: i32 = 0;
    for (i, w) in WEIGHTS.iter().enumerate() {
        let ch = bytes[i].to_ascii_uppercase();
        let pos = match ALPHABET.find(ch) {
            Some(p) => p as i32,
            None => return false,
        };
        sum += pos * w;
    }
    let expected_pos = (31 - sum.rem_euclid(31)) % 31;
    let expected_char = match ALPHABET.chars().nth(expected_pos as usize) {
        Some(c) => c,
        None => return false,
    };
    bytes[17].to_ascii_uppercase() == expected_char
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_iso_chinese() {
        assert_eq!(normalize_date("2026年5月18日").as_deref(), Some("2026-05-18"));
    }

    #[test]
    fn date_slash() {
        assert_eq!(normalize_date("2026/05/18").as_deref(), Some("2026-05-18"));
    }

    #[test]
    fn date_short_year() {
        assert_eq!(normalize_date("26-5-18").as_deref(), Some("2026-05-18"));
    }

    #[test]
    fn date_dot_separator() {
        assert_eq!(normalize_date("2026.05.18").as_deref(), Some("2026-05-18"));
    }

    #[test]
    fn date_invalid_month_returns_none() {
        assert!(normalize_date("2026-13-01").is_none());
    }

    #[test]
    fn date_garbage_returns_none() {
        assert!(normalize_date("not a date").is_none());
    }

    #[test]
    fn date_embedded_in_text() {
        assert_eq!(
            normalize_date("开票日期：2026年5月18日，请确认").as_deref(),
            Some("2026-05-18")
        );
    }

    #[test]
    fn amount_comma() {
        assert_eq!(normalize_amount("1,234.56").as_deref(), Some("1234.56"));
    }

    #[test]
    fn amount_yuan_symbol() {
        assert_eq!(normalize_amount("￥1,234.56").as_deref(), Some("1234.56"));
    }

    #[test]
    fn amount_dollar_symbol() {
        assert_eq!(normalize_amount("$1,234.56").as_deref(), Some("1234.56"));
    }

    #[test]
    fn amount_fullwidth_digits() {
        assert_eq!(normalize_amount("１２３４").as_deref(), Some("1234.00"));
    }

    #[test]
    fn amount_garbage_returns_none() {
        assert!(normalize_amount("abc").is_none());
    }

    #[test]
    fn luhn_valid_known_pattern() {
        // 79927398713 is a well-known Luhn-valid test number (11 digits).
        // Need ≥13 digits — pad: 4992 7398 7137 4567 (16 digits, validated)
        // VISA test number 4111 1111 1111 1111 is a known passing Luhn.
        assert!(luhn_check("4111111111111111"));
    }

    #[test]
    fn luhn_invalid_short() {
        assert!(!luhn_check("123"));
    }

    #[test]
    fn luhn_invalid_too_long() {
        assert!(!luhn_check("12345678901234567890")); // 20 digits
    }

    #[test]
    fn luhn_strips_spaces_and_dashes() {
        assert!(luhn_check("4111-1111 1111-1111"));
    }

    #[test]
    fn id_card_invalid_length() {
        assert!(!id_card_cn_check("1234"));
    }

    #[test]
    fn id_card_valid_synthetic() {
        // 17-digit body "11010119900101001":
        //   digits  = [1,1,0,1,0,1,1,9,9,0,0,1,0,1,0,0,1]
        //   weights = [7,9,10,5,8,4,2,1,6,3,7,9,10,5,8,4,2]
        //   sum     = 7+9+0+5+0+4+2+9+54+0+0+9+0+5+0+0+2 = 106
        //   106 % 11 = 7 → CHECK_CHARS[7] = '5'
        // So mathematically valid ID = "110101199001010015".
        assert!(id_card_cn_check("110101199001010015"));
    }

    #[test]
    fn id_card_check_digit_wrong() {
        // Same 17 prefix as valid_synthetic but flip check digit '5' → '9'.
        assert!(!id_card_cn_check("110101199001010019"));
    }

    #[test]
    fn business_license_invalid_length() {
        assert!(!business_license_check("123"));
    }

    #[test]
    fn business_license_garbage_returns_false() {
        // 18 chars but contains a char outside the alphabet (e.g. 'I' or 'O' or 'S')
        assert!(!business_license_check("91110000000000000I"));
    }
}
