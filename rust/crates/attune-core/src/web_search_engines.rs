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

/// DuckDuckGo HTML 端点的结果链接是重定向形式
/// `//duckduckgo.com/l/?uddg=<percent-encoded 真实 URL>&rut=...`。
/// 解出 `uddg` 参数还原真实目标 URL；非重定向形式仅做协议相对前缀归一。
/// 这样下游（引用展示 / Chat citation）拿到的是干净 `https://` 真实链接。
fn unwrap_ddg_redirect(href: &str) -> String {
    let normalized = href
        .strip_prefix("//")
        .map(|rest| format!("https://{rest}"))
        .unwrap_or_else(|| href.to_string());
    if !normalized.contains("duckduckgo.com/l/") {
        return normalized;
    }
    if let Some(query) = normalized.split('?').nth(1) {
        for pair in query.split('&') {
            if let Some(encoded) = pair.strip_prefix("uddg=") {
                if let Ok(decoded) = urlencoding::decode(encoded) {
                    let real = decoded.into_owned();
                    if real.starts_with("http") {
                        return real;
                    }
                }
            }
        }
    }
    normalized
}

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
            let url = unwrap_ddg_redirect(title_el.value().attr("href").unwrap_or(""));
            // 只接受能还原出的真实 http(s) 链接；相对路径 / 残缺 href 丢弃，
            // 保证结果 URL 恒以 http 开头（下游引用展示 / citation 契约）。
            if title.is_empty() || !url.starts_with("http") {
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

    #[test]
    fn unwrap_ddg_redirect_decodes_real_url() {
        // DDG /l/ 重定向 → 解出 uddg 真实 URL
        let redirect = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F&rut=abc";
        assert_eq!(unwrap_ddg_redirect(redirect), "https://www.rust-lang.org/");
        // 结果必以 http 开头 —— real_duckduckgo_search 的断言契约
        assert!(unwrap_ddg_redirect(redirect).starts_with("http"));
        // 直接 URL → 原样返回
        assert_eq!(unwrap_ddg_redirect("https://example.com/x"), "https://example.com/x");
        // 协议相对非重定向 → 归一为 https
        assert_eq!(unwrap_ddg_redirect("//example.com/y"), "https://example.com/y");
    }
}
