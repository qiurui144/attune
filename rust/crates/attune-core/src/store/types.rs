//! Store 层共享类型（DTO / 行映射结构体）
//!
//! 抽出来集中管理 - 让 mod.rs 专注 schema + open/migrate，
//! impl Store 的具体方法散落在 items.rs / dirs.rs / ... 等子模块中。

use serde::{Deserialize, Serialize};

use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};

pub(super) struct RawItem {
    pub(super) id: String,
    pub(super) title: String,
    pub(super) content: Vec<u8>,
    pub(super) url: Option<String>,
    pub(super) source_type: String,
    pub(super) domain: Option<String>,
    pub(super) tags: Option<Vec<u8>>,
    pub(super) created_at: String,
    pub(super) updated_at: String,
}

impl RawItem {
    pub(super) fn decrypt(self, dek: &Key32) -> Result<DecryptedItem> {
        let content = String::from_utf8(crypto::decrypt(dek, &self.content)?)
            .map_err(|e| VaultError::Crypto(format!("utf8: {e}")))?;
        // tags 字段兼容两种历史格式：
        //   1. 老版：Vec<String>（手工标签）
        //   2. 新版：ClassificationResult（AI 分类结果，是 JSON map 带 core/universal/plugin/user_tags）
        // 新版反序列化为 Vec<String> 会 "invalid type: map, expected a sequence"
        // 导致整条 item 无法 decrypt，进而把 get_item / 搜索全链路阻塞。
        // 策略：先尝试 Vec<String>；失败则解为 Value 提取 user_tags / 或返回空 Vec。
        let tags: Option<Vec<String>> = match self.tags {
            Some(ref enc) => {
                let plain = crypto::decrypt(dek, enc)?;
                let parsed: Option<Vec<String>> = serde_json::from_slice::<Vec<String>>(&plain)
                    .ok()
                    .or_else(|| {
                        // 新版：ClassificationResult 格式。读取 user_tags（如果有）或降级为空
                        serde_json::from_slice::<serde_json::Value>(&plain).ok().map(|v| {
                            v.get("user_tags")
                                .and_then(|t| t.as_array())
                                .map(|arr| arr.iter()
                                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                    .collect())
                                .unwrap_or_default()
                        })
                    });
                parsed
            }
            None => None,
        };
        Ok(DecryptedItem {
            id: self.id,
            title: self.title,
            content,
            url: self.url,
            source_type: self.source_type,
            domain: self.domain,
            tags,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DecryptedItem {
    pub id: String,
    pub title: String,
    pub content: String,
    pub url: Option<String>,
    pub source_type: String,
    pub domain: Option<String>,
    pub tags: Option<Vec<String>>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ItemSummary {
    pub id: String,
    pub title: String,
    pub source_type: String,
    pub domain: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StaleItemSummary {
    pub id: String,
    pub title: String,
    pub source_type: String,
    pub updated_at: String,
    pub created_at: String,
}

#[derive(Debug, serde::Serialize)]
pub struct ItemStats {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub chunk_count: i64,
    pub embedding_pending: i64,
    pub embedding_done: i64,
}

/// Embedding 队列任务
#[derive(Debug)]
pub struct QueueTask {
    pub id: i64,
    pub item_id: String,
    pub chunk_idx: i32,
    pub chunk_text: String,
    pub level: i32,
    pub section_idx: i32,
    pub priority: i32,
    pub attempts: i32,
    pub task_type: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BoundDirRow {
    pub id: String,
    pub path: String,
    pub recursive: bool,
    pub file_types: String,
    pub last_scan: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchHistoryRow {
    pub id: i64,
    pub query: String,
    pub result_count: usize,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct IndexedFileRow {
    pub id: String,
    pub dir_id: String,
    pub path: String,
    pub file_hash: String,
    pub item_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Citation {
    pub item_id: String,
    pub title: String,
    pub relevance: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConvMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub citations: Vec<Citation>,
    pub created_at: String,
}

/// 技能进化信号：一次本地搜索失败记录
#[derive(Debug, Clone)]
pub struct SkillSignal {
    pub id: i64,
    pub query: String,
    pub knowledge_count: usize,
    pub web_used: bool,
    pub created_at: String,
}

/// 批注记录 — content 已解密
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Annotation {
    pub id: String,
    pub item_id: String,
    pub offset_start: i64,
    pub offset_end: i64,
    pub text_snippet: String,
    pub label: Option<String>,
    pub color: String,
    /// 批注内容（用户自由输入），空 = 纯高亮无附注
    pub content: String,
    /// user | ai
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
}

/// 创建/更新批注时的字段（id + 时间戳由服务器填充）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnotationInput {
    pub offset_start: i64,
    pub offset_end: i64,
    pub text_snippet: String,
    pub label: Option<String>,
    pub color: String,
    pub content: String,
    /// 默认 "user"；AI 路径会传 "ai"
    #[serde(default)]
    pub source: Option<String>,
}

// ============================================================================
// Project / Case 卷宗（spec §2.1）
// ============================================================================

/// 通用 Project 类型：行业层（attune-pro 系列插件）通过 metadata_encrypted
/// 持有自己 schema 的 opaque blob。attune-core 仅负责 kind 路由 + 时间线 + 文件归属，
/// **不约束** kind 的取值集合 — 任意行业字符串（'generic' / 'case' / 'deal' / 'topic' /
/// 插件自定义）都被允许，由调用方自行约定。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub title: String,
    /// 'generic' / 'case' / 'deal' / 'topic' / 任意 plugin 自定义类型 — attune-core 不约束。
    pub kind: String,
    /// 行业层在此存 opaque blob（如 attune-pro/law 的 case_no/parties/court 序列化 + AES-GCM 加密）。
    /// attune-core 不解析。
    pub metadata_encrypted: Option<Vec<u8>>,
    pub created_at: i64,
    pub updated_at: i64,
    pub archived: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectFile {
    pub project_id: String,
    pub file_id: String,
    /// 文件在该 project 中的角色，由 plugin / 调用方约定。空字符串表示未分类。
    /// attune-core 不约束取值集合。
    pub role: String,
    pub added_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectTimelineEntry {
    pub project_id: String,
    /// 毫秒级时间戳（比一般 timestamp 精度高，便于排序时间相近事件）
    pub ts_ms: i64,
    /// `fact` / `evidence_added` / `rpa_call` / `ai_inference` 等
    pub event_type: String,
    pub payload_encrypted: Option<Vec<u8>>,
}

/// A1 Memory Consolidation：聚合的情景记忆行（解密后）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRow {
    pub id: String,
    pub kind: String,            // 'episodic'（W5+ 加 'semantic'）
    pub window_start: i64,       // unix epoch 秒
    pub window_end: i64,
    pub source_chunk_hashes: Vec<String>,  // 升序
    pub summary: String,         // 已解密
    pub model: String,
    pub created_at: i64,
}
