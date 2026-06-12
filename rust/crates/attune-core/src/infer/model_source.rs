//! S8 动态模型源选择(客户端) — spec docs/superpowers/specs/2026-06-11-modelstack-lifecycle.md §12
//!
//! 把 `platform::region` 的**静态单源**(CN→ModelScope / 海外→HF)升级为
//! **候选源注册表 + 健康/吞吐探测 + 自动选最优 + 失败 failover**。
//!
//! 根因(§12 实测,§6.3 数据有源):CN 冷启动无本地缓存,HF/hf-mirror 在 CN 不可靠
//! → 模型获取是 CN 市场 P0 发布阻断。单源(`region.hf_endpoint()`)做不到"今天 ModelScope
//! 快不等于永远",需运行时探测 + failover。
//!
//! 铁律(§12 反模式 + R3):
//! - **探测只在 pre-flight 或显式触发**,绝不在请求路径同步阻塞(R3)。
//! - region 判定用 locale/timezone(复用 `region.rs`),**不**用 IP geo(隐私出网)。
//! - 下载失败/sha 不符 → 自动切次优 + 重探,不直接报死。
//! - 用户显式 `HF_ENDPOINT` env 仍最高优先(覆盖 selector,§5 向后兼容)。

use crate::platform::region::Region;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// 单个候选下载源。endpoint 是 HF-resolve 兼容根(hf-hub 拼
/// `{endpoint}/{repo}/resolve/{rev}/{file}`)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSource {
    /// 稳定标识(`company-mirror` / `modelscope` / `hf-mirror` / `hf-official`)。
    pub id: String,
    /// HF-resolve 兼容 endpoint 根(无尾 `/`)。
    pub endpoint: String,
    /// 内置优先级(越大越优先)。company > ModelScope > hf-mirror > HF。
    /// 同一组 healthy 源里最终按 `throughput × priority` 排序,priority 是 tie-break + 偏置。
    pub priority: u32,
    /// 该源偏好的区域(CN 区把 company/ModelScope 提前)。`None` = 任意区域中性。
    pub region_hint: Option<Region>,
    /// 仓覆盖范围:`Full` = 全镜像;`OnlyXenovaOnnx` = 仅 Xenova ONNX(embedding/reranker),
    /// whisper/PP-OCR 等在该源 404 → selector 对这些 repo 跳过此源(避免无谓 404 重试)。
    pub coverage: SourceCoverage,
}

/// 源的仓覆盖范围。ModelScope 非全镜像(§12 / region.rs doc):有 Xenova ONNX,无
/// whisper.cpp / RapidOCR。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceCoverage {
    /// 全集(company-mirror / HF 官方)。
    Full,
    /// 仅 Xenova ONNX(ModelScope 实测覆盖面)。
    OnlyXenovaOnnx,
}

impl SourceCoverage {
    /// 该源是否覆盖给定 HF repo_id。`OnlyXenovaOnnx` 仅放行 `Xenova/*`。
    pub fn covers(self, repo_id: &str) -> bool {
        match self {
            SourceCoverage::Full => true,
            SourceCoverage::OnlyXenovaOnnx => repo_id.starts_with("Xenova/"),
        }
    }
}

/// company-mirror endpoint(cloud R2.E 配套的自建 HF-layout 镜像)。
/// host 决策归 cloud spec(R2.E 新 W);本客户端只 pin 契约 URL,可经
/// `ATTUNE_COMPANY_MIRROR` env 覆盖(私有部署/测试)。
fn company_mirror_endpoint() -> String {
    std::env::var("ATTUNE_COMPANY_MIRROR")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "https://models.engi-stack.com".to_string())
}

/// 内置候选源注册表(优先级降序)。§12:company-mirror(最高)> ModelScope(CN)
/// > hf-mirror > HF 官方(海外)。
///
/// 注:hf-mirror 实测在 CN 已死(§12 / region.rs 注),但保留作低优先候选 —
/// 探测会把它判为 unreachable 自动剔除,无需从注册表删(避免源恢复时漏掉)。
pub fn builtin_sources() -> Vec<ModelSource> {
    vec![
        ModelSource {
            id: "company-mirror".to_string(),
            endpoint: company_mirror_endpoint(),
            priority: 100,
            region_hint: None, // 全集 + 最稳兜底,任意区域可用
            coverage: SourceCoverage::Full,
        },
        ModelSource {
            id: "modelscope".to_string(),
            endpoint: "https://modelscope.cn/models".to_string(),
            priority: 80,
            region_hint: Some(Region::China),
            coverage: SourceCoverage::OnlyXenovaOnnx,
        },
        ModelSource {
            id: "hf-mirror".to_string(),
            endpoint: "https://hf-mirror.com".to_string(),
            priority: 40,
            region_hint: Some(Region::China),
            coverage: SourceCoverage::Full,
        },
        ModelSource {
            id: "hf-official".to_string(),
            endpoint: "https://huggingface.co".to_string(),
            priority: 60,
            region_hint: Some(Region::International),
            coverage: SourceCoverage::Full,
        },
    ]
}

/// 探测一个源的连接超时 + 1MB range-GET 整体超时上限。探测是 pre-flight 的轻量动作,
/// 死源在此快速 fail,不拖累选源。
const PROBE_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_TOTAL_TIMEOUT: Duration = Duration::from_secs(15);
/// range-GET 探测字节数(1MB,§12)。足够算出有意义的 throughput,又不浪费带宽。
const PROBE_BYTES: u64 = 1024 * 1024;

/// 单源健康/吞吐探测结果。
#[derive(Debug, Clone, PartialEq)]
pub struct SourceHealth {
    /// 源 id(对应 `ModelSource::id`)。
    pub source_id: String,
    /// 是否可达(probe 成功拿到 ≥1 字节)。
    pub reachable: bool,
    /// 实测吞吐(字节/秒);unreachable 时 0。
    pub throughput_bps: f64,
    /// 首字节延迟(毫秒);unreachable 时 None。
    pub latency_ms: Option<u64>,
}

impl SourceHealth {
    /// 不可达占位(probe 失败统一产出)。
    fn unreachable(source_id: &str) -> Self {
        Self {
            source_id: source_id.to_string(),
            reachable: false,
            throughput_bps: 0.0,
            latency_ms: None,
        }
    }
}

