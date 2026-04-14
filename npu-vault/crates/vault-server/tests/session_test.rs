// ── Chat Session CRUD 集成测试（直接测 Store，无需 HTTP server）────────────

#[cfg(test)]
mod chat_session_tests {
    use vault_core::crypto::Key32;
    use vault_core::store::Store;

    fn make_dek() -> Key32 {
        Key32::generate()
    }

    #[test]
    fn test_session_lifecycle() {
        let store = Store::open_memory().unwrap();
        let dek = make_dek();

        // 创建 session
        let sid = store.create_conversation(&dek, "专利检索会话").unwrap();
        assert!(!sid.is_empty());

        // 写入消息
        store.append_message(&dek, &sid, "user", "分析权利要求书", &[]).unwrap();
        store.append_message(&dek, &sid, "assistant", "权利要求书分析如下...", &[]).unwrap();

        // 读取消息
        let msgs = store.get_conversation_messages(&dek, &sid).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].content, "分析权利要求书");
        assert_eq!(msgs[1].content, "权利要求书分析如下...");

        // 列出 sessions
        let list = store.list_conversations(10, 0).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, sid);

        // 删除 session（级联删消息）
        store.delete_conversation(&sid).unwrap();
        let msgs_after = store.get_conversation_messages(&dek, &sid).unwrap();
        assert!(msgs_after.is_empty());
    }

    #[test]
    fn test_session_pagination() {
        let store = Store::open_memory().unwrap();
        let dek = make_dek();
        for i in 0..5 {
            store.create_conversation(&dek, &format!("会话{}", i)).unwrap();
        }
        let page1 = store.list_conversations(3, 0).unwrap();
        let page2 = store.list_conversations(3, 3).unwrap();
        assert_eq!(page1.len(), 3);
        assert_eq!(page2.len(), 2);
    }

    #[test]
    fn test_session_updated_at_changes_on_append() {
        let store = Store::open_memory().unwrap();
        let dek = make_dek();
        let sid = store.create_conversation(&dek, "测试").unwrap();
        let before = store.list_conversations(1, 0).unwrap()[0].updated_at.clone();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        store.append_message(&dek, &sid, "user", "消息", &[]).unwrap();
        let after = store.list_conversations(1, 0).unwrap()[0].updated_at.clone();
        assert_ne!(before, after, "updated_at 应在 append_message 后更新");
    }
}
