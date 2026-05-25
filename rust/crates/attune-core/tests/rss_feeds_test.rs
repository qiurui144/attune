//! rss_feeds 表加密持久化集成测试。
//!
//! 与 email_accounts_test / webdav_remotes_test 同模式：覆盖 CRUD + 加密不漏 +
//! 条件 GET 字段维护 + last_entry_guid 推进 + enabled toggle 幂等性。

use attune_core::crypto::Key32;
use attune_core::store::rss_feeds::{RssFeedInput, DEFAULT_POLL_INTERVAL_MINUTES};
use attune_core::store::Store;

fn sample_input(name: &str, url: &str) -> RssFeedInput {
    RssFeedInput {
        name: name.to_string(),
        url: url.to_string(),
        poll_interval_minutes: None,
    }
}

#[test]
fn add_then_get_round_trips_with_decrypted_url() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let id = store
        .add_rss_feed(&dek, &sample_input("LWN", "https://lwn.net/headlines/rss"))
        .unwrap();

    let got = store.get_rss_feed(&dek, &id).unwrap().expect("feed exists");
    assert_eq!(got.id, id);
    assert_eq!(got.name, "LWN");
    assert_eq!(got.url, "https://lwn.net/headlines/rss", "url 必须能解密回明文");
    assert!(got.enabled, "默认启用");
    assert_eq!(got.poll_interval_minutes, DEFAULT_POLL_INTERVAL_MINUTES);
    assert!(got.etag.is_none());
    assert!(got.last_modified.is_none());
    assert!(got.last_entry_guid.is_none());
    assert!(got.last_polled_at.is_none());
}

#[test]
fn url_is_not_stored_in_plaintext() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let id = store
        .add_rss_feed(&dek, &sample_input("Secret", "https://PLAINTEXT_MARKER_XYZ/feed.xml"))
        .unwrap();
    let raw = store.debug_raw_rss_url_enc(&id).unwrap();
    assert!(!raw.is_empty(), "url_enc 应已写入");
    assert!(
        !raw.windows(20).any(|w| w == b"PLAINTEXT_MARKER_XYZ"),
        "url_enc 列绝不能含明文 URL"
    );
}

#[test]
fn list_returns_all_in_creation_order() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    for i in 0..3 {
        store
            .add_rss_feed(
                &dek,
                &sample_input(&format!("feed{i}"), &format!("https://ex.com/{i}.xml")),
            )
            .unwrap();
    }
    let all = store.list_rss_feeds(&dek).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].name, "feed0");
    assert_eq!(all[2].name, "feed2", "按 created_at 升序");
}

#[test]
fn delete_removes_row() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let id = store
        .add_rss_feed(&dek, &sample_input("tmp", "https://ex.com/feed.xml"))
        .unwrap();
    store.delete_rss_feed(&id).unwrap();
    assert!(store.get_rss_feed(&dek, &id).unwrap().is_none());
}

#[test]
fn update_etag_lastmod_persists_and_touches_polled_at() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let id = store
        .add_rss_feed(&dek, &sample_input("etag", "https://ex.com/feed.xml"))
        .unwrap();
    store
        .update_rss_etag_lastmod(&id, Some("\"abc123\""), Some("Wed, 21 Oct 2025 07:28:00 GMT"))
        .unwrap();

    let got = store.get_rss_feed(&dek, &id).unwrap().unwrap();
    assert_eq!(got.etag.as_deref(), Some("\"abc123\""));
    assert_eq!(
        got.last_modified.as_deref(),
        Some("Wed, 21 Oct 2025 07:28:00 GMT")
    );
    assert!(got.last_polled_at.is_some(), "200 OK 路径应 touch last_polled_at");
}

#[test]
fn touch_polled_at_advances_timestamp_without_clearing_etag() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let id = store
        .add_rss_feed(&dek, &sample_input("304", "https://ex.com/feed.xml"))
        .unwrap();
    store
        .update_rss_etag_lastmod(&id, Some("\"v1\""), None)
        .unwrap();
    let first = store
        .get_rss_feed(&dek, &id)
        .unwrap()
        .unwrap()
        .last_polled_at
        .unwrap();

    // 模拟"几毫秒后 304 命中" —— touch_polled_at 必须前进但不清 etag。
    std::thread::sleep(std::time::Duration::from_millis(10));
    store.touch_rss_polled_at(&id).unwrap();
    let got = store.get_rss_feed(&dek, &id).unwrap().unwrap();
    assert_eq!(got.etag.as_deref(), Some("\"v1\""), "304 路径不能清 etag");
    assert!(
        got.last_polled_at.unwrap() > first,
        "last_polled_at 必须前进，否则 tight-loop 保护失效"
    );
}

#[test]
fn update_last_entry_advances_guid_and_polled_at() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let id = store
        .add_rss_feed(&dek, &sample_input("guid", "https://ex.com/feed.xml"))
        .unwrap();
    store
        .update_rss_last_entry(&id, "tag:example.com,2026:entry/42")
        .unwrap();
    let got = store.get_rss_feed(&dek, &id).unwrap().unwrap();
    assert_eq!(
        got.last_entry_guid.as_deref(),
        Some("tag:example.com,2026:entry/42")
    );
    assert!(got.last_polled_at.is_some());
}

#[test]
fn update_feed_settings_can_disable_and_change_interval() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let id = store
        .add_rss_feed(&dek, &sample_input("toggle", "https://ex.com/feed.xml"))
        .unwrap();
    store
        .update_rss_feed_settings(&id, Some(false), Some(15))
        .unwrap();
    let got = store.get_rss_feed(&dek, &id).unwrap().unwrap();
    assert!(!got.enabled);
    assert_eq!(got.poll_interval_minutes, 15);

    // None 应保留原值
    store.update_rss_feed_settings(&id, None, None).unwrap();
    let got2 = store.get_rss_feed(&dek, &id).unwrap().unwrap();
    assert!(!got2.enabled);
    assert_eq!(got2.poll_interval_minutes, 15);

    // 重新启用
    store.update_rss_feed_settings(&id, Some(true), None).unwrap();
    let got3 = store.get_rss_feed(&dek, &id).unwrap().unwrap();
    assert!(got3.enabled);
}

#[test]
fn add_feed_uses_default_interval_when_none() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let id = store
        .add_rss_feed(
            &dek,
            &RssFeedInput {
                name: "default-iv".into(),
                url: "https://ex.com/feed.xml".into(),
                poll_interval_minutes: None,
            },
        )
        .unwrap();
    let got = store.get_rss_feed(&dek, &id).unwrap().unwrap();
    assert_eq!(got.poll_interval_minutes, DEFAULT_POLL_INTERVAL_MINUTES);
}

#[test]
fn add_feed_respects_explicit_interval() {
    let store = Store::open_memory().unwrap();
    let dek = Key32::generate();
    let id = store
        .add_rss_feed(
            &dek,
            &RssFeedInput {
                name: "fast".into(),
                url: "https://ex.com/feed.xml".into(),
                poll_interval_minutes: Some(5),
            },
        )
        .unwrap();
    let got = store.get_rss_feed(&dek, &id).unwrap().unwrap();
    assert_eq!(got.poll_interval_minutes, 5);
}