/// 对单个候选源做 range-GET 1MB 探测(§12 step 2)。命中 `Range: bytes=0-{PROBE_BYTES-1}`
/// 拉一小段 → 算 reachable + throughput + latency。
///
/// 关键不变量(R3):**只在 pre-flight / 显式触发调用**,绝不进请求路径。探针对一个**已知存在**
/// 的小文件发 range-GET;若源连不上 / 超时 / 非 2xx → `SourceHealth::unreachable`(不 panic,
/// 不阻塞 — 死源在 connect/total 超时内有界失败)。
///
/// `probe_repo` / `probe_file`:探测用的标准小文件(各源都该有);默认用 embedding tokenizer
/// (各 HF 兼容源通常都有 Xenova/bge-m3)。该 repo 必须落在源的 `coverage` 内,否则探测必 404
/// —— 调用方(`probe_source`)对 `OnlyXenovaOnnx` 源已用 Xenova repo,匹配。
fn probe_source_with(
    source: &ModelSource,
    probe_repo: &str,
    probe_file: &str,
) -> SourceHealth {
    let url = format!("{}/{}/resolve/main/{}", source.endpoint, probe_repo, probe_file);
    let client = match reqwest::blocking::Client::builder()
        .connect_timeout(PROBE_CONNECT_TIMEOUT)
        .timeout(PROBE_TOTAL_TIMEOUT)
        .build()
    {
        Ok(c) => c,
        Err(_) => return SourceHealth::unreachable(&source.id),
    };
    let start = std::time::Instant::now();
    let resp = client
        .get(&url)
        .header("Range", format!("bytes=0-{}", PROBE_BYTES - 1))
        .send();
    let resp = match resp {
        Ok(r) if r.status().is_success() => r,
        _ => return SourceHealth::unreachable(&source.id),
    };
    let latency_ms = start.elapsed().as_millis() as u64;
    // 读 body(range-GET 已限 1MB;若源忽略 Range 返回全文,bytes() 仍受 total timeout 兜底)。
    let bytes = match resp.bytes() {
        Ok(b) if !b.is_empty() => b,
        _ => return SourceHealth::unreachable(&source.id),
    };
    let elapsed = start.elapsed().as_secs_f64().max(1e-3);
    let throughput_bps = bytes.len() as f64 / elapsed;
    SourceHealth {
        source_id: source.id.clone(),
        reachable: true,
        throughput_bps,
        latency_ms: Some(latency_ms),
    }
}

/// 探测一个源:对 `OnlyXenovaOnnx` 源用 Xenova 标准 repo,`Full` 源同样可用该 repo
/// (各源都该有 embedding tokenizer)。统一探测文件 = `tokenizer.json`(小,各源齐全)。
pub fn probe_source(source: &ModelSource) -> SourceHealth {
    probe_source_with(source, "Xenova/bge-m3", "tokenizer.json")
}

/// region_hint 命中当前检测区域时的优先级偏置乘数(§12:CN 区把 company/ModelScope 提前)。
/// 对 healthy 源的排序键 `throughput × priority × region_bias` 生效 —— 同区源被显著抬升,
/// 但**实测 throughput 仍能翻盘**(一个区内极慢的源不会因 region 命中就压过区外快源)。
const REGION_MATCH_BIAS: f64 = 1.5;

/// 给定一组**已探测**的源 + 它们的 health,为 `repo_id` 选出 failover 顺序(最优在前)。
///
/// 规则(§12 step 2+3):
/// 1. 先按 `coverage.covers(repo_id)` 过滤 —— 该源不覆盖此 repo 直接剔除(whisper/PP-OCR
///    跳过 ModelScope,避免无谓 404)。
/// 2. 再按 `reachable` 过滤 —— 探测不可达的源剔除。
/// 3. 剩余按排序键 `throughput_bps × priority × region_bias` 降序;`region_bias` 在源
///    `region_hint == 当前 region` 时取 `REGION_MATCH_BIAS`,否则 1.0。
///
/// 返回的是**排序后的源列表**(供 failover 顺序消费):空 = 无可用源(调用方报 model-missing /
/// unreachable)。`region` 由 `detect_region()`(locale/timezone,**非 IP geo**)传入,便于测试注入。
pub fn select_failover_order(
    probed: &[(ModelSource, SourceHealth)],
    repo_id: &str,
    region: Region,
) -> Vec<ModelSource> {
    let mut eligible: Vec<&(ModelSource, SourceHealth)> = probed
        .iter()
        .filter(|(s, h)| s.coverage.covers(repo_id) && h.reachable)
        .collect();
    eligible.sort_by(|a, b| {
        let score = |s: &ModelSource, h: &SourceHealth| -> f64 {
            let region_bias = if s.region_hint == Some(region) {
                REGION_MATCH_BIAS
            } else {
                1.0
            };
            h.throughput_bps * (s.priority as f64) * region_bias
        };
        let sa = score(&a.0, &a.1);
        let sb = score(&b.0, &b.1);
        // 降序;NaN 兜底成 Equal(理论上 throughput 非 NaN)。
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });
    eligible.into_iter().map(|(s, _)| s.clone()).collect()
}

/// 进程内选源缓存:把 `resolve_sources_for` 的探测结果(已排序 failover 顺序)按
/// (region, eligible-class) 缓存一段 TTL,**避免每次模型下载都重探所有源**。
///
/// 根因(§5.2.0b cache 死代码 finding):S8 落了 `SelectedSource` 读写 + TTL 但**零调用** ——
/// 每次下载都 `resolve_sources_for` 重探全注册表,首源黑洞时串行卡到 connect 超时(~5s/源 ×
/// 4 源 ≈ 加到首搜延迟)。这里把缓存真正接进解析路径:fresh 命中直接复用顺序,跳过重探。
///
/// 缓存 key = (region, eligible_class):同一 region 下,所有 `Full`-only repo(whisper/OCR/
/// layout)共享同一组 eligible 源 + 同一相对健康序;所有 `Xenova/*` repo 共享另一组。两类
/// 即可覆盖全部下载点(§model_source coverage 只有 Full / OnlyXenovaOnnx 两种)。
struct CachedResolution {
    order: Vec<ModelSource>,
    cached_at_unix: u64,
}

/// 选源缓存 key 的 eligible-class 维度:repo 是否落在 `OnlyXenovaOnnx` 源的覆盖面内
/// (`Xenova/*` → true)。决定 eligible 源集合,从而决定可复用的缓存桶。
fn eligible_class_is_xenova(repo_id: &str) -> bool {
    SourceCoverage::OnlyXenovaOnnx.covers(repo_id)
}

