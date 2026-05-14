//! Tests for bind_directory path validation logic.
//! These tests call the actual `validate_bind_path` function used by the route handler.

#[cfg(test)]
mod tests {
    use attune_server::routes::index::validate_bind_path;

    #[test]
    fn rejects_relative_path() {
        let home = std::path::Path::new("/home/user");
        let result = validate_bind_path("relative/path", home);
        assert!(result.is_err());
        let (status, body) = result.unwrap_err();
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        let body_str = serde_json::to_string(&body.0).unwrap();
        assert!(body_str.contains("absolute"));
    }

    #[test]
    fn rejects_path_outside_home() {
        // Linux: "/tmp" 存在且在 home 外 → 校验拒绝, body 含 "home directory"
        // Windows: "/tmp" 不带 drive prefix, Path::is_absolute() = false → body 含 "absolute"
        // 也算"正确拒绝" — 关键是 result 必须是 Err. 用宽断言覆盖三平台.
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/home/test"));
        let result = validate_bind_path("/tmp", &home);
        assert!(result.is_err(), "path outside home should be rejected: {result:?}");
        let (status, body) = result.unwrap_err();
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        let body_str = serde_json::to_string(&body.0).unwrap();
        // 各平台的拒绝理由不同: absolute / home directory / not found 任一即可
        assert!(
            body_str.contains("home directory")
                || body_str.contains("not found")
                || body_str.contains("absolute"),
            "unexpected rejection reason: {body_str}"
        );
    }

    #[test]
    fn rejects_nonexistent_path() {
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/home/test"));
        let result = validate_bind_path("/absolutely/nonexistent/path/xyz123", &home);
        assert!(result.is_err());
        let (status, _) = result.unwrap_err();
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn accepts_home_directory_itself() {
        // 三平台都应通过 — validate_bind_path 改用 dunce::canonicalize 后,
        // Windows 上不再返回 \\?\ UNC 前缀, canonical.starts_with(home) 正常工作.
        // 历史: Windows 用 std::fs::canonicalize 时 starts_with 失败 → 用户连
        // home 目录本身都加不进 vault, prod bug 被 cfg(unix) 测试 mask 过一阵.
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        if home.exists() && home.is_dir() {
            let result = validate_bind_path(home.to_str().unwrap(), &home);
            assert!(result.is_ok(), "home dir itself should be accepted: {:?}", result);
        }
    }
}
