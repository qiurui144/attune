// ── Chat Session CRUD 集成测试（直接测 Store，无需 HTTP server）────────────

#[cfg(test)]
mod chat_session_tests {
    use attune_core::crypto::Key32;
    use attune_core::store::Store;

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
        let list = store.list_conversations(&dek, 10, 0).unwrap();
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
        let page1 = store.list_conversations(&dek, 3, 0).unwrap();
        let page2 = store.list_conversations(&dek, 3, 3).unwrap();
        assert_eq!(page1.len(), 3);
        assert_eq!(page2.len(), 2);
    }

    #[test]
    fn test_session_updated_at_changes_on_append() {
        let store = Store::open_memory().unwrap();
        let dek = make_dek();
        let sid = store.create_conversation(&dek, "测试").unwrap();
        let before = store.list_conversations(&dek, 1, 0).unwrap()[0].updated_at.clone();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        store.append_message(&dek, &sid, "user", "消息", &[]).unwrap();
        let after = store.list_conversations(&dek, 1, 0).unwrap()[0].updated_at.clone();
        assert_ne!(before, after, "updated_at 应在 append_message 后更新");
    }

    // #11: POST /chat 带/不带 session_id（Store 层验证）
    #[test]
    fn test_chat_session_auto_created_when_no_session_id() {
        // 模拟 chat.rs 中 session_id=None 时自动创建 session 的逻辑
        let store = Store::open_memory().unwrap();
        let dek = make_dek();

        // 没有 session_id → 创建新 session
        let title = "第一条消息的前50字".to_string();
        let sid = store.create_conversation(&dek, &title).unwrap();
        store.append_message(&dek, &sid, "user", "用户问题", &[]).unwrap();
        store.append_message(&dek, &sid, "assistant", "助手回答", &[]).unwrap();

        let msgs = store.get_conversation_messages(&dek, &sid).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[1].role, "assistant");
    }

    #[test]
    fn test_chat_session_reuse_existing_session_id() {
        // 模拟 chat.rs 中传入有效 session_id 时复用
        let store = Store::open_memory().unwrap();
        let dek = make_dek();

        let sid = store.create_conversation(&dek, "已有会话").unwrap();
        store.append_message(&dek, &sid, "user", "第一轮问题", &[]).unwrap();

        // 第二轮：传入已有 session_id
        assert!(store.get_conversation_by_id(&dek, &sid).unwrap().is_some());
        store.append_message(&dek, &sid, "user", "第二轮问题", &[]).unwrap();
        store.append_message(&dek, &sid, "assistant", "第二轮回答", &[]).unwrap();

        let msgs = store.get_conversation_messages(&dek, &sid).unwrap();
        assert_eq!(msgs.len(), 3); // 两轮 user + 一轮 assistant
    }

    // #14: DELETE /sessions/:id 返回 204（Store 层验证）
    #[test]
    fn test_delete_conversation_returns_no_content() {
        let store = Store::open_memory().unwrap();
        let dek = make_dek();

        let sid = store.create_conversation(&dek, "要删除的会话").unwrap();
        // 验证存在
        assert!(store.get_conversation_by_id(&dek, &sid).unwrap().is_some());
        // 删除
        store.delete_conversation(&sid).unwrap();
        // 验证已删除
        assert!(store.get_conversation_by_id(&dek, &sid).unwrap().is_none());
        // 列表为空
        assert!(store.list_conversations(&dek, 10, 0).unwrap().is_empty());
    }
}