/// (region_is_china, is_xenova_class) → 缓存槽。进程生命期内 2×2 桶。
/// 用 `OnceLock<Mutex<..>>`(1.70+,对齐仓内 MSRV 1.75 + 既有 idiom),非 `LazyLock`(1.80)。
type ResolutionMap = std::collections::HashMap<(bool, bool), CachedResolution>;
static RESOLUTION_CACHE: std::sync::OnceLock<std::sync::Mutex<ResolutionMap>> =
    std::sync::OnceLock::new();

/// 取(惰性初始化)缓存的 Mutex。
fn resolution_cache() -> &'static std::sync::Mutex<ResolutionMap> {
    RESOLUTION_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 便捷:对内置注册表逐源探测(覆盖 `repo_id` 的源才探,省掉必 404 的探测)→ 选 failover 顺序。
/// **只在 pre-flight / 显式触发调用**(R3)。`detect_region()` 决定 region 偏置。
///
/// 进程内缓存:TTL(`SELECTED_SOURCE_TTL_SECS`)内对同 (region, class) 的请求直接复用上次
/// 排序结果,跳过重探(§cache finding 修复)。过期/未命中才真探测并回填。
pub fn resolve_sources_for(repo_id: &str) -> Vec<ModelSource> {
    let region = crate::platform::region::detect_region();
    resolve_sources_with(repo_id, region, &probe_source)
}

/// `resolve_sources_for` 的可注入核心(测试用固定 `probe_fn` 计探测次数 + 注入 region)。
/// 缓存命中(fresh)→ 返回缓存顺序,`probe_fn` **零调用**;未命中/过期 → 真探测 + 回填。
/// `probe_fn` 取引用 —— 让测试可跨多次 resolve 复用同一(捕获引用、非 Copy)计数闭包。
fn resolve_sources_with(
    repo_id: &str,
    region: Region,
    probe_fn: &impl Fn(&ModelSource) -> SourceHealth,
) -> Vec<ModelSource> {
    let key = (region == Region::China, eligible_class_is_xenova(repo_id));
    let now = now_unix();
    {
        let cache = resolution_cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(c) = cache.get(&key) {
            // fresh(now - cached_at < TTL;时钟回拨也视为新鲜)→ 复用,跳过重探。
            if now < c.cached_at_unix || now - c.cached_at_unix < SELECTED_SOURCE_TTL_SECS {
                return c.order.clone();
            }
        }
    }
    // 未命中/过期:真探测覆盖 `repo_id` 的源 → 选序 → 回填缓存。
    let probed: Vec<(ModelSource, SourceHealth)> = builtin_sources()
        .into_iter()
        .filter(|s| s.coverage.covers(repo_id))
        .map(|s| {
            let h = probe_fn(&s);
            (s, h)
        })
        .collect();
    let order = select_failover_order(&probed, repo_id, region);
    {
        let mut cache = resolution_cache().lock().unwrap_or_else(|e| e.into_inner());
        cache.insert(key, CachedResolution { order: order.clone(), cached_at_unix: now });
    }
    order
}

