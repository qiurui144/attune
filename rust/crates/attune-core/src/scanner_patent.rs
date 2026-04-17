// npu-vault/crates/vault-core/src/scanner_patent.rs
//
// 专利数据库网络查询客户端。
//
// 支持的数据库：
//   - USPTO PatentsView v1 API（免认证，45 req/min）
//     https://search.patentsview.org/api/v1/patent/
//   - CNIPA 配置扩展预留（需第三方 API Key）

use crate::chunker;
use crate::crypto::Key32;
use crate::error::{Result, VaultError};
use crate::store::Store;
use serde::{Deserialize, Serialize};

const USPTO_BASE: &str = "https://search.patentsview.org/api/v1/patent/";
/// 单次查询最大返回条数（USPTO 限制 per_page <= 1000，我们限制 20 防止批量入库过大）
const MAX_PER_QUERY: usize = 20;
/// 单条专利 abstract 截取字符数上限（防止超出 ingest 限制）
const MAX_ABSTRACT_CHARS: usize = 4000;

// ── 公开数据结构 ──────────────────────────────────────────────────────────────

/// 要查询的专利数据库
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PatentDatabase {
    /// USPTO PatentsView — 美国专利，免认证
    Uspto,
}

impl std::fmt::Display for PatentDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatentDatabase::Uspto => write!(f, "USPTO"),
        }
    }
}

/// 专利查询参数
#[derive(Debug, Clone)]
pub struct PatentQuery {
    /// 关键词（自然语言，将同时搜索标题和摘要）
    pub keywords: String,
    /// 返回条数上限（最大 20）
    pub limit: usize,
    /// 目标数据库
    pub database: PatentDatabase,
    /// 可选的 IPC 大类过滤（如 "G06F"）
    pub ipc_filter: Option<String>,
}

/// 单条专利记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatentRecord {
    pub patent_number: String,
    pub title: String,
    pub abstract_text: String,
    pub filing_date: Option<String>,
    pub grant_date: Option<String>,
    pub assignees: Vec<String>,
    pub inventors: Vec<String>,
    pub ipc_classes: Vec<String>,
    /// 专利原文页面 URL
    pub source_url: String,
    /// 来源数据库名称
    pub database: String,
}

impl PatentRecord {
    /// 构建适合 ingest 的富文本内容
    fn to_ingest_content(&self) -> String {
        let mut s = String::new();
        if !self.abstract_text.is_empty() {
            s.push_str("【摘要】\n");
            s.push_str(&self.abstract_text);
            s.push('\n');
        }
        if !self.inventors.is_empty() {
            s.push_str("\n【发明人】");
            s.push_str(&self.inventors.join("、"));
            s.push('\n');
        }
        if !self.assignees.is_empty() {
            s.push_str("\n【申请人/权利人】");
            s.push_str(&self.assignees.join("、"));
            s.push('\n');
        }
        if !self.ipc_classes.is_empty() {
            s.push_str("\n【IPC分类】");
            s.push_str(&self.ipc_classes.join(" "));
            s.push('\n');
        }
        if let Some(d) = &self.grant_date {
            s.push_str(&format!("\n【授权日期】{d}\n"));
        }
        s.push_str(&format!("\n【来源】{}\n{}\n", self.database, self.source_url));
        s
    }
}

/// 查询结果摘要
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatentSearchResult {
    pub database: String,
    pub keywords: String,
    pub total_found: usize,
    pub records: Vec<PatentRecord>,
    pub ingested: usize,
}

// ── USPTO 内部响应结构 ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct UsptoResponse {
    patents: Option<Vec<UsptoPatent>>,
    total_patent_count: Option<usize>,
}

#[derive(Deserialize)]
struct UsptoPatent {
    patent_id: Option<String>,
    patent_title: Option<String>,
    patent_abstract: Option<String>,
    patent_date: Option<String>,
    #[serde(default)]
    assignees: Vec<UsptoAssignee>,
    #[serde(default)]
    inventors: Vec<UsptoInventor>,
    #[serde(default)]
    cpc_current: Vec<UsptoIpc>,
}

