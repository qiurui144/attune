//! chunk 文档分类 — 文本片段 → 内容类型.
//!
//! 纯规则版 (关键词 + 启发式), 无 LLM. 调用方按需追加 LLM 二次分类提高准确率.
//!
//! 分类:
//! - borrowing_doc: 借条 / 借款合同
//! - contract: 一般合同 (买卖 / 服务 / 租赁)
//! - bank_statement: 银行流水 / 对账单
//! - chat: 微信 / 短信 / 聊天记录
//! - receipt: 收据 / 发票 / 还款凭证
//! - judgment: 判决书 / 裁定书
//! - id_doc: 身份证 / 户口本 (脱敏后的引用)
//! - other: 不属于以上任何类型

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Classification {
    pub kind: ChunkKind,
    /// 0.0-1.0, 关键词命中数归一化得分
    pub confidence: f64,
    /// 命中的关键词列表 (供 audit)
    pub matched_keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ChunkKind {
    BorrowingDoc,
    Contract,
    BankStatement,
    Chat,
    Receipt,
    Judgment,
    IdDoc,
    Other,
}

impl ChunkKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChunkKind::BorrowingDoc => "borrowing_doc",
            ChunkKind::Contract => "contract",
            ChunkKind::BankStatement => "bank_statement",
            ChunkKind::Chat => "chat",
            ChunkKind::Receipt => "receipt",
            ChunkKind::Judgment => "judgment",
            ChunkKind::IdDoc => "id_doc",
            ChunkKind::Other => "other",
        }
    }
}

/// 关键词权重表: 命中即累计得分; 最终得分高的 kind 胜出.
const KIND_KEYWORDS: &[(ChunkKind, &[&str])] = &[
    (ChunkKind::BorrowingDoc, &[
        "借条", "借款合同", "借据", "出借人", "借款人", "本金", "月利率", "年利率",
        "利息", "约定利息", "还款期限", "履行期限",
    ]),
    (ChunkKind::Contract, &[
        "甲方", "乙方", "丙方", "合同", "协议", "标的", "违约责任", "争议解决",
        "签订日期", "履约", "承诺", "保证",
    ]),
    (ChunkKind::BankStatement, &[
        "交易日期", "交易金额", "余额", "对方账号", "对方户名", "他行来账", "网银",
        "卡号", "账号", "汇入", "汇出", "活期", "结算账户",
    ]),
    (ChunkKind::Chat, &[
        "[微信]", "微信聊天", "微信记录", "短信", "聊天记录", "对话",
        "下午好", "晚上好", "嗯嗯", "哦哦", "[图片]", "[语音]",
    ]),
    (ChunkKind::Receipt, &[
        "收据", "发票", "收条", "凭证", "今收到", "兹收到", "开票日期",
        "发票代码", "发票号码", "购买方", "销售方",
    ]),
    (ChunkKind::Judgment, &[
        "判决书", "裁定书", "本院查明", "本院认为", "判决如下", "裁定如下",
        "审判长", "审判员", "书记员", "案号",
    ]),
    (ChunkKind::IdDoc, &[
        "身份证号", "公民身份号码", "出生日期", "户口本", "户籍",
    ]),
];

/// 主入口
pub fn classify(text: &str) -> Classification {
    if text.trim().is_empty() {
        return Classification {
            kind: ChunkKind::Other,
            confidence: 0.0,
            matched_keywords: vec![],
        };
    }

    let mut scores: Vec<(ChunkKind, Vec<String>)> = Vec::new();
    for (kind, keywords) in KIND_KEYWORDS {
        let matched: Vec<String> = keywords
            .iter()
            .filter(|kw| text.contains(*kw))
            .map(|s| s.to_string())
            .collect();
        if !matched.is_empty() {
            scores.push((kind.clone(), matched));
        }
    }

    if scores.is_empty() {
        return Classification {
            kind: ChunkKind::Other,
            confidence: 0.0,
            matched_keywords: vec![],
        };
    }

    // 取命中关键词最多的 kind
    scores.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
    let (best_kind, matched) = scores.into_iter().next().unwrap();
    // confidence: 命中数 / 该 kind 关键词总数, 上限 1.0
    let total_kw = KIND_KEYWORDS
        .iter()
        .find(|(k, _)| k == &best_kind)
        .map(|(_, kws)| kws.len())
        .unwrap_or(1);
    let confidence = (matched.len() as f64 / total_kw as f64).min(1.0);

    Classification {
        kind: best_kind,
        confidence,
        matched_keywords: matched,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_borrowing_doc() {
        let text = "借条\n\n出借人: 张三\n借款人: 李四\n借款本金: 人民币伍拾万元\n月利率 1%, 还款期限 2 年.";
        let r = classify(text);
        assert_eq!(r.kind, ChunkKind::BorrowingDoc);
        assert!(r.confidence > 0.3);
        assert!(r.matched_keywords.contains(&"借条".to_string()));
        assert!(r.matched_keywords.contains(&"出借人".to_string()));
    }

    #[test]
    fn classify_bank_statement() {
        let text = "交易日期 2023-01-15  对方户名 张三  交易金额 +500000.00  余额 1234567.89  汇入 网银";
        let r = classify(text);
        assert_eq!(r.kind, ChunkKind::BankStatement);
        assert!(r.matched_keywords.contains(&"交易日期".to_string()));
        assert!(r.matched_keywords.contains(&"对方户名".to_string()));
        assert!(r.matched_keywords.contains(&"余额".to_string()));
    }

    #[test]
    fn classify_chat() {
        let text = "[微信] 张三: 借款已转 [图片]\n李四: 收到了 嗯嗯";
        let r = classify(text);
        assert_eq!(r.kind, ChunkKind::Chat);
    }

    #[test]
    fn classify_receipt() {
        let text = "收据\n今收到张三还款人民币贰万元整\n开票日期: 2023-06-15";
        let r = classify(text);
        assert_eq!(r.kind, ChunkKind::Receipt);
    }

    #[test]
    fn classify_judgment() {
        let text = "(2024)京01民初1234号民事判决书\n本院查明: ...\n本院认为: ...\n判决如下: ...";
        let r = classify(text);
        assert_eq!(r.kind, ChunkKind::Judgment);
    }

    #[test]
    fn classify_general_contract() {
        let text = "甲方: A 公司\n乙方: B 公司\n合同标的: 提供咨询服务\n违约责任: ...";
        let r = classify(text);
        assert_eq!(r.kind, ChunkKind::Contract);
    }

    #[test]
    fn classify_other_for_irrelevant_text() {
        let r = classify("今天天气不错，适合散步。");
        assert_eq!(r.kind, ChunkKind::Other);
        assert_eq!(r.confidence, 0.0);
    }

    #[test]
    fn classify_empty() {
        let r = classify("");
        assert_eq!(r.kind, ChunkKind::Other);
        assert_eq!(r.confidence, 0.0);
        assert!(r.matched_keywords.is_empty());
    }

    #[test]
    fn confidence_increases_with_more_hits() {
        let weak = classify("借条");
        let strong = classify("借条 出借人 借款人 本金 月利率 利息 还款期限");
        assert_eq!(weak.kind, ChunkKind::BorrowingDoc);
        assert_eq!(strong.kind, ChunkKind::BorrowingDoc);
        assert!(strong.confidence > weak.confidence);
    }

    #[test]
    fn ambiguous_text_picks_most_keywords() {
        // 既像借条又像合同, 但借条关键词多
        let text = "甲方乙方 借条 出借人 借款人 本金 月利率";
        let r = classify(text);
        assert_eq!(r.kind, ChunkKind::BorrowingDoc);
    }
}
