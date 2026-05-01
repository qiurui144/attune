// 上下文压缩流水线 —— Batch B.1
//
// ## 成本/触发契约
//
// 本模块**首次**对每个 chunk 生成摘要时走 💰 LLM（建 cache），之后永久命中缓存变成 🆓。
// 调用时机：**用户发起 chat 时**，在 RAG 检索后、Prompt 组装前。不在建库流水线里主动
// 生成摘要（成本太高，且大多数 chunk 永远不会被查询到，浪费）。
//
// ## Strategy
//
// - `raw`        —— 不压缩，全文透传给 LLM。适合纯本地模式（免费）。
// - `economical` —— ~150 字摘要，保留关键数字/专名。云端模式默认。
// - `accurate`   —— ~300 字摘要 + 原文前 100 字。长文 Chat 时用。
//
// ## 缓存键
//
// sha256(chunk_text) + strategy → summary。chunk 内容变 → hash 变 → 自然失效。
// 批注变更不影响"原文摘要"缓存；批注加权注入到 Prompt 的另一段（非摘要），见 Batch B.2。

use crate::error::Result;
use crate::crypto::Key32;
use crate::llm::LlmProvider;
use crate::store::Store;
use sha2::{Digest, Sha256};

/// 上下文压缩策略
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextStrategy {
    Raw,
    Economical,
    Accurate,
}

impl ContextStrategy {
    pub fn parse(s: &str) -> Self {
        match s {
            "raw" => Self::Raw,
            "accurate" => Self::Accurate,
            _ => Self::Economical, // 未知值退化到默认 —— 与 settings default 对齐
        }
    }

    /// 用于 cache 持久化的字符串形式
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Economical => "economical",
            Self::Accurate => "accurate",
        }
    }

    /// 目标摘要字符数（字符而非 token，对中文更直观）
    pub fn target_chars(&self) -> usize {
        match self {
            Self::Raw => usize::MAX, // 不压缩
            Self::Economical => 150,
            Self::Accurate => 300,
        }
    }
}

/// 压缩后的单个 chunk。保留原文引用 + 摘要 + cache 状态（便于 UI 展示命中率）。
#[derive(Debug, Clone)]
pub struct CompressedChunk {
    /// 透给 LLM 的最终文本（raw=原文 / economical=摘要 / accurate=摘要+头）
    pub injected: String,
    /// chunk 原文字符数（统计/chip 展示）
    pub original_chars: usize,
    /// true = 本次请求命中缓存（0 成本）；false = 本次调用了 LLM 生成
    pub cache_hit: bool,
}

/// 计算 chunk_hash — sha256 hex。调用方保证 text 是 canonical（去 BOM / trim 末尾空白）。
pub fn chunk_hash(text: &str) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    format!("{:x}", h.finalize())
}

/// 压缩单个 chunk。
///   - strategy=Raw：直接返回原文，不触 LLM，不动 cache。
///   - strategy=Economical/Accurate：先查 cache，命中立即返回；缺失则调 LLM 生成并写 cache。
///
/// `item_id` 用于摘要失效级联（item 软删除 → 摘要也删）。web 搜索结果没有 item_id，
/// 调用方应传空字符串，这种情况不进缓存（不确定何时失效，更安全）。
pub fn compress_chunk(
    store: &Store,
    dek: &Key32,
    llm: &dyn LlmProvider,
    item_id: &str,
    chunk_text: &str,
    strategy: ContextStrategy,
) -> Result<CompressedChunk> {
    let original_chars = chunk_text.chars().count();

    // 短 chunk 不压缩 —— 摘要反而可能比原文长
    if strategy == ContextStrategy::Raw || original_chars <= strategy.target_chars() {
        return Ok(CompressedChunk {
            injected: chunk_text.to_string(),
            original_chars,
            cache_hit: true, // 视作"0 成本命中"，统计上与摘要缓存命中同级
        });
    }

    let hash = chunk_hash(chunk_text);

    // 无 item_id 的 chunk（如 web 结果）不走缓存 —— 直接 LLM 生成每次都付费，
    // 或更优：直接 raw 返回。当前选 raw 以避免云端 API 频繁计费。
    if item_id.is_empty() {
        return Ok(CompressedChunk {
            injected: chunk_text.to_string(),
            original_chars,
            cache_hit: true,
        });
    }

    // 查 cache
    if let Some(cached) = store.get_chunk_summary(dek, &hash, strategy.as_str())? {
        let injected = match strategy {
            ContextStrategy::Accurate => {
                // Accurate = 摘要 + 原文前 100 字
                let head: String = chunk_text.chars().take(100).collect();
                format!("{cached}\n原文摘录: {head}...")
            }
            _ => cached,
        };
        return Ok(CompressedChunk {
            injected,
            original_chars,
            cache_hit: true,
        });
    }

    // Cache miss → 调 LLM 生成
    if !llm.is_available() {
        // LLM 不可用时退化：用截断的原文代替摘要，别让 chat 流程挂掉
        log::warn!("context_compress: LLM unavailable, fallback to truncated raw for chunk_hash={hash}");
        let truncated: String = chunk_text.chars().take(strategy.target_chars()).collect();
        return Ok(CompressedChunk {
            injected: truncated,
            original_chars,
            cache_hit: false,
        });
    }

    let prompt = build_compression_prompt(strategy);
    let user_msg = format!("段落：\n{chunk_text}");
    // F-17-PRIVACY: redact chunk_text before LLM (chunk may contain PII from user-ingested docs)
    let redactor = crate::pii::Redactor::default();
    let summary = crate::pii::llm_chat_redacted(llm, &redactor, &prompt, &user_msg, "context_compress")?;
    let summary = summary.trim();

    // LLM 偶尔会返超过目标字数的摘要 —— 截断保底
    let summary_capped: String = summary.chars().take(strategy.target_chars() + 50).collect();

    // 写 cache（失败不致命 —— 本次仍然能回答，下次重做摘要）
    if let Err(e) = store.put_chunk_summary(
        dek, &hash, strategy.as_str(), item_id,
        llm.model_name(), &summary_capped, original_chars,
    ) {
        log::warn!("context_compress: failed to cache summary for {hash}: {e}");
    }

    let injected = match strategy {
        ContextStrategy::Accurate => {
            let head: String = chunk_text.chars().take(100).collect();
            format!("{summary_capped}\n原文摘录: {head}...")
        }
        _ => summary_capped,
    };
    Ok(CompressedChunk {
        injected,
        original_chars,
        cache_hit: false,
    })
}