/// 清空进程内选源缓存(测试隔离 / 显式"强制重探"用,如用户在 Settings 手动切区域)。
pub fn clear_resolution_cache() {
    resolution_cache().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// 当前(检测区域,Full-class)缓存桶的首选源 id —— 供下载后持久化(`persist_used_source`)。
/// `None` = 缓存空(尚未 resolve 过 / 已 clear)。Full-class 桶覆盖 whisper/OCR/layout 全集。
pub fn current_top_source_id() -> Option<String> {
    let region_cn = crate::platform::region::detect_region() == Region::China;
    let cache = resolution_cache().lock().unwrap_or_else(|e| e.into_inner());
    cache
        .get(&(region_cn, false))
        .and_then(|c| c.order.first())
        .map(|s| s.id.clone())
}

/// S8 step 3 集成:按给定 failover 顺序逐源尝试下载 `repo_id/filename` → `dst`,
/// 任一源失败(网络错 / 非 2xx / sha 不符)→ 自动切**次优源**重试,全失败才 `Err`。
///
/// `sources` 是 selector 输出(已按 throughput×priority×region 排序);通常由
/// `resolve_sources_for(repo_id)` 提供,测试可注入固定列表绕过网络探测。
///
/// **向后兼容**:用户显式 `HF_ENDPOINT` env 仍最高优先 —— 设了就只用它(单源,
/// 不走 failover),尊重运维显式注入(§5 / spec §12.5)。
///
/// 不变量(R3):本函数是**下载**动作(显式触发,后台队列),非请求路径;探测/选源
/// 已在上游 `resolve_sources_for` 完成,这里只消费已排序列表 + 逐源下载。
pub fn download_with_failover(
    sources: &[ModelSource],
    repo_id: &str,
    filename: &str,
    dst: &std::path::Path,
) -> crate::error::Result<String> {
    use crate::error::VaultError;

    // 向后兼容逃生门:显式 HF_ENDPOINT → 单源直下,不 failover(运维/测试显式注入优先)。
    if let Some(explicit) = explicit_hf_endpoint_override() {
        crate::infer::model_store::download_hf_file_from(&explicit, repo_id, filename, dst)?;
        return Ok(format!("env:{explicit}"));
    }

    if sources.is_empty() {
        return Err(VaultError::ModelLoad(format!(
            "no eligible model source for {repo_id}/{filename} (all unreachable or uncovered); engine degraded"
        )));
    }

    let mut last_err = None;
    for source in sources {
        match crate::infer::model_store::download_hf_file_from(
            &source.endpoint,
            repo_id,
            filename,
            dst,
        ) {
            Ok(()) => return Ok(source.id.clone()),
            Err(e) => {
                log::warn!(
                    "model source {} failed for {repo_id}/{filename}: {e}; failing over to next",
                    source.id
                );
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| {
        VaultError::ModelLoad(format!("all sources failed for {repo_id}/{filename}"))
    }))
}

/// 显式 `HF_ENDPOINT` env 覆盖(向后兼容):非空则返回 trimmed 值。复用 model_store
/// 的解析口径但区分"用户显式设置"与"region 默认" —— 这里只看 env 是否被显式设过。
fn explicit_hf_endpoint_override() -> Option<String> {
    std::env::var("HF_ENDPOINT")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
}

/// 选定源的缓存(§12.3:选定缓存进 settings `model_source_selected` + 探测时间戳)。
/// 落在 `app_settings.model_source` 节(纯 JSON,随 server settings 持久化);避免每次下载
/// 都重探所有源。新鲜度由 TTL 控制(§3.4:非 READY 态 TTL 10min;这里对"已选源"统一用
/// 一个保守 TTL,过期则下次显式下载重探)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectedSource {
    /// 选定源 id(对应 `ModelSource::id`)。
    pub source_id: String,
    /// 选定源 endpoint(下载时直接用,省去重新 resolve)。
    pub endpoint: String,
    /// 探测/选定时刻(unix epoch 秒)。新鲜度判定用。
    pub probed_at_unix: u64,
}

/// `app_settings` 中模型源缓存节的 key。
const MODEL_SOURCE_SETTINGS_KEY: &str = "model_source";
/// 选源缓存新鲜度 TTL(秒)。§3.4:远端可能恢复(K3 回网 / 镜像上线),不宜永久 pin
/// 一个选定源 —— 过期后下次显式下载重探,自愈到当前最优。1h 平衡"少重探"与"跟上变化"。
const SELECTED_SOURCE_TTL_SECS: u64 = 3600;

/// 当前 unix epoch 秒(单调性不重要,只用于 TTL 比较)。
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 从 `app_settings` JSON 读出缓存的选定源(若存在且 schema 合法)。纯函数,无 IO。
pub fn read_selected_source(settings: &serde_json::Value) -> Option<SelectedSource> {
    let node = settings.get(MODEL_SOURCE_SETTINGS_KEY)?;
    serde_json::from_value(node.clone()).ok()
}

/// 把选定源写进 `app_settings` JSON 的 `model_source` 节(覆盖旧值)。纯函数返回新 JSON,
/// 不做 IO(调用方负责持久化,对齐 `llm_settings::merge_gateway_into_settings` 模式)。
pub fn write_selected_source(
    mut settings: serde_json::Value,
    selected: &SelectedSource,
) -> serde_json::Value {
    if !settings.is_object() {
        settings = serde_json::json!({});
    }
    if let Some(obj) = settings.as_object_mut() {
        obj.insert(
            MODEL_SOURCE_SETTINGS_KEY.to_string(),
            serde_json::to_value(selected).unwrap_or(serde_json::Value::Null),
        );
    }
    settings
}

/// 缓存的选定源是否仍新鲜(`now - probed_at < TTL`)。过期 → 调用方应重探。
/// 也防御未来时钟(`probed_at > now` 视为新鲜,避免时钟回拨误判过期)。
pub fn selected_source_is_fresh(selected: &SelectedSource, now: u64) -> bool {
    now < selected.probed_at_unix || now - selected.probed_at_unix < SELECTED_SOURCE_TTL_SECS
}

impl SelectedSource {
    /// 从一个刚选定的 `ModelSource` 构造缓存项(打当前时间戳)。
    pub fn from_source(source: &ModelSource) -> Self {
        Self {
            source_id: source.id.clone(),
            endpoint: source.endpoint.clone(),
            probed_at_unix: now_unix(),
        }
    }
}

/// 启动期 seed:把持久化(`app_settings.model_source`)的选定源接进**进程内**选源缓存,
/// 使**冷启动后第一次下载也免重探**(跨重启复用上次选定结果,只要仍 fresh)。
///
/// 这正是 §cache finding 要求的"把 read_selected_source / selected_source_is_fresh 接进
/// 解析路径"——否则二者是死代码。`settings` 由调用方(state.rs unlock 后)从 store 读出。
/// 该选定源回填进**两个**缓存桶(Full + Xenova class)的当前 region 槽:它是上次实测的 healthy
/// 源,作为 failover 首选合理;过期后正常重探自愈。返回 seed 的源数(0=无持久化/已过期)。
pub fn seed_resolution_cache_from_settings(settings: &serde_json::Value, region: Region) -> usize {
    let Some(sel) = read_selected_source(settings) else {
        return 0;
    };
    let now = now_unix();
    if !selected_source_is_fresh(&sel, now) {
        return 0; // 过期 → 不 seed,下次下载正常重探
    }
    // 用持久化 endpoint 重建一个 ModelSource(coverage 取 Full —— 选定源既然下过模型必是全覆盖
    // 候选;Xenova-class 桶也放它,whisper/OCR 类下载首选它,失败再 failover 到注册表其余源)。
    let seeded = builtin_sources()
        .into_iter()
        .find(|s| s.id == sel.source_id)
        .unwrap_or_else(|| ModelSource {
            id: sel.source_id.clone(),
            endpoint: sel.endpoint.clone(),
            priority: 50,
            region_hint: None,
            coverage: SourceCoverage::Full,
        });
    let mut cache = resolution_cache().lock().unwrap_or_else(|e| e.into_inner());
    let mut n = 0;
    for is_xenova in [false, true] {
        cache.insert(
            (region == Region::China, is_xenova),
            CachedResolution { order: vec![seeded.clone()], cached_at_unix: sel.probed_at_unix },
        );
        n += 1;
    }
    n
}

/// 把刚成功用过的源(`source_id`)持久化进 `app_settings`(供下次冷启动 seed)。纯函数返回
/// 新 JSON(调用方负责落 store),对齐 `write_selected_source` 的无 IO 约定。`None` = 未知源
/// (不写)。这让 `download_with_failover` 报出的 winning source 真正进入持久层(非死代码)。
pub fn persist_used_source(
    settings: serde_json::Value,
    source_id: &str,
) -> serde_json::Value {
    let Some(source) = builtin_sources().into_iter().find(|s| s.id == source_id) else {
        return settings; // env: 覆盖等非注册表源不持久化
    };
    write_selected_source(settings, &SelectedSource::from_source(&source))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_sources_ordered_company_first() {
        let sources = builtin_sources();
        // §12 内置序:company-mirror 优先级最高。
        let company = sources.iter().find(|s| s.id == "company-mirror").unwrap();
        let modelscope = sources.iter().find(|s| s.id == "modelscope").unwrap();
        let hf = sources.iter().find(|s| s.id == "hf-official").unwrap();
        assert!(company.priority > modelscope.priority);
        assert!(modelscope.priority > hf.priority || hf.region_hint == Some(Region::International));
        assert_eq!(company.coverage, SourceCoverage::Full);
    }

    #[test]
    fn modelscope_only_covers_xenova() {
        let sources = builtin_sources();
        let ms = sources.iter().find(|s| s.id == "modelscope").unwrap();
        assert_eq!(ms.coverage, SourceCoverage::OnlyXenovaOnnx);
        // embedding/reranker(Xenova ONNX)覆盖;whisper / PP-OCR 不覆盖 → selector 跳过。
        assert!(ms.coverage.covers("Xenova/bge-m3"));
        assert!(ms.coverage.covers("Xenova/bge-reranker-base"));
        assert!(!ms.coverage.covers("ggerganov/whisper.cpp"));
        assert!(!ms.coverage.covers("SWHL/RapidOCR"));
    }

    #[test]
    fn full_coverage_covers_everything() {
        assert!(SourceCoverage::Full.covers("ggerganov/whisper.cpp"));
        assert!(SourceCoverage::Full.covers("Xenova/bge-m3"));
        assert!(SourceCoverage::Full.covers("anything/at-all"));
    }

    #[test]
    fn company_mirror_env_override() {
        // 默认值是契约 URL;env 覆盖用于私有部署/测试。这里只断言默认形态(不 mutate env,
        // 与并行测试竞争);env 覆盖路径由集成测试覆盖。
        if std::env::var_os("ATTUNE_COMPANY_MIRROR").is_none() {
            assert_eq!(company_mirror_endpoint(), "https://models.engi-stack.com");
        }
        let ms = builtin_sources();
        let c = ms.iter().find(|s| s.id == "company-mirror").unwrap();
        assert!(!c.endpoint.ends_with('/'), "endpoint must have no trailing slash");
    }

    #[test]
    fn endpoints_have_no_trailing_slash() {
        // hf-hub 拼 `{endpoint}/{repo}/...`,尾 `/` 会产生 `//` → 部分源 404。
        for s in builtin_sources() {
            assert!(
                !s.endpoint.ends_with('/'),
                "source {} endpoint {} must not end with /",
                s.id,
                s.endpoint
            );
        }
    }

    #[test]
    fn unreachable_health_is_zero() {
        let h = SourceHealth::unreachable("hf-official");
        assert!(!h.reachable);
        assert_eq!(h.throughput_bps, 0.0);
        assert_eq!(h.latency_ms, None);
    }

    // --- 探测 / failover 用的 hand-rolled 单/多发 TcpListener mock(复用 embed.rs 既有
    //     idiom,不引新 dev-dep)。每个 mock server 处理 `n_requests` 个连接后退出。 ---

    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// 起一个 mock HF-resolve 源:对任意请求返回 200 + `body`(忽略 Range,模拟"源忽略 Range
    /// 返回全文"的退化路径,probe 仍受 total timeout + 字节计兜底)。返回 (endpoint, join_handle)。
    /// 处理 `n_requests` 次连接后退出。
    fn mock_source_ok(body: Vec<u8>, n_requests: usize) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            for _ in 0..n_requests {
                let (mut stream, _) = match listener.accept() {
                    Ok(s) => s,
                    Err(_) => return,
                };
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let resp_head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(resp_head.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            }
        });
        (format!("http://127.0.0.1:{port}"), handle)
    }

    /// 起一个 mock 源对任意请求返回 404(模拟该 repo 在源上无覆盖)。
    fn mock_source_404(n_requests: usize) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            for _ in 0..n_requests {
                let (mut stream, _) = match listener.accept() {
                    Ok(s) => s,
                    Err(_) => return,
                };
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                let _ = stream.flush();
            }
        });
        (format!("http://127.0.0.1:{port}"), handle)
    }

    fn src(id: &str, endpoint: &str, priority: u32) -> ModelSource {
        ModelSource {
            id: id.to_string(),
            endpoint: endpoint.to_string(),
            priority,
            region_hint: None,
            coverage: SourceCoverage::Full,
        }
    }

    #[test]
    fn probe_reachable_source_reports_throughput() {
        let (endpoint, handle) = mock_source_ok(vec![0u8; 64 * 1024], 1);
        let source = src("mock", &endpoint, 50);
        let health = probe_source_with(&source, "Xenova/bge-m3", "tokenizer.json");
        handle.join().unwrap();
        assert!(health.reachable, "mock 200 must be reachable");
        assert_eq!(health.source_id, "mock");
        assert!(health.throughput_bps > 0.0, "throughput must be positive");
        assert!(health.latency_ms.is_some());
    }

    #[test]
    fn probe_404_source_is_unreachable() {
        // 源对该 repo 404(无覆盖)→ 视为该探测不可用(不 panic,不 hang)。
        let (endpoint, handle) = mock_source_404(1);
        let source = src("mock404", &endpoint, 50);
        let health = probe_source_with(&source, "ggerganov/whisper.cpp", "ggml-base.bin");
        handle.join().unwrap();
        assert!(!health.reachable, "404 must be unreachable");
        assert_eq!(health.throughput_bps, 0.0);
    }

    #[test]
    fn probe_black_hole_fails_fast() {
        // 127.0.0.1:1 无监听 → connect refused/timeout;断言有界失败(不永久 hang)。
        let source = src("blackhole", "http://127.0.0.1:1", 50);
        let start = std::time::Instant::now();
        let health = probe_source_with(&source, "Xenova/bge-m3", "tokenizer.json");
        let elapsed = start.elapsed();
        assert!(!health.reachable);
        assert!(
            elapsed < PROBE_TOTAL_TIMEOUT + Duration::from_secs(5),
            "probe must fail-fast, took {elapsed:?}"
        );
    }

    // --- 选源 / failover 排序(纯逻辑,无网络;用构造的 health 注入)。 ---

    fn srch(id: &str, priority: u32, region: Option<Region>, cov: SourceCoverage) -> ModelSource {
        ModelSource {
            id: id.to_string(),
            endpoint: format!("https://{id}.example"),
            priority,
            region_hint: region,
            coverage: cov,
        }
    }

    fn health(id: &str, reachable: bool, bps: f64) -> SourceHealth {
        SourceHealth {
            source_id: id.to_string(),
            reachable,
            throughput_bps: bps,
            latency_ms: if reachable { Some(50) } else { None },
        }
    }

    #[test]
    fn select_filters_unreachable_and_uncovered() {
        let probed = vec![
            // 不覆盖 whisper(OnlyXenovaOnnx)→ 剔除
            (srch("ms", 80, Some(Region::China), SourceCoverage::OnlyXenovaOnnx), health("ms", true, 5e6)),
            // 不可达 → 剔除
            (srch("dead", 100, None, SourceCoverage::Full), health("dead", false, 0.0)),
            // healthy + 覆盖 → 保留
            (srch("ok", 60, None, SourceCoverage::Full), health("ok", true, 1e6)),
        ];
        let order = select_failover_order(&probed, "ggerganov/whisper.cpp", Region::International);
        assert_eq!(order.len(), 1, "only the covering+reachable source survives");
        assert_eq!(order[0].id, "ok");
    }

    #[test]
    fn select_sorts_by_throughput_times_priority() {
        // 两源都 Full + healthy:throughput×priority 决定序。
        // fast: 4MB/s × 50 = 2e8;slow: 1MB/s × 100 = 1e8 → fast 应在前。
        let probed = vec![
            (srch("slow", 100, None, SourceCoverage::Full), health("slow", true, 1e6)),
            (srch("fast", 50, None, SourceCoverage::Full), health("fast", true, 4e6)),
        ];
        let order = select_failover_order(&probed, "Xenova/bge-m3", Region::International);
        assert_eq!(order[0].id, "fast", "higher throughput×priority wins");
        assert_eq!(order[1].id, "slow");
    }

    #[test]
    fn select_region_hint_boosts_same_region() {
        // 两源 throughput×priority 持平(都 = 1e6×60 = 6e7);CN 区时 CN-hint 源因 1.5x 偏置抬前。
        let probed = vec![
            (srch("intl", 60, Some(Region::International), SourceCoverage::Full), health("intl", true, 1e6)),
            (srch("cn", 60, Some(Region::China), SourceCoverage::Full), health("cn", true, 1e6)),
        ];
        let order = select_failover_order(&probed, "Xenova/bge-m3", Region::China);
        assert_eq!(order[0].id, "cn", "CN-region source boosted in CN region");
    }

    #[test]
    fn select_throughput_can_beat_region_bias() {
        // region 偏置只是 1.5x,不该让一个极慢的同区源压过区外快源(§12:实测能翻盘)。
        // cn(同区): 1e5 × 60 × 1.5 = 9e6;intl(区外): 1e6 × 60 × 1 = 6e7 → intl 仍在前。
        let probed = vec![
            (srch("cn", 60, Some(Region::China), SourceCoverage::Full), health("cn", true, 1e5)),
            (srch("intl", 60, Some(Region::International), SourceCoverage::Full), health("intl", true, 1e6)),
        ];
        let order = select_failover_order(&probed, "Xenova/bge-m3", Region::China);
        assert_eq!(order[0].id, "intl", "fast out-of-region must beat slow in-region");
    }

    #[test]
    fn select_empty_when_no_eligible() {
        let probed = vec![
            (srch("dead", 100, None, SourceCoverage::Full), health("dead", false, 0.0)),
            (srch("ms", 80, None, SourceCoverage::OnlyXenovaOnnx), health("ms", true, 5e6)),
        ];
        // whisper 不被 OnlyXenovaOnnx 覆盖,dead 不可达 → 空
        let order = select_failover_order(&probed, "ggerganov/whisper.cpp", Region::China);
        assert!(order.is_empty(), "no eligible source → empty failover order");
    }

    // --- download_with_failover(集成:逐源下载 + 切次优)。这些测试 mutate HF_ENDPOINT
    //     env 以验证向后兼容逃生门;env 是进程全局 → 用一个共享 Mutex 串行化(不引
    //     serial_test dev-dep),并用 RAII guard 保证恢复原值。 ---

    // 串行锁:所有 mutate HF_ENDPOINT 的测试持有它,避免与彼此(及未来同 env 测试)竞争。
    static HF_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 临时清掉/设置 HF_ENDPOINT;Drop 时恢复原值。持有 HF_ENV_LOCK 的 guard 串行化访问。
    struct HfEndpointGuard {
        prev: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }
    impl HfEndpointGuard {
        fn acquire(set_to: Option<&str>) -> Self {
            let lock = HF_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var("HF_ENDPOINT").ok();
            #[allow(unsafe_code)]
            unsafe {
                match set_to {
                    Some(v) => std::env::set_var("HF_ENDPOINT", v),
                    None => std::env::remove_var("HF_ENDPOINT"),
                }
            }
            Self { prev, _lock: lock }
        }
        fn clear() -> Self {
            Self::acquire(None)
        }
        fn set(val: &str) -> Self {
            Self::acquire(Some(val))
        }
    }
    impl Drop for HfEndpointGuard {
        fn drop(&mut self) {
            #[allow(unsafe_code)]
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var("HF_ENDPOINT", v),
                    None => std::env::remove_var("HF_ENDPOINT"),
                }
            }
        }
    }

    #[test]
    fn failover_skips_dead_source_uses_next() {
        let _g = HfEndpointGuard::clear();
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("model.bin");
        let (good_endpoint, handle) = mock_source_ok(b"REAL-MODEL-BYTES".to_vec(), 1);
        // 首选是黑洞(连不上),次优是 good mock → 应 failover 到 good 并成功。
        let sources = vec![
            src("dead", "http://127.0.0.1:1", 100),
            src("good", &good_endpoint, 50),
        ];
        let used = download_with_failover(&sources, "Xenova/bge-m3", "model.bin", &dst)
            .expect("failover to good source must succeed");
        handle.join().unwrap();
        assert_eq!(used, "good", "must report the source that actually worked");
        assert_eq!(std::fs::read(&dst).unwrap(), b"REAL-MODEL-BYTES");
    }

    #[test]
    fn failover_all_dead_returns_err() {
        let _g = HfEndpointGuard::clear();
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("model.bin");
        let (e404, h404) = mock_source_404(1);
        let sources = vec![
            src("dead", "http://127.0.0.1:1", 100),
            src("notfound", &e404, 50),
        ];
        let r = download_with_failover(&sources, "Xenova/bge-m3", "model.bin", &dst);
        h404.join().unwrap();
        assert!(r.is_err(), "all sources dead → Err");
        assert!(!dst.exists(), "no file on total failure");
    }

    #[test]
    fn failover_empty_sources_returns_err() {
        let _g = HfEndpointGuard::clear();
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("model.bin");
        let r = download_with_failover(&[], "Xenova/bge-m3", "model.bin", &dst);
        assert!(r.is_err(), "empty source list → Err (degraded)");
    }

    // --- 进程内选源缓存接进解析路径(§cache finding:fresh 命中跳过重探)。 ---
    //     缓存是进程全局静态 → 用一把锁串行化这组测试,避免桶互相污染计数。

    static RESOLVE_CACHE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn resolve_cache_fresh_hit_skips_reprobe() {
        let _g = RESOLVE_CACHE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_resolution_cache();
        // 探测计数器:每次 probe_fn 被调即 +1。第一次 resolve 真探(>0),第二次 fresh 命中应 0。
        let count = std::sync::atomic::AtomicUsize::new(0);
        let probe = |s: &ModelSource| {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // 标记 company-mirror healthy,其余不可达 → 选序确定(company 首位)。
            health(&s.id, s.id == "company-mirror", 1e6)
        };
        let first = resolve_sources_with("Xenova/bge-m3", Region::China, &probe);
        let after_first = count.load(std::sync::atomic::Ordering::SeqCst);
        assert!(after_first > 0, "first resolve must probe at least once");
        assert!(!first.is_empty(), "company-mirror healthy → non-empty order");

        // 第二次同 (region, class):fresh 命中 → probe_fn 零调用,顺序一致。
        let second = resolve_sources_with("Xenova/bge-reranker-base", Region::China, &probe);
        assert_eq!(
            count.load(std::sync::atomic::Ordering::SeqCst),
            after_first,
            "fresh cache hit must NOT re-probe"
        );
        assert_eq!(
            first.iter().map(|s| &s.id).collect::<Vec<_>>(),
            second.iter().map(|s| &s.id).collect::<Vec<_>>(),
            "cached order reused verbatim"
        );
        clear_resolution_cache();
    }

    #[test]
    fn resolve_cache_distinct_class_and_region_buckets() {
        let _g = RESOLVE_CACHE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_resolution_cache();
        let count = std::sync::atomic::AtomicUsize::new(0);
        let probe = |s: &ModelSource| {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            health(&s.id, true, 1e6)
        };
        // Full-class repo (whisper) 与 Xenova-class repo 用**不同桶** → 各自首次都要探测。
        let _ = resolve_sources_with("ggerganov/whisper.cpp", Region::China, &probe);
        let after_full = count.load(std::sync::atomic::Ordering::SeqCst);
        let _ = resolve_sources_with("Xenova/bge-m3", Region::China, &probe);
        let after_xenova = count.load(std::sync::atomic::Ordering::SeqCst);
        assert!(after_xenova > after_full, "distinct class bucket must probe (not reuse Full bucket)");
        clear_resolution_cache();
    }

    #[test]
    fn seed_from_settings_populates_cache_and_skips_probe() {
        let _g = RESOLVE_CACHE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_resolution_cache();
        // 持久化一个 fresh 选定源(company-mirror, 刚探测)。
        let sel = SelectedSource {
            source_id: "company-mirror".into(),
            endpoint: company_mirror_endpoint(),
            probed_at_unix: now_unix(),
        };
        let settings = write_selected_source(serde_json::json!({}), &sel);
        let seeded = seed_resolution_cache_from_settings(&settings, Region::China);
        assert_eq!(seeded, 2, "seeds both Full + Xenova class buckets");

        // seed 后 resolve 应直接命中 seeded 源,probe 零调用。
        let count = std::sync::atomic::AtomicUsize::new(0);
        let probe = |s: &ModelSource| {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            health(&s.id, true, 1e6)
        };
        let order = resolve_sources_with("SWHL/RapidOCR", Region::China, &probe);
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 0, "seeded cache → no probe");
        assert_eq!(order.first().map(|s| s.id.as_str()), Some("company-mirror"));
        clear_resolution_cache();
    }

    #[test]
    fn seed_from_settings_expired_does_not_seed() {
        let _g = RESOLVE_CACHE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_resolution_cache();
        // 过期选定源(probed_at 远在 TTL 之前)→ 不 seed。
        let sel = SelectedSource {
            source_id: "company-mirror".into(),
            endpoint: company_mirror_endpoint(),
            probed_at_unix: now_unix().saturating_sub(SELECTED_SOURCE_TTL_SECS + 100),
        };
        let settings = write_selected_source(serde_json::json!({}), &sel);
        assert_eq!(seed_resolution_cache_from_settings(&settings, Region::China), 0);
        clear_resolution_cache();
    }

    #[test]
    fn persist_used_source_roundtrips_registry_source() {
        // download_with_failover 报出的 winning id → 持久化 → 下次可读回。
        let out = persist_used_source(serde_json::json!({"keep": 1}), "modelscope");
        assert_eq!(out["keep"], 1, "sibling key preserved");
        let back = read_selected_source(&out).expect("must persist");
        assert_eq!(back.source_id, "modelscope");
        // 非注册表源(env: 覆盖)不写。
        let unchanged = persist_used_source(serde_json::json!({"x": 2}), "env:http://foo");
        assert!(read_selected_source(&unchanged).is_none(), "non-registry source not persisted");
    }

    // --- 选定源缓存(§12.3:settings 持久化 + TTL 新鲜度)。纯 JSON,无 IO/无 env。 ---

    #[test]
    fn selected_source_roundtrip_in_settings() {
        let source = src("modelscope", "https://modelscope.cn/models", 80);
        let sel = SelectedSource::from_source(&source);
        let settings = serde_json::json!({"llm": {"model": "x"}});
        let written = write_selected_source(settings, &sel);
        // 既有 key 不动
        assert_eq!(written["llm"]["model"], "x");
        // 读回一致
        let back = read_selected_source(&written).expect("must read back");
        assert_eq!(back.source_id, "modelscope");
        assert_eq!(back.endpoint, "https://modelscope.cn/models");
        assert_eq!(back.probed_at_unix, sel.probed_at_unix);
    }

    #[test]
    fn read_selected_source_absent_or_invalid() {
        assert!(read_selected_source(&serde_json::json!({})).is_none());
        // 节存在但 schema 错(字符串而非对象)→ None,不 panic
        assert!(read_selected_source(&serde_json::json!({"model_source": "garbage"})).is_none());
        // 缺字段 → None
        assert!(
            read_selected_source(&serde_json::json!({"model_source": {"source_id": "x"}}))
                .is_none()
        );
    }

    #[test]
    fn write_selected_source_into_non_object() {
        let sel = SelectedSource {
            source_id: "hf".into(),
            endpoint: "https://huggingface.co".into(),
            probed_at_unix: 100,
        };
        let out = write_selected_source(serde_json::json!("not-an-object"), &sel);
        assert_eq!(read_selected_source(&out).unwrap().source_id, "hf");
    }

    #[test]
    fn selected_source_freshness_ttl() {
        let sel = SelectedSource {
            source_id: "ms".into(),
            endpoint: "https://modelscope.cn/models".into(),
            probed_at_unix: 1000,
        };
        // 刚探测 → 新鲜
        assert!(selected_source_is_fresh(&sel, 1000));
        // TTL 内 → 新鲜
        assert!(selected_source_is_fresh(&sel, 1000 + SELECTED_SOURCE_TTL_SECS - 1));
        // 超 TTL → 过期(应重探)
        assert!(!selected_source_is_fresh(&sel, 1000 + SELECTED_SOURCE_TTL_SECS + 1));
        // 时钟回拨(now < probed_at)→ 视为新鲜,不误判过期
        assert!(selected_source_is_fresh(&sel, 500));
    }

    #[test]
    fn explicit_hf_endpoint_overrides_failover() {
        // 向后兼容:显式 HF_ENDPOINT 设了 → 只用它(单源直下),忽略 sources 列表。
        let (explicit_endpoint, handle) = mock_source_ok(b"VIA-ENV".to_vec(), 1);
        let _g = HfEndpointGuard::set(&explicit_endpoint);
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("model.bin");
        // sources 里全是黑洞;但 env 逃生门应让它走 explicit_endpoint。
        let sources = vec![src("dead", "http://127.0.0.1:1", 100)];
        let used = download_with_failover(&sources, "Xenova/bge-m3", "model.bin", &dst)
            .expect("explicit HF_ENDPOINT must be honored");
        handle.join().unwrap();
        assert!(used.starts_with("env:"), "must report env override, got {used}");
        assert_eq!(std::fs::read(&dst).unwrap(), b"VIA-ENV");
    }

    // --- 属性测试(proptest ≥3,per spec §9 + Agent 验证铁律):选源/缓存对任意输入
    //     不 panic、不变量恒成立。纯逻辑,无网络。 ---
    use proptest::prelude::*;

    prop_compose! {
        fn arb_source()(
            id in "[a-z]{1,8}",
            priority in 1u32..200,
            bps in 0.0f64..1e8,
            reachable in any::<bool>(),
            is_cn in any::<bool>(),
            full in any::<bool>(),
        ) -> (ModelSource, SourceHealth) {
            let source = ModelSource {
                id: id.clone(),
                endpoint: format!("https://{id}.example"),
                priority,
                region_hint: if is_cn { Some(Region::China) } else { Some(Region::International) },
                coverage: if full { SourceCoverage::Full } else { SourceCoverage::OnlyXenovaOnnx },
            };
            let health = SourceHealth {
                source_id: id,
                reachable,
                throughput_bps: if reachable { bps } else { 0.0 },
                latency_ms: if reachable { Some(10) } else { None },
            };
            (source, health)
        }
    }

    proptest! {
        // ① 任意源/health 组合:select 不 panic;输出只含 covered+reachable 源;
        //    输出每项的排序键单调非增(failover 顺序合法)。
        //    注:把生成的 source.id 用下标去重(probed 是 (source, health) 配对,id 唯一才能
        //    在验证侧按 id 回查到**正确**那条 health;否则是测试 oracle 的二义,非选源逻辑 bug)。
        #[test]
        fn prop_select_never_panics_and_filters(
            raw in prop::collection::vec(arb_source(), 0..12),
            repo in prop_oneof!["Xenova/bge-m3", "ggerganov/whisper.cpp", "SWHL/RapidOCR"],
            cn in any::<bool>(),
        ) {
            // 下标去重 id + 同步 health.source_id,保证 (source,health) 配对的 id 全局唯一。
            let probed: Vec<(ModelSource, SourceHealth)> = raw
                .into_iter()
                .enumerate()
                .map(|(i, (mut s, mut h))| {
                    let uniq = format!("{}-{i}", s.id);
                    s.id = uniq.clone();
                    s.endpoint = format!("https://{uniq}.example");
                    h.source_id = uniq;
                    (s, h)
                })
                .collect();
            let region = if cn { Region::China } else { Region::International };
            let order = select_failover_order(&probed, &repo, region);
            // 输出只含 covered + reachable 的源
            for s in &order {
                prop_assert!(s.coverage.covers(&repo), "uncovered source leaked into order");
                let h = probed.iter().find(|(src, _)| src.id == s.id).map(|(_, h)| h);
                prop_assert!(h.map(|h| h.reachable).unwrap_or(false), "unreachable leaked");
            }
            // 输出数量 ≤ 输入
            prop_assert!(order.len() <= probed.len());
            // 排序键单调非增(降序)
            let score = |s: &ModelSource| -> f64 {
                let h = probed.iter().find(|(src,_)| src.id == s.id).map(|(_,h)| h.throughput_bps).unwrap_or(0.0);
                let bias = if s.region_hint == Some(region) { REGION_MATCH_BIAS } else { 1.0 };
                h * s.priority as f64 * bias
            };
            for w in order.windows(2) {
                prop_assert!(score(&w[0]) >= score(&w[1]), "order not descending by score");
            }
        }

        // ② read_selected_source 对任意 JSON 不 panic(garbage in → None/Some,never crash)。
        #[test]
        fn prop_read_selected_never_panics(
            sid in ".*",
            ep in ".*",
            ts in any::<u64>(),
            wrap in any::<bool>(),
        ) {
            let node = if wrap {
                serde_json::json!({"source_id": sid, "endpoint": ep, "probed_at_unix": ts})
            } else {
                serde_json::json!(sid) // 非对象 garbage
            };
            let settings = serde_json::json!({"model_source": node, "other": 1});
            // 不 panic 即通过;合法 schema 时读回字段一致。
            if let Some(sel) = read_selected_source(&settings) {
                prop_assert_eq!(sel.probed_at_unix, ts);
            }
        }

        // ③ write→read 缓存幂等:任意合法 SelectedSource 写入再读出字段不变,且不破坏 sibling key。
        #[test]
        fn prop_selected_source_write_read_roundtrip(
            sid in "[a-z]{1,10}",
            ep in "https://[a-z]{1,10}\\.example",
            ts in any::<u64>(),
        ) {
            let sel = SelectedSource { source_id: sid.clone(), endpoint: ep.clone(), probed_at_unix: ts };
            let settings = serde_json::json!({"sibling": "keep-me"});
            let out = write_selected_source(settings, &sel);
            prop_assert_eq!(&out["sibling"], "keep-me");
            let back = read_selected_source(&out).expect("must roundtrip");
            prop_assert_eq!(back.source_id, sid);
            prop_assert_eq!(back.endpoint, ep);
            prop_assert_eq!(back.probed_at_unix, ts);
        }
    }
}
