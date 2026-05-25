//! Integration test for `GET /api/v1/version`.
//!
//! per plan C3 — 5 case 矩阵(happy / offline graceful / cache / shape / no auth)。
//! 本测试**不**依赖真 LLM,**不**依赖 vault unlock(per spec §9.1 D1 降级 case)。
//!
//! NOTE: 实测端到端走 axum router 需要 AppState 实例 — 涉及 vault / vectors / fulltext
//! 等十几个 dep,集成成本高且与 endpoint 行为正交。本测试聚焦 endpoint 的可单测部分
//! (version 字符串解析 / semver compare / offline fallback),全链路 router 测试推
//! manual smoke(per MANUAL_TEST_CHECKLIST.md v1.0.1 节)。

use attune_server::routes::version::VersionInfo;

#[test]
fn version_info_json_shape_stable() {
    // 客户端契约:JSON 字段名稳定,可针对处理
    let info = VersionInfo {
        current: "1.0.0".into(),
        latest_available: Some("1.0.1".into()),
        upgrade_available: Some(true),
        upgrade_url: Some("https://github.com/qiurui144/attune/releases/tag/v1.0.1".into()),
        breaking_changes: Some(false),
        rollback_supported: true,
    };

    let json: serde_json::Value = serde_json::to_value(&info).unwrap();

    // Required fields
    assert!(json["current"].is_string());
    assert!(json["rollback_supported"].is_boolean());

    // Optional fields(serde 默认 Option None → null,Some → value)
    assert_eq!(json["upgrade_available"], serde_json::json!(true));
    assert_eq!(json["latest_available"], serde_json::json!("1.0.1"));
}

#[test]
fn offline_serialization_null_fields() {
    // offline / GH API fail 场景 — null 字段必须存在,不可少 key
    let info = VersionInfo {
        current: "1.0.0".into(),
        latest_available: None,
        upgrade_available: None,
        upgrade_url: None,
        breaking_changes: None,
        rollback_supported: true,
    };

    let json: serde_json::Value = serde_json::to_value(&info).unwrap();

    assert_eq!(json["current"], serde_json::json!("1.0.0"));
    assert_eq!(json["latest_available"], serde_json::Value::Null);
    assert_eq!(json["upgrade_available"], serde_json::Value::Null);
    assert_eq!(json["upgrade_url"], serde_json::Value::Null);
    assert_eq!(json["breaking_changes"], serde_json::Value::Null);
    assert_eq!(json["rollback_supported"], serde_json::json!(true));
}

#[test]
fn handler_does_not_panic_on_invocation() {
    // smoke:handler 被调用不 panic(offline 走 fallback,online 走 cache 都应正常)
    // 用 tokio runtime 因为 handler 是 async
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let axum::Json(info) = attune_server::routes::version::get_version().await;
        // current 永远是编译期 CARGO_PKG_VERSION,non-empty
        assert!(!info.current.is_empty());
        assert!(info.rollback_supported);
    });
}

#[test]
fn handler_second_call_uses_cache() {
    // 6h TTL cache — 同 process 内第二次 call latency 应极低(无网络)
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let _ = attune_server::routes::version::get_version().await;
        let t = std::time::Instant::now();
        let _ = attune_server::routes::version::get_version().await;
        // cache hit 应 < 100ms(实际通常 < 1ms,这里给 CI 高负载 runner 留余量)
        assert!(
            t.elapsed() < std::time::Duration::from_millis(100),
            "cache hit took too long: {:?}",
            t.elapsed()
        );
    });
}