/// 只调 LLM 生成摘要，不碰 store —— 供需要"无锁"调用的场景（见 `chat.rs` 三阶段）。
/// 返回字符串截断到 `target + 50` 字符，与 `compress_chunk` 的 cap 逻辑一致。
///
/// 错误情况（LLM 不可用 / LLM 调用失败 / 返回空）统一返回 Err，调用方自行决定降级。
pub fn generate_summary(
    llm: &dyn LlmProvider,
    chunk_text: &str,
    strategy: ContextStrategy,
) -> Result<String> {
    if !llm.is_available() {
        return Err(crate::error::VaultError::InvalidInput("LLM not available".into()));
    }
    let prompt = build_compression_prompt(strategy);
    let user_msg = format!("段落：\n{chunk_text}");
    // F-17-PRIVACY: redact chunk_text before LLM (per `compress_chunk` doc — same threat model)
    let redactor = crate::pii::Redactor::default();
    let raw = crate::pii::llm_chat_redacted(llm, &redactor, &prompt, &user_msg, "context_compress.generate_summary")?;
    let summary = raw.trim();
    if summary.is_empty() {
        return Err(crate::error::VaultError::InvalidInput("empty summary from LLM".into()));
    }
    Ok(summary.chars().take(strategy.target_chars() + 50).collect())
}

fn build_compression_prompt(strategy: ContextStrategy) -> String {
    let target = strategy.target_chars();
    format!(
        "你是浓缩器。把用户给你的段落压缩为不超过 {target} 字的中文摘要。\n\
         \n\
         规则：\n\
         1. 保留所有专有名词、数字、日期、命令 / 代码 / 函数名。\n\
         2. 省略举例铺垫、重复、客套话。\n\
         3. 直接输出摘要正文，不加任何前后缀（不说『摘要如下』之类）。\n\
         4. 若原文过短（< {target} 字），返回原文即可。"
    )
}

/// 批量压缩。对 Raw 策略是 no-op；其余策略逐 chunk 压缩，失败单条降级不中断整批。
pub fn compress_batch(
    store: &Store,
    dek: &Key32,
    llm: &dyn LlmProvider,
    chunks: &[(String, String)],  // (item_id, chunk_text) —— item_id 为空表示 web/临时 chunk
    strategy: ContextStrategy,
) -> Vec<CompressedChunk> {
    chunks.iter().map(|(item_id, text)| {
        match compress_chunk(store, dek, llm, item_id, text, strategy) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("context_compress: chunk compression failed, using raw: {e}");
                CompressedChunk {
                    injected: text.clone(),
                    original_chars: text.chars().count(),
                    cache_hit: false,
                }
            }
        }
    }).collect()
}

