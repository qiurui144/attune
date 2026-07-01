pub(crate) fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !in_ws && !out.is_empty() {
                out.push(' ');
            }
            in_ws = true;
        } else {
            out.push(ch);
            in_ws = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapse_trims_and_deduplicates_unicode_whitespace() {
        assert_eq!(collapse_whitespace("  a\tb\n\nc\u{3000} "), "a b c");
    }

    #[test]
    fn collapse_empty_and_whitespace_only_to_empty() {
        assert_eq!(collapse_whitespace(""), "");
        assert_eq!(collapse_whitespace(" \n\t "), "");
    }
}
