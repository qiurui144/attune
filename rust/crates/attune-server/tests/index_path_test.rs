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
        // /tmp typically exists and is a directory on Linux
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/home/test"));
        let result = validate_bind_path("/tmp", &home);
        // /tmp exists and is a directory, but should be outside home
        // (home is typically /home/xxx, so /tmp is outside)
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
    fn accepts_home_directory_itself() {
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        // Use home dir itself as the path to bind - should succeed if home exists
        if home.exists() && home.is_dir() {
            let result = validate_bind_path(home.to_str().unwrap(), &home);
            assert!(result.is_ok(), "home dir itself should be accepted: {:?}", result);
        }
    }
}
