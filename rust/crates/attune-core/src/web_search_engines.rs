// npu-vault/crates/vault-core/src/web_search_engines.rs
//
// 搜索引擎策略接口：把 DuckDuckGo / Google / Bing 等不同引擎的 DOM 解析逻辑隔离，
// 每加一个引擎只增加一个 impl block，不改动 BrowserSearchProvider。

use crate::web_search::WebSearchResult;

/// 搜索引擎策略：负责 URL 构造和 HTML 解析
pub trait SearchEngineStrategy: Send + Sync {
    /// 给定查询词，返回请求 URL
    fn build_url(&self, query: &str) -> String;
    /// 给定 HTML 响应，解析成结果列表
    fn parse(&self, html: &str, limit: usize) -> Vec<WebSearchResult>;
    /// 引擎名，用于日志和调试
    fn name(&self) -> &str;
}

/// DuckDuckGo HTML 端点引擎（对爬虫友好）
pub struct DuckDuckGoEngine;

impl SearchEngineStrategy for DuckDuckGoEngine {
    fn build_url(&self, query: &str) -> String {
        let encoded = urlencoding::encode(query);
        format!("https://html.duckduckgo.com/html/?q={encoded}")
    }

    fn parse(&self, html: &str, limit: usize) -> Vec<WebSearchResult> {
        use scraper::{Html, Selector};

        let document = Html::parse_document(html);
        let result_sel = Selector::parse("div.result").expect("result selector");
        let title_sel = Selector::parse("a.result__a").expect("title selector");
        let snippet_sel =
            Selector::parse("a.result__snippet, .result__snippet").expect("snippet selector");

        let mut results = Vec::new();
        for node in document.select(&result_sel).take(limit) {
            let title_el = match node.select(&title_sel).next() {
                Some(t) => t,
                None => continue,
            };
            let title = title_el.text().collect::<String>().trim().to_string();
            let url = title_el.value().attr("href").unwrap_or("").to_string();
            if title.is_empty() || url.is_empty() {
                continue;
            }
            let snippet = node
                .select(&snippet_sel)
                .next()
                .map(|s| s.text().collect::<String>().trim().to_string())
                .unwrap_or_default();

            results.push(WebSearchResult {
                title,
                url,
                snippet,
                published_date: None,
            });
        }
        results
    }

    fn name(&self) -> &str {
        "duckduckgo"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duckduckgo_parses_sample_html() {
        let html = include_str!("../tests/fixtures/duckduckgo_sample.html");
        let engine = DuckDuckGoEngine;
        let results = engine.parse(html, 5);

        assert_eq!(results.len(), 3, "sample has 3 results");
        assert_eq!(results[0].title, "第一个结果标题");
        assert_eq!(results[0].url, "https://example.com/first");
        assert!(results[0].snippet.contains("第一个结果的摘要"));
        assert_eq!(results[1].title, "Second Result Title");
    }

    #[test]
    fn duckduckgo_respects_limit() {
        let html = include_str!("../tests/fixtures/duckduckgo_sample.html");
        let engine = DuckDuckGoEngine;
        let results = engine.parse(html, 2);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn duckduckgo_builds_url() {
        let engine = DuckDuckGoEngine;
        let url = engine.build_url("rust async");
        assert!(url.starts_with("https://html.duckduckgo.com/html/"));
        assert!(url.contains("q=rust"));
    }
}
