//! 零成本主动建议引擎 — 确定性规则层(信号 → 规则 → 建议卡)。
//!
//! 北极星"忙碌时的助理"的被动腿:系统基于**已有的确定性本地信号**(search_miss /
//! doc lifecycle / citation_hit / annotation_marker / organizer 聚类 /
//! project_recommender 实体重叠 / browse 画像)生成**被动可见**的建议卡,把"花钱/
//! 花时间"的决定权 100% 交还用户。
//!
//! ## 成本契约硬约束(本模块存在的核心理由)
//!
//! [`evaluate`] 的签名**编译期不含任何 LLM provider 句柄** —— 入参是纯数据
//! [`SignalContext`],出参是纯数据 `Vec<`[`SuggestionCard`]`>`。本模块**不 import**
//! `crate::llm::LlmProvider`(见反偷跑 proptest)。因此"为生成建议卡而调 LLM"在编译期
//! 就不可能发生。LLM 只可能在**用户点击建议卡的 action 之后**,由既有的显式触发 handler
//! (OrganizeWizard 确认 / Chat 发送 / web search)里发生,且那些 handler 已带成本 UI。
//!
//! per spec docs/superpowers/specs/2026-06-17-suggestions-and-thirdparty-accounts.md
//! (能力 A 部分);成本契约见 CLAUDE.md §成本感知与触发契约。

use serde::{Deserialize, Serialize};

/// 建议卡类别。对应 4 类确定性机会。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionKind {
    /// 整理类:一批文件实体/向量高度聚集,可整理成 Project/案卷。
    Organize,
    /// 补充类:某查询反复落空,提示补资料(打开搜索/web 入口)。
    Enrich,
    /// 检索优化类:积累了足够 search_miss,可触发已有技能进化(扩展词)。
    Retrieval,
    /// 画像类:浏览/批注行为积累到阈值,可查看行为画像。
    Profile,
    /// 连接源类:尚未连接任何第三方账号(WebDAV / IMAP / RSS / Git / …),
    /// 提示打开账号管理面板连接一个,扩大可被检索的知识来源。
    ConnectSource,
}

impl SuggestionKind {
    /// 稳定的 kebab 字符串(DB 持久化 + mute key + WS 推送用)。
    pub fn as_str(&self) -> &'static str {
        match self {
            SuggestionKind::Organize => "organize",
            SuggestionKind::Enrich => "enrich",
            SuggestionKind::Retrieval => "retrieval",
            SuggestionKind::Profile => "profile",
            SuggestionKind::ConnectSource => "connect_source",
        }
    }

    /// 解析 kebab 字符串(REST mute/unmute 入参用)。未知 → None。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "organize" => Some(SuggestionKind::Organize),
            "enrich" => Some(SuggestionKind::Enrich),
            "retrieval" => Some(SuggestionKind::Retrieval),
            "profile" => Some(SuggestionKind::Profile),
            "connect_source" => Some(SuggestionKind::ConnectSource),
            _ => None,
        }
    }

    /// 所有 kind(UI 枚举 / 测试遍历)。
    pub const ALL: [SuggestionKind; 5] = [
        SuggestionKind::Organize,
        SuggestionKind::Enrich,
        SuggestionKind::Retrieval,
        SuggestionKind::Profile,
        SuggestionKind::ConnectSource,
    ];
}

/// 点击建议卡后路由到的**既有**显式动作。建议卡本身永不执行 LLM;
/// 这些 action 全部指向已存在的、带成本 UI 的显式 handler。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    OpenOrganizeWizard,
    OpenSearch,
    RunSkillEvolution,
    OpenProfile,
    /// 打开第三方账号管理面板(连接 WebDAV / IMAP / RSS / Git / … 凭据)。
    /// 连接动作本身零成本(只是把凭据加密落库);采集/同步走既有显式路径。
    OpenAccountsPanel,
}

impl ActionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ActionKind::OpenOrganizeWizard => "open_organize_wizard",
            ActionKind::OpenSearch => "open_search",
            ActionKind::RunSkillEvolution => "run_skill_evolution",
            ActionKind::OpenProfile => "open_profile",
            ActionKind::OpenAccountsPanel => "open_accounts_panel",
        }
    }
}

/// 成本层级提示。卡本身生成永远零成本(`tier = Free`);`note` 描述**点击后**动作的
/// 成本层级,让用户知情(per 成本契约 §UI 显示成本)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostTier {
    /// 零成本(CPU 毫秒)。
    Free,
    /// 本地算力(秒级,embedding/classify/聚类)。
    Local,
    /// 时间/金钱(LLM,本地或云端)。
    Cloud,
}