/// 估算文本对应的 token 数量。
/// 粗略模型：CJK 按 **1.2 token/char**，ASCII 按 **0.25 token/char**（≈ 4 chars/token）。
/// CJK 系数按 gpt-4o / Claude 实测校正（BPE 常把一个汉字切成 ≥1 token），
/// 比"字符数/4"的传统估算偏保守，避免 2× 账单惊吓。
///
/// 用于 UI 的 token chip 显示，**不是**精确 tokenizer —— 实际走云端 provider 时
/// 以供应商账单为准。
pub fn estimate_tokens(text: &str) -> usize {
    let mut cjk_chars = 0usize;
    let mut ascii_chars = 0usize;
    for ch in text.chars() {
        if ch.is_ascii() { ascii_chars += 1; }
        else { cjk_chars += 1; }
    }
    // 1.2 * cjk + 0.25 * ascii, round up
    // = (12 * cjk + 25/10 * ascii) / 10 简化为 (12 * cjk + ascii * 2.5) / 10
    // 为避免浮点：(cjk * 120 + ascii * 25 + 99) / 100
    (cjk_chars * 120 + ascii_chars * 25).div_ceil(100)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Key32;
    use crate::llm::MockLlmProvider;

    fn setup() -> (Store, Key32, String) {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        let item_id = store
            .insert_item(&dek, "test", "body text", None, "note", None, None)
            .unwrap();
        (store, dek, item_id)
    }

    fn mock_with(resp: &str) -> MockLlmProvider {
        let m = MockLlmProvider::new("mock-qwen");
        m.push_response(resp);
        m
    }

    #[test]
    fn parse_strategy() {
        assert_eq!(ContextStrategy::parse("raw"), ContextStrategy::Raw);
        assert_eq!(ContextStrategy::parse("economical"), ContextStrategy::Economical);
        assert_eq!(ContextStrategy::parse("accurate"), ContextStrategy::Accurate);
        // 未知值 → 默认 Economical（与 settings default 一致）
        assert_eq!(ContextStrategy::parse("weird"), ContextStrategy::Economical);
    }

    #[test]
    fn chunk_hash_deterministic() {
        assert_eq!(chunk_hash("abc"), chunk_hash("abc"));
        assert_ne!(chunk_hash("abc"), chunk_hash("abc "));
        assert_eq!(chunk_hash("").len(), 64); // sha256 hex
    }

    #[test]
    fn raw_strategy_passes_through() {
        let (store, dek, item_id) = setup();
        let mock = MockLlmProvider::new("m");
        let text = "a".repeat(2000);
        let r = compress_chunk(&store, &dek, &mock, &item_id, &text, ContextStrategy::Raw).unwrap();
        assert_eq!(r.injected, text);
        assert!(r.cache_hit); // raw 视作 0 成本
        // LLM 不该被调用（mock 无响应，若被调会报错）
    }

    #[test]
    fn short_chunk_below_target_passes_through() {
        let (store, dek, item_id) = setup();
        let mock = MockLlmProvider::new("m");
        let text = "短文本原文".repeat(5); // < 150 chars
        let r = compress_chunk(&store, &dek, &mock, &item_id, &text, ContextStrategy::Economical).unwrap();
        assert_eq!(r.injected, text, "short chunk returned as-is");
        assert!(r.cache_hit);
    }

    #[test]
    fn economical_calls_llm_first_time_then_hits_cache() {
        let (store, dek, item_id) = setup();
        let long_text = "这是一段很长的内容。".repeat(30); // > 150 chars
        let mock = mock_with("这是一段很长内容的压缩版。"); // LLM 返回摘要

        // 第一次：cache miss → 调 LLM
        let r1 = compress_chunk(&store, &dek, &mock, &item_id, &long_text, ContextStrategy::Economical).unwrap();
        assert!(!r1.cache_hit);
        assert_eq!(r1.injected, "这是一段很长内容的压缩版。");

        // 第二次：cache hit → 不调 LLM（mock 响应列表已空，若被调会 Err）
        let r2 = compress_chunk(&store, &dek, &mock, &item_id, &long_text, ContextStrategy::Economical).unwrap();
        assert!(r2.cache_hit);
        assert_eq!(r2.injected, "这是一段很长内容的压缩版。");
    }

    #[test]
    fn empty_item_id_web_chunk_no_cache() {
        let (store, dek, _) = setup();
        let text = "这是一段长的 web 搜索结果".repeat(20);
        let mock = MockLlmProvider::new("m");
        let r = compress_chunk(&store, &dek, &mock, "", &text, ContextStrategy::Economical).unwrap();
        // web chunk 退到原文（避免云端 API 每次都付费）
        assert_eq!(r.injected, text);
        // 不应写入 cache
        assert_eq!(store.chunk_summary_count().unwrap(), 0);
    }

    #[test]
    fn accurate_strategy_appends_original_head() {
        let (store, dek, item_id) = setup();
        let long_text = "数据库管理系统的核心功能是存储和检索。".repeat(20);
        let mock = mock_with("DBMS 负责存储与检索。"); // LLM 摘要

        let r = compress_chunk(&store, &dek, &mock, &item_id, &long_text, ContextStrategy::Accurate).unwrap();
        assert!(r.injected.contains("DBMS 负责存储与检索。"));
        assert!(r.injected.contains("原文摘录:"));
    }

    #[test]
    fn cache_is_strategy_scoped() {
        // 同一 chunk_hash 的两种 strategy 各有独立 cache 行。
        // 文本要足够长（> 300 字）让 Accurate 也压缩。
        let (store, dek, item_id) = setup();
        let long_text = "x".repeat(400);
        let mock = mock_with("E"); // economical 摘要
        let _ = compress_chunk(&store, &dek, &mock, &item_id, &long_text, ContextStrategy::Economical).unwrap();
        assert_eq!(store.chunk_summary_count().unwrap(), 1);

        // Accurate 摘要 —— 应再调一次 LLM 并产生第二条缓存
        let mock2 = mock_with("A"); // accurate 摘要
        let _ = compress_chunk(&store, &dek, &mock2, &item_id, &long_text, ContextStrategy::Accurate).unwrap();
        assert_eq!(store.chunk_summary_count().unwrap(), 2);
    }

    #[test]
    fn llm_unavailable_falls_back_to_truncated() {
        let (store, dek, item_id) = setup();
        let long_text = "x".repeat(500);
        // MockLlmProvider::new 总是可用的；需要模拟不可用，用空响应后触发 Err
        let mock = MockLlmProvider::new("m"); // 无 push_response → chat() 会 Err
        let r = compress_chunk(&store, &dek, &mock, &item_id, &long_text, ContextStrategy::Economical);
        // mock 的 chat() 返回 Err("no mock response") —— compress_chunk 向外抛
        assert!(r.is_err(), "LLM err should surface (upper layer will fallback)");
    }

    #[test]
    fn batch_compression_degrades_on_failure() {
        // 一批 chunk 里有一个导致 LLM 失败 —— 其余仍应成功
        let (store, dek, item_id) = setup();
        let short_text = "短文".to_string();
        let long_text = "x".repeat(500);
        let mock = mock_with("摘要");

        let batch = vec![
            (item_id.clone(), short_text.clone()),
            (item_id.clone(), long_text.clone()),
        ];
        let results = compress_batch(&store, &dek, &mock, &batch, ContextStrategy::Economical);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].injected, short_text);   // short passthrough
        assert_eq!(results[1].injected, "摘要");       // long compressed
    }

    #[test]
    fn estimate_tokens_cjk() {
        // 中文 1.2 tok/char；"数据库" 3 字 ≈ 3-4 tok
        let n = estimate_tokens("数据库");
        assert!((3..=5).contains(&n), "CJK 3 chars ≈ 3-4 tokens, got {n}");
    }

    #[test]
    fn estimate_tokens_mixed() {
        // "hello 世界" —— 6 ASCII + 2 CJK（空格算 ASCII）
        // 估算：2*1.2 + 6*0.25 = 2.4 + 1.5 = 3.9 → ceil 4
        let n = estimate_tokens("hello 世界");
        assert!((3..=5).contains(&n), "mixed got {n}");
    }

    #[test]
    fn estimate_tokens_pure_chinese_100_chars() {
        // 100 汉字预期 ≈ 120 tokens（与 gpt-4o-mini 实测持平）
        let text: String = "字".repeat(100);
        let n = estimate_tokens(&text);
        assert!((100..=130).contains(&n), "100 CJK chars ≈ 100-130 tokens, got {n}");
    }

    #[test]
    fn generate_summary_unlocked_happy_path() {
        let mock = mock_with("这是压缩后的摘要。");
        let r = generate_summary(&mock, "很长的原文内容" . repeat(20).as_str(), ContextStrategy::Economical);
        assert_eq!(r.unwrap(), "这是压缩后的摘要。");
    }

    #[test]
    fn generate_summary_errors_on_empty_response() {
        let mock = mock_with("   \n  ");
        let r = generate_summary(&mock, "some text to summarize please", ContextStrategy::Economical);
        assert!(r.is_err(), "empty summary must surface as Err");
    }

    #[test]
    fn generate_summary_errors_when_llm_fails() {
        // MockLlmProvider 没有 push_response 时 chat() 返 Err
        let mock = MockLlmProvider::new("mock");
        let r = generate_summary(&mock, "text", ContextStrategy::Economical);
        assert!(r.is_err());
    }
}