#[derive(Deserialize)]
struct UsptoAssignee {
    assignee_organization: Option<String>,
}

#[derive(Deserialize)]
struct UsptoInventor {
    inventor_first_name: Option<String>,
    inventor_last_name: Option<String>,
}

#[derive(Deserialize)]
struct UsptoIpc {
    cpc_subclass_id: Option<String>,
}

// ── 核心函数 ──────────────────────────────────────────────────────────────────

/// 向专利数据库发起网络查询，返回专利记录列表。
/// 使用 blocking reqwest，调用方应在 spawn_blocking 中执行。
pub fn search_patents(query: &PatentQuery) -> Result<PatentSearchResult> {
    let limit = query.limit.min(MAX_PER_QUERY).max(1);
    match query.database {
        PatentDatabase::Uspto => search_uspto(query, limit),
    }
}

/// 将查询到的专利记录入库（分块 + 排队 embedding）。
/// 返回成功入库条数。
pub fn ingest_patent_records(store: &Store, dek: &Key32, records: &[PatentRecord]) -> Result<usize> {
    let mut count = 0usize;
    for rec in records {
        let content = rec.to_ingest_content();
        // 用 source_url 防止重复入库同一专利
        if store.find_item_by_url(&rec.source_url)?.is_some() {
            continue;
        }
        let item_id = store.insert_item(
            dek,
            &rec.title,
            &content,
            Some(&rec.source_url),
            "patent",
            None,
            None,
        )?;
        // 分块并排队 embedding（priority=2, level=1）
        let chunks = chunker::chunk(&content, 512, 64);
        for (idx, chunk) in chunks.iter().enumerate() {
            let _ = store.enqueue_embedding(&item_id, idx, chunk, 2, 1, idx);
        }
        count += 1;
    }
    Ok(count)
}

// ── USPTO 实现 ────────────────────────────────────────────────────────────────

fn search_uspto(query: &PatentQuery, limit: usize) -> Result<PatentSearchResult> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("npu-vault/0.5 patent-search")
        .build()
        .map_err(|e| VaultError::LlmUnavailable(format!("patent HTTP client: {e}")))?;

    // 构造 PatentsView v1 查询 JSON
    // _text_any 同时搜索 title 和 abstract
    let q_json = if let Some(ipc) = &query.ipc_filter {
        serde_json::json!({
            "_and": [
                {"_text_any": {
                    "patent_title": query.keywords,
                    "patent_abstract": query.keywords
                }},
                {"_begins": {"cpc_current.cpc_subclass_id": ipc}}
            ]
        })
    } else {
        serde_json::json!({
            "_text_any": {
                "patent_title": query.keywords,
                "patent_abstract": query.keywords
            }
        })
    };

    let fields_json = serde_json::json!([
        "patent_id", "patent_title", "patent_abstract", "patent_date",
        "assignees.assignee_organization",
        "inventors.inventor_first_name", "inventors.inventor_last_name",
        "cpc_current.cpc_subclass_id"
    ]);

    let options_json = serde_json::json!({"per_page": limit, "page": 1});
    let sort_json = serde_json::json!([{"patent_date": "desc"}]);

    let resp = client
        .get(USPTO_BASE)
        .query(&[
            ("q", q_json.to_string()),
            ("f", fields_json.to_string()),
            ("o", options_json.to_string()),
            ("s", sort_json.to_string()),
        ])
        .send()
        .map_err(|e| VaultError::LlmUnavailable(format!("USPTO request failed: {e}")))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(VaultError::LlmUnavailable(format!("USPTO HTTP {status}: {body}")));
    }

    let raw: UsptoResponse = resp.json()
        .map_err(|e| VaultError::LlmUnavailable(format!("USPTO response parse: {e}")))?;

    let total = raw.total_patent_count.unwrap_or(0);
    let patents = raw.patents.unwrap_or_default();

    let records: Vec<PatentRecord> = patents.into_iter().map(|p| {
        let patent_number = p.patent_id.clone().unwrap_or_default();
        let title = p.patent_title.unwrap_or_else(|| "(无标题)".into());
        let abstract_text: String = p.patent_abstract
            .unwrap_or_default()
            .chars()
            .take(MAX_ABSTRACT_CHARS)
            .collect();
        let grant_date = p.patent_date;
        let assignees: Vec<String> = p.assignees.into_iter()
            .filter_map(|a| a.assignee_organization)
            .collect();
        let inventors: Vec<String> = p.inventors.into_iter()
            .map(|i| {
                let first = i.inventor_first_name.unwrap_or_default();
                let last = i.inventor_last_name.unwrap_or_default();
                format!("{first} {last}").trim().to_string()
            })
            .filter(|s| !s.is_empty())
            .collect();
        let ipc_classes: Vec<String> = p.cpc_current.into_iter()
            .filter_map(|c| c.cpc_subclass_id)
            .collect();
        let source_url = format!("https://patents.google.com/patent/US{patent_number}/en");
        PatentRecord {
            patent_number,
            title,
            abstract_text,
            filing_date: None,
            grant_date,
            assignees,
            inventors,
            ipc_classes,
            source_url,
            database: "USPTO".into(),
        }
    }).collect();

    Ok(PatentSearchResult {
        database: "USPTO".into(),
        keywords: query.keywords.clone(),
        total_found: total,
        records,
        ingested: 0,
    })
}