/// 建议卡(typed)。**不含任何 LLM 产出** —— 仅轻量确定性元数据。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuggestionCard {
    /// 去重指纹(稳定哈希 of kind + 排序后的 ref_ids/语义键)。同一机会多次
    /// evaluate 产生同一 signature,用于幂等去重 + dismiss 记忆。
    pub signature: String,
    pub kind: SuggestionKind,
    pub title: String,
    pub detail: String,
    /// 引用的实体 id(item_id / 查询关键词 等),供 UI 跳转 + action 上下文。
    pub ref_ids: Vec<String>,
    pub action_kind: ActionKind,
    /// 点击后动作的成本层级 + 说明。卡生成本身永远是 Free。
    pub cost_tier: CostTier,
    pub cost_note: String,
}

impl SuggestionCard {
    /// 由 (kind, 已排序语义键) 计算稳定 signature。同输入 → 同输出(去重幂等)。
    fn make_signature(kind: SuggestionKind, sorted_keys: &[String]) -> String {
        // djb2(与扩展捕获去重同族)— 无加密需求,只要稳定 + 低碰撞。
        let mut hash: u64 = 5381;
        for byte in kind.as_str().bytes() {
            hash = hash.wrapping_mul(33).wrapping_add(byte as u64);
        }
        hash = hash.wrapping_mul(33).wrapping_add(b'|' as u64);
        for key in sorted_keys {
            for byte in key.bytes() {
                hash = hash.wrapping_mul(33).wrapping_add(byte as u64);
            }
            hash = hash.wrapping_mul(33).wrapping_add(b',' as u64);
        }
        format!("{}-{:016x}", kind.as_str(), hash)
    }
}

/// 一个聚类候选(来自 organizer::analyze_items 的确定性聚类结果投影)。
/// 仅持已算好的元数据,**不含向量/LLM 命名** —— 命名只在用户点确认时才发生。
#[derive(Debug, Clone, PartialEq)]
pub struct ClusterCandidate {
    /// 该组的 item_id 集。
    pub item_ids: Vec<String>,
    /// 该组主导实体/关键词(extractive,确定性;用于卡标题,无 LLM)。
    pub top_entities: Vec<String>,
}

/// 一个反复落空的查询(来自 search_miss 信号聚合)。
#[derive(Debug, Clone, PartialEq)]
pub struct MissedQuery {
    pub query: String,
    /// 该查询累计 miss 次数。
    pub miss_count: u32,
}

/// 规则引擎的**纯数据**输入。caller(route 层)负责从 Store 读已有信号填充。
///
/// 关键:此结构**不含 LLM provider 句柄**,所以 [`evaluate`] 编译期无法偷跑 LLM。
#[derive(Debug, Clone, Default)]
pub struct SignalContext {
    /// 反复落空的查询(已聚合)。
    pub missed_queries: Vec<MissedQuery>,
    /// 待处理 search_miss 总数(检索优化阈值判断)。
    pub unprocessed_search_miss: u32,
    /// organizer 聚类候选组(确定性聚类已算好)。
    pub cluster_candidates: Vec<ClusterCandidate>,
    /// 浏览行为信号累计条数(画像阈值判断)。
    pub browse_signal_count: u32,
    /// 批注 marker 累计条数(画像阈值判断)。
    pub annotation_marker_count: u32,
    /// 用户已 mute 的 kind 集(本层过滤,避免生成被静音的卡)。
    pub muted_kinds: Vec<SuggestionKind>,
    /// 用户已 dismiss 的卡 signature 集(本层过滤,避免重复打扰)。
    pub dismissed_signatures: Vec<String>,
    /// 已连接的第三方账号源数量(WebDAV / IMAP / RSS / Git / …)。
    /// `None` = 调用方未评估源清单(默认,不出连接卡);`Some(n)` = 已评估,n 个已连接,
    /// `Some(0)`(≤ 阈值)→ 出"连接第三方账号"卡。纯计数,无凭据/secret。
    /// 用 Option 区分"未知"与"确为 0",使既有 4 类卡的 caller(默认 None)行为不变。
    pub connected_source_count: Option<u32>,
}

// === 阈值常量(确定性,复用已验证语义) ===

/// 单个查询达到此 miss 次数 → 出"补充资料"卡。
pub const ENRICH_MISS_THRESHOLD: u32 = 3;
/// 待处理 search_miss 总数达此值 → 出"检索优化(触发技能进化)"卡。
pub const RETRIEVAL_SIGNAL_THRESHOLD: u32 = 5;
/// 一个聚类候选组至少这么多 item 才值得提示整理(低于则噪声)。
pub const ORGANIZE_MIN_GROUP: usize = 3;
/// 浏览 + 批注信号合计达此值 → 出"查看画像"卡。
pub const PROFILE_SIGNAL_THRESHOLD: u32 = 10;
/// 已连接源数量 ≤ 此值 → 出"连接第三方账号"卡(0 = 一个都没连)。
pub const CONNECT_SOURCE_THRESHOLD: u32 = 0;
/// 单卡 ref_ids 上限(防超量膨胀 UI / DB)。
pub const MAX_REF_IDS: usize = 50;

