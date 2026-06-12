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

/// 便捷:对内置注册表逐源探测(覆盖 `repo_id` 的源才探,省掉必 404 的探测)→ 选 failover 顺序。
/// **只在 pre-flight / 显式触发调用**(R3)。`detect_region()` 决定 region 偏置。
pub fn resolve_sources_for(repo_id: &str) -> Vec<ModelSource> {
    let region = crate::platform::region::detect_region();
    let probed: Vec<(ModelSource, SourceHealth)> = builtin_sources()
        .into_iter()
        .filter(|s| s.coverage.covers(repo_id))
        .map(|s| {
            let h = probe_source(&s);
            (s, h)
        })
        .collect();
    select_failover_order(&probed, repo_id, region)
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
}
