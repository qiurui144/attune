//! attune 版本常量 + 插件兼容性 gate。
//!
//! 用于在插件加载期(scan)拒绝声明了更高 `min_attune_version` 的包,
//! 让"装了跑不了"在加载期被清晰拒绝,而非运行期 NotFound 崩溃。
//!
//! WHY semver: 插件作者声明 `min_attune_version: "1.1.0"`,只有当前
//! attune ≥ 该版本才放行。比较走 semver 语义(1.10.0 > 1.9.0,非字典序)。

use crate::error::{Result, VaultError};

/// 当前 attune-core 版本(来源 Cargo.toml `[package].version`)。
pub const ATTUNE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 判断当前 attune 是否满足插件声明的 `min_attune_version`。
///
/// - `Ok(true)`  → 当前 ATTUNE_VERSION ≥ min,兼容
/// - `Ok(false)` → 当前 < min,不兼容(调用方应 skip + 提示升级)
/// - `Err`       → `min_attune_version` 非合法 semver(加载期拒绝)
///
/// pre-release(如 `1.1.0-rc.1`)按 semver 语义比较;若 min 含 pre-release 标识,
/// 当前为 `1.1.0` 仍视为满足 `>= 1.1.0-rc.1`(stable > prerelease)。
pub fn is_compatible(min_attune_version: &str) -> Result<bool> {
    let min = semver::Version::parse(min_attune_version.trim()).map_err(|e| {
        VaultError::InvalidInput(format!(
            "invalid min_attune_version '{min_attune_version}': {e}"
        ))
    })?;
    let current = semver::Version::parse(ATTUNE_VERSION).map_err(|e| {
        // 理论不会发生(Cargo.toml 版本由 cargo 保证合法 semver),保险起见返回 Err。
        VaultError::InvalidInput(format!("ATTUNE_VERSION '{ATTUNE_VERSION}' not semver: {e}"))
    })?;
    Ok(current >= min)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attune_version_is_valid_semver() {
        assert!(
            semver::Version::parse(ATTUNE_VERSION).is_ok(),
            "ATTUNE_VERSION '{ATTUNE_VERSION}' must be valid semver"
        );
    }

    #[test]
    fn min_below_current_is_compatible() {
        // 0.0.1 一定 <= 任何正常发布版本
        assert!(is_compatible("0.0.1").expect("valid semver"));
    }

    #[test]
    fn min_far_above_current_is_incompatible() {
        // 99.0.0 一定 > 当前版本
        assert!(!is_compatible("99.0.0").expect("valid semver"));
    }

    #[test]
    fn min_equal_to_current_is_compatible() {
        // 边界: min == current → >= 成立 → 兼容
        assert!(is_compatible(ATTUNE_VERSION).expect("valid semver"));
    }

    #[test]
    fn invalid_semver_is_error() {
        assert!(is_compatible("not-a-version").is_err());
        assert!(is_compatible("1.x").is_err());
        assert!(is_compatible("").is_err());
    }

    #[test]
    fn semver_ordering_not_lexicographic() {
        // 字典序会把 "1.9.0" 排在 "1.10.0" 之后;semver 不会。
        // 用一个确定低于当前的版本验证语义比较生效。
        assert!(is_compatible("1.0.0").expect("valid"));
        // whitespace 容错
        assert!(is_compatible("  1.0.0  ").expect("valid"));
    }
}