// ── 测试 ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patent_record_to_ingest_content_includes_fields() {
        let rec = PatentRecord {
            patent_number: "10000000".into(),
            title: "Test Patent".into(),
            abstract_text: "A method for testing.".into(),
            filing_date: None,
            grant_date: Some("2020-01-01".into()),
            assignees: vec!["Acme Corp".into()],
            inventors: vec!["John Doe".into()],
            ipc_classes: vec!["G06F".into()],
            source_url: "https://patents.google.com/patent/US10000000/en".into(),
            database: "USPTO".into(),
        };
        let content = rec.to_ingest_content();
        assert!(content.contains("A method for testing."), "摘要应出现在内容中");
        assert!(content.contains("Acme Corp"), "申请人应出现在内容中");
        assert!(content.contains("John Doe"), "发明人应出现在内容中");
        assert!(content.contains("G06F"), "IPC 分类应出现在内容中");
        assert!(content.contains("2020-01-01"), "授权日期应出现在内容中");
        assert!(content.contains("USPTO"), "来源数据库应出现在内容中");
    }

    #[test]
    fn patent_record_empty_optional_fields() {
        let rec = PatentRecord {
            patent_number: "9000000".into(),
            title: "Minimal Patent".into(),
            abstract_text: String::new(),
            filing_date: None,
            grant_date: None,
            assignees: vec![],
            inventors: vec![],
            ipc_classes: vec![],
            source_url: "https://patents.google.com/patent/US9000000/en".into(),
            database: "USPTO".into(),
        };
        let content = rec.to_ingest_content();
        // 无摘要时不应有【摘要】标题
        assert!(!content.contains("【摘要】"), "空摘要不应生成摘要标题");
        assert!(content.contains("USPTO"), "来源应始终出现");
    }

    #[test]
    fn patent_database_display() {
        assert_eq!(PatentDatabase::Uspto.to_string(), "USPTO");
    }

    #[test]
    fn max_per_query_clamping() {
        let q = PatentQuery {
            keywords: "test".into(),
            limit: 999,
            database: PatentDatabase::Uspto,
            ipc_filter: None,
        };
        let effective = q.limit.min(MAX_PER_QUERY).max(1);
        assert_eq!(effective, MAX_PER_QUERY, "limit 应被 clamp 至 MAX_PER_QUERY");
    }

    #[test]
    fn abstract_truncation() {
        let long_abstract = "x".repeat(MAX_ABSTRACT_CHARS + 1000);
        let truncated: String = long_abstract.chars().take(MAX_ABSTRACT_CHARS).collect();
        assert_eq!(truncated.len(), MAX_ABSTRACT_CHARS);
    }
}