/// **建议引擎核心**:确定性信号 → 规则 → 建议卡。
///
/// 纯函数,零 IO,零 LLM。同一 [`SignalContext`] 多次调用产出稳定(signature 去重幂等)。
/// mute / dismiss 在本层过滤。返回的卡已按 kind 稳定排序(组内按 signature)。
pub fn evaluate(ctx: &SignalContext) -> Vec<SuggestionCard> {
    let mut cards: Vec<SuggestionCard> = Vec::new();

    // 单规则内部失败被隔离:每条规则用 catch_unwind 包裹,一条 panic 不影响其它卡
    // (per spec §7 异常 case:单规则 panic 隔离)。规则本身是纯函数,正常不会 panic;
    // 这是 defense-in-depth(future 自定义规则可能 panic)。
    run_rule_isolated(&mut cards, ctx, SuggestionKind::Organize, rule_organize);
    run_rule_isolated(&mut cards, ctx, SuggestionKind::Enrich, rule_enrich);
    run_rule_isolated(&mut cards, ctx, SuggestionKind::Retrieval, rule_retrieval);
    run_rule_isolated(&mut cards, ctx, SuggestionKind::Profile, rule_profile);
    run_rule_isolated(
        &mut cards,
        ctx,
        SuggestionKind::ConnectSource,
        rule_connect_source,
    );

    // dismiss 过滤(用户已忽略的 signature 不再出现)。
    cards.retain(|c| !ctx.dismissed_signatures.contains(&c.signature));

    // signature 去重(防同一机会被多规则/重复输入产出两次)。
    cards.sort_by(|a, b| {
        a.kind
            .as_str()
            .cmp(b.kind.as_str())
            .then_with(|| a.signature.cmp(&b.signature))
    });
    cards.dedup_by(|a, b| a.signature == b.signature);
    cards
}

/// 跑单条规则:mute 过滤 + panic 隔离。一条规则 panic 不阻塞其它规则出卡。
fn run_rule_isolated(
    out: &mut Vec<SuggestionCard>,
    ctx: &SignalContext,
    kind: SuggestionKind,
    rule: fn(&SignalContext) -> Vec<SuggestionCard>,
) {
    if ctx.muted_kinds.contains(&kind) {
        return;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rule(ctx)));
    match result {
        Ok(cards) => out.extend(cards),
        Err(_) => {
            // 静默忽略该规则(per signals 静默约定 + spec §7)。
            log::warn!("suggestion rule {} panicked, skipped", kind.as_str());
        }
    }
}

/// 整理类:每个 ≥ ORGANIZE_MIN_GROUP 的聚类候选组 → 一张整理卡。
fn rule_organize(ctx: &SignalContext) -> Vec<SuggestionCard> {
    ctx.cluster_candidates
        .iter()
        .filter(|g| g.item_ids.len() >= ORGANIZE_MIN_GROUP)
        .map(|g| {
            let group_size = g.item_ids.len();
            let mut ref_ids = g.item_ids.clone();
            ref_ids.sort();
            ref_ids.truncate(MAX_REF_IDS);
            // signature 用排序后的 item_ids(语义键),保证组内容稳定 → signature 稳定。
            let signature = SuggestionCard::make_signature(SuggestionKind::Organize, &ref_ids);
            let detail = if g.top_entities.is_empty() {
                String::new()
            } else {
                format!("涉及实体:{}", g.top_entities.join(" / "))
            };
            SuggestionCard {
                signature,
                kind: SuggestionKind::Organize,
                title: format!("{group_size} 份文件高度相关,可整理成一个 Project"),
                detail,
                ref_ids,
                action_kind: ActionKind::OpenOrganizeWizard,
                cost_tier: CostTier::Local,
                cost_note: "整理预览免费;确认归类时按你的设置可能调用本地/云端模型".into(),
            }
        })
        .collect()
}

/// 补充类:每个 miss_count ≥ ENRICH_MISS_THRESHOLD 的查询 → 一张补充卡。
fn rule_enrich(ctx: &SignalContext) -> Vec<SuggestionCard> {
    ctx.missed_queries
        .iter()
        .filter(|q| q.miss_count >= ENRICH_MISS_THRESHOLD && !q.query.trim().is_empty())
        .map(|q| {
            let key = vec![q.query.clone()];
            let signature = SuggestionCard::make_signature(SuggestionKind::Enrich, &key);
            SuggestionCard {
                signature,
                kind: SuggestionKind::Enrich,
                title: format!("查询「{}」反复落空,要不要补充资料?", q.query),
                detail: format!("过去累计 {} 次未命中本地知识库", q.miss_count),
                ref_ids: vec![q.query.clone()],
                action_kind: ActionKind::OpenSearch,
                cost_tier: CostTier::Free,
                cost_note: "打开搜索/网络入口免费;是否调用云端搜索由你决定".into(),
            }
        })
        .collect()
}

