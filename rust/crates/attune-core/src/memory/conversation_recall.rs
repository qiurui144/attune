//! Loose-conversation lazy recall — the "working memory" of a single loose chat.
//!
//! chat-centric IA (2026-06-26). A loose conversation (`project_id = NULL`) does NOT
//! persist scoped memory rows: its own recall is derived **on demand** from the last
//! K turns of that conversation's messages and never written to the `memories` table.
//! This gives a natural TTL (the recall disappears when the conversation is deleted)
//! and zero storage cost, while still letting a loose chat reference its own recent
//! context. Cross-conversation isolation is automatic — only this conversation's
//! messages are read, so one loose chat can never see another's working memory.

use crate::crypto::Key32;
use crate::error::Result;
use crate::memory::ContextBlock;
use crate::store::Store;

/// The synthetic tier tag for a derived loose-conversation recall block.
pub const CONVERSATION_RECALL_TIER: &str = "CONV";

/// Derive a single lightweight context block from the most recent `max_turns`
/// messages of conversation `conv_id`. Returns `None` when the conversation has no
/// messages (nothing to recall). The block is **not** persisted — no `memories` row
/// is written — so a loose conversation's recall has a natural TTL.
///
/// `max_turns` counts individual messages (user / assistant), most-recent-last; the
/// block content joins them in chronological order so the LLM reads them naturally.
pub fn derive_conversation_recall(
    store: &Store,
    dek: &Key32,
    conv_id: &str,
    max_turns: usize,
) -> Result<Option<ContextBlock>> {
    if max_turns == 0 {
        return Ok(None);
    }
    let messages = store.get_conversation_messages(dek, conv_id)?;
    if messages.is_empty() {
        return Ok(None);
    }
    // Keep the last `max_turns` messages in chronological order.
    let start = messages.len().saturating_sub(max_turns);
    let recent = &messages[start..];
    let content = recent
        .iter()
        .map(|m| format!("{}: {}", m.role, m.content))
        .collect::<Vec<_>>()
        .join("\n");
    if content.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(ContextBlock {
        title: "本次对话上下文".to_string(),
        content,
        // Score is informational only — this block is appended, not gated.
        score: 1.0,
        tier: CONVERSATION_RECALL_TIER,
        item_id: String::new(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_scope_derives_recall_from_messages_not_db() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let c = store.create_conversation(&dek, "t", None).unwrap();
        store
            .append_conversation_turn(&dek, &c, "我在研究 tokio", "tokio 是异步运行时", &[])
            .unwrap();
        // No memory row is written — recall is purely derived.
        assert_eq!(store.memory_count().unwrap(), 0);
        let block = derive_conversation_recall(&store, &dek, &c, 6).unwrap();
        let block = block.expect("recall block present");
        assert_eq!(block.tier, CONVERSATION_RECALL_TIER);
        assert!(block.item_id.is_empty(), "recall block has no source item");
        assert!(block.content.contains("tokio"), "recall must include the message content");
        // Still no memory row after deriving.
        assert_eq!(store.memory_count().unwrap(), 0);
    }

    #[test]
    fn empty_conversation_yields_no_recall() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let c = store.create_conversation(&dek, "t", None).unwrap();
        assert!(derive_conversation_recall(&store, &dek, &c, 6).unwrap().is_none());
    }

    #[test]
    fn missing_conversation_yields_no_recall() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        assert!(derive_conversation_recall(&store, &dek, "nope", 6).unwrap().is_none());
    }

    #[test]
    fn zero_max_turns_yields_no_recall() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let c = store.create_conversation(&dek, "t", None).unwrap();
        store
            .append_conversation_turn(&dek, &c, "hi", "hello", &[])
            .unwrap();
        assert!(derive_conversation_recall(&store, &dek, &c, 0).unwrap().is_none());
    }

    #[test]
    fn recall_keeps_only_most_recent_turns() {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let c = store.create_conversation(&dek, "t", None).unwrap();
        store.append_conversation_turn(&dek, &c, "first-q", "first-a", &[]).unwrap();
        store.append_conversation_turn(&dek, &c, "second-q", "second-a", &[]).unwrap();
        // 4 messages total; keep only the last 2 → should not contain the first turn.
        let block = derive_conversation_recall(&store, &dek, &c, 2).unwrap().unwrap();
        assert!(block.content.contains("second-a"));
        assert!(!block.content.contains("first-q"), "older turns trimmed");
    }
}
