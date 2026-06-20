//! Export security helpers: download-filename sanitisation (path-traversal defence)
//! and CSV/spreadsheet formula-injection escaping.
//!
//! Both are mandatory quality-gate items (spec §7 / task §安全):
//!   - A caller-supplied filename must never escape the temp dir or contain path
//!     separators / control chars (`../`, `..\\`, NUL, newlines).
//!   - A cell value that begins with `=`, `+`, `-`, `@`, or a leading tab/CR
//!     would be interpreted as a formula by Excel / LibreOffice / Sheets on open
//!     ("CSV injection" / "formula injection"). We neutralise it by prefixing a
//!     single quote `'` so the value is rendered as literal text.

/// Maximum length for a sanitised filename stem (defensive; OS limits are larger).
const MAX_STEM_LEN: usize = 80;

/// Characters that are never allowed in a download filename component.
fn is_forbidden(c: char) -> bool {
    // path separators, control chars, and Windows-reserved chars
    c == '/'
        || c == '\\'
        || c == '\0'
        || c.is_control()
        || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|')
}

/// Sanitise a user-supplied name into a safe download filename **stem** (no
/// extension). Strips any directory components, forbidden chars, leading dots
/// (no `..` traversal, no hidden files), and clamps the length. Falls back to
/// `"export"` if nothing usable remains.
///
/// The returned stem contains no `/`, `\`, `.`-prefix, or control characters, so
/// joining it with a known extension and a server-controlled directory cannot
/// traverse out of that directory.
pub fn safe_stem(raw: &str) -> String {
    // Take only the final path component (defeats "a/b/../../etc" inputs).
    let last = raw
        .rsplit(|c| c == '/' || c == '\\')
        .next()
        .unwrap_or(raw);

    let mut out = String::with_capacity(last.len().min(MAX_STEM_LEN));
    for c in last.chars() {
        if is_forbidden(c) {
            out.push('_');
        } else {
            out.push(c);
        }
        if out.chars().count() >= MAX_STEM_LEN {
            break;
        }
    }

    // Drop a trailing extension if the caller passed "foo.csv" as the stem.
    if let Some(idx) = out.rfind('.') {
        // keep only if there is a non-empty base before the dot
        if idx > 0 {
            out.truncate(idx);
        }
    }

    // Trim leading/trailing dots, spaces, underscores so we never produce
    // "." / ".." / hidden-file names.
    let trimmed = out.trim_matches(|c: char| c == '.' || c == ' ' || c == '_');
    let cleaned = trimmed.to_string();

    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "export".to_string()
    } else {
        cleaned
    }
}

/// Build a full safe download filename from a raw name and an extension.
/// e.g. `download_filename("设备参数差异/../x", "xlsx")` → `"设备参数差异_x.xlsx"`-ish,
/// guaranteed free of path separators.
pub fn download_filename(raw: &str, ext: &str) -> String {
    format!("{}.{}", safe_stem(raw), ext)
}

/// Returns `true` if `value` would be interpreted as a formula by a spreadsheet
/// program when it appears as the first character of a CSV/cell value.
pub fn needs_formula_escape(value: &str) -> bool {
    matches!(
        value.chars().next(),
        Some('=') | Some('+') | Some('-') | Some('@') | Some('\t') | Some('\r')
    )
}

/// Escape a cell value against CSV/formula injection. If the value starts with a
/// dangerous character it is prefixed with a single quote so spreadsheet apps
/// treat it as literal text. (The `csv` crate / `rust_xlsxwriter` already handle
/// quoting for delimiters and newlines; this is the *formula* layer they do not.)
pub fn escape_cell(value: &str) -> std::borrow::Cow<'_, str> {
    if needs_formula_escape(value) {
        let mut s = String::with_capacity(value.len() + 1);
        s.push('\'');
        s.push_str(value);
        std::borrow::Cow::Owned(s)
    } else {
        std::borrow::Cow::Borrowed(value)
    }
}

#[cfg(test)]
mod sanitize_tests {
    use super::*;

    #[test]
    fn strips_path_traversal() {
        assert_eq!(safe_stem("../../etc/passwd"), "passwd");
        assert_eq!(safe_stem("..\\..\\windows\\system32"), "system32");
        assert_eq!(safe_stem("/abs/path/file"), "file");
        assert!(!download_filename("../x", "csv").contains('/'));
        assert!(!download_filename("..\\x", "csv").contains('\\'));
    }

    #[test]
    fn rejects_dot_and_empty() {
        assert_eq!(safe_stem(""), "export");
        assert_eq!(safe_stem("."), "export");
        assert_eq!(safe_stem(".."), "export");
        assert_eq!(safe_stem("   "), "export");
        assert_eq!(safe_stem("..."), "export");
    }

    #[test]
    fn strips_control_and_reserved() {
        assert!(!safe_stem("a\nb\tc").contains('\n'));
        assert!(!safe_stem("a\0b").contains('\0'));
        assert!(!safe_stem("a:b*c?").contains(':'));
    }

    #[test]
    fn keeps_cjk_and_basic() {
        assert_eq!(safe_stem("设备参数差异"), "设备参数差异");
        assert_eq!(safe_stem("report-2026"), "report-2026");
        assert_eq!(download_filename("设备参数差异", "xlsx"), "设备参数差异.xlsx");
    }

    #[test]
    fn clamps_length() {
        let long = "a".repeat(500);
        assert!(safe_stem(&long).chars().count() <= MAX_STEM_LEN);
    }

    #[test]
    fn formula_injection_escaped() {
        assert!(needs_formula_escape("=1+1"));
        assert!(needs_formula_escape("+44"));
        assert!(needs_formula_escape("-cmd"));
        assert!(needs_formula_escape("@SUM"));
        assert!(!needs_formula_escape("normal"));
        assert!(!needs_formula_escape("数值123"));
        assert_eq!(escape_cell("=1+1"), "'=1+1");
        assert_eq!(escape_cell("@x"), "'@x");
        assert_eq!(escape_cell("safe"), "safe");
    }
}