/// 检索优化类:待处理 search_miss 总数达阈值 → 一张技能进化卡(单卡,聚合)。
fn rule_retrieval(ctx: &SignalContext) -> Vec<SuggestionCard> {
    if ctx.unprocessed_search_miss < RETRIEVAL_SIGNAL_THRESHOLD {
        return Vec::new();
    }
    // 单卡:语义键用固定 token,signature 在"有待处理信号"期间稳定。
    let key = vec!["search_miss_batch".to_string()];
    let signature = SuggestionCard::make_signature(SuggestionKind::Retrieval, &key);
    vec![SuggestionCard {
        signature,
        kind: SuggestionKind::Retrieval,
        title: "积累了一批搜索失败,可优化检索词典".into(),
        detail: format!(
            "{} 条待处理失败信号,技能进化可学习同义/扩展词提升命中",
            ctx.unprocessed_search_miss
        ),
        ref_ids: vec![],
        action_kind: ActionKind::RunSkillEvolution,
        // 技能进化本身会调 LLM(扩展词学习)— 点击后走既有显式路径,带成本 UI。
        cost_tier: CostTier::Cloud,
        cost_note: "点击后技能进化会按你的设置调用本地/云端模型学习扩展词".into(),
    }]
}

/// 画像类:浏览 + 批注信号合计达阈值 → 一张画像卡(单卡)。
fn rule_profile(ctx: &SignalContext) -> Vec<SuggestionCard> {
    let total = ctx
        .browse_signal_count
        .saturating_add(ctx.annotation_marker_count);
    if total < PROFILE_SIGNAL_THRESHOLD {
        return Vec::new();
    }
    let key = vec!["profile".to_string()];
    let signature = SuggestionCard::make_signature(SuggestionKind::Profile, &key);
    vec![SuggestionCard {
        signature,
        kind: SuggestionKind::Profile,
        title: "你的行为画像已积累足够数据,来看看?".into(),
        detail: format!(
            "{} 条浏览信号 + {} 条批注,可查看主题偏好画像",
            ctx.browse_signal_count, ctx.annotation_marker_count
        ),
        ref_ids: vec![],
        action_kind: ActionKind::OpenProfile,
        cost_tier: CostTier::Free,
        cost_note: "画像基于本地确定性统计,免费".into(),
    }]
}

/// 连接源类:用户已连接的第三方账号源 ≤ 阈值(默认 0 = 一个都没连)→ 一张
/// "连接第三方账号"卡(单卡)。仅在用户**已在使用 attune**(有其它确定性活动信号)
/// 时才出 —— 全新空 vault 不打扰(避免开箱即弹)。
///
/// 卡生成零成本:连接动作本身只是把凭据加密落库(Free);采集/同步是点击后既有
/// 显式路径。signature 用固定 token,在"未连接源"期间稳定(去重幂等)。
fn rule_connect_source(ctx: &SignalContext) -> Vec<SuggestionCard> {
    // None = caller 未评估源清单 → 不出卡(既有 4 类卡 caller 默认行为不变)。
    let connected = match ctx.connected_source_count {
        Some(n) => n,
        None => return Vec::new(),
    };
    if connected > CONNECT_SOURCE_THRESHOLD {
        return Vec::new();
    }
    // 全新空 vault(零活动)不弹:要求用户至少有一类其它确定性活动信号。
    let user_is_active = !ctx.missed_queries.is_empty()
        || !ctx.cluster_candidates.is_empty()
        || ctx.unprocessed_search_miss > 0
        || ctx.browse_signal_count > 0
        || ctx.annotation_marker_count > 0;
    if !user_is_active {
        return Vec::new();
    }
    let key = vec!["connect_source".to_string()];
    let signature = SuggestionCard::make_signature(SuggestionKind::ConnectSource, &key);
    vec![SuggestionCard {
        signature,
        kind: SuggestionKind::ConnectSource,
        title: "还没连接任何外部知识源,要不要连一个?".into(),
        detail: "连接网盘 / 邮箱 / RSS / Git 等账号,让更多资料能被检索(凭据本地加密保存)".into(),
        ref_ids: vec![],
        action_kind: ActionKind::OpenAccountsPanel,
        // 连接本身零成本(只是加密保存凭据);采集/同步走既有显式路径,届时另有成本提示。
        cost_tier: CostTier::Free,
        cost_note: "连接账号免费(凭据本地 AES-256 加密保存);采集/同步由你显式触发".into(),
    }]
}

#[cfg(test)]
mod tests;
