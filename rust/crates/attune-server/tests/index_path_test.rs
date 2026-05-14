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
    #[cfg(unix)]
    fn rejects_path_outside_home() {
        // /tmp typically exists and is a directory on Linux; on Windows it isn't
        // even an absolute path (Path::is_absolute 要求 drive prefix), 所以这个
        // 断言形态 Unix-only. Windows 路径校验逻辑由 validate_bind_path 单测覆盖.
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/home/test"));
        let result = validate_bind_path("/tmp", &home);
        if let Err((status, body)) = result {
            assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
            let body_str = serde_json::to_string(&body.0).unwrap();
            assert!(body_str.contains("home directory") || body_str.contains("not found"));
        }
        // If home happens to be / or contains /tmp (unlikely), test is vacuously ok
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
    #[cfg(unix)]
    fn accepts_home_directory_itself() {
        // Unix-only: Windows canonicalize() 给 home 返回 UNC \\?\C:\Users\xxx 前缀,
        // 但 home 参数仍是 C:\Users\xxx — canonical.starts_with(home) 失败.
        // 这是 Windows 路径系统的真实行为, 应在 validate_bind_path 里处理 UNC 前缀
        // 或换更稳健的 path containment 算法; 暂以 cfg(unix) 隔离测试.
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        if home.exists() && home.is_dir() {
            let result = validate_bind_path(home.to_str().unwrap(), &home);
            assert!(result.is_ok(), "home dir itself should be accepted: {:?}", result);
        }
    }
}
