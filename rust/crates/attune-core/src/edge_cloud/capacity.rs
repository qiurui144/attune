//! k3-scheduler `/capacity` 容量信号客户端。
//!
//! Spec: `docs/superpowers/specs/2026-06-22-edge-cloud-model1.md` §5（HTTP 契约）+ §7（降级）。
//!
//! attune 提交推理前 `GET http://127.0.0.1:8090/capacity?model=X` →
//! `{state, eta_ms, mem_headroom_mb}`。**任何失败（超时/连接拒/非200/畸形）→
//! `CapacityState::Unknown`**（graceful，退回静态二分，绝不崩 / 绝不 block 推理热路径，§7 R2）。
//!
//! probe 在路由决策前**一次性**查；不进推理热路径，短超时（默认 1.5s）。

use serde::Deserialize;
use std::time::Duration;

/// k3-scheduler 默认 loopback 端点（:8090，绝不经 frpc 暴露，设计 doc §5.E）。
pub const DEFAULT_SCHEDULER_BASE: &str = "http://127.0.0.1:8090";

/// probe 默认超时（短，失败即降级；不拖垮请求路径，§7 R2）。
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

/// 本地容量状态。`Unknown` = 查询失败 / 非 K3 形态（降级哨兵，非 scheduler 真返回值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityState {
    /// 本地空闲，可立即快速服务。
    ReadyFast,
    /// 本地排队中（有 eta）。
    Queued,
    /// 本地可服务但慢（内存吃紧 / 降频）。
    ReadySlow,
    /// 本地此刻不可用（worker 挂 / 内存不足以加载该模型）。
    Unavailable,
    /// 未知 —— probe 失败或非 K3 形态。决策按静态二分（degrade）。
    Unknown,
}

impl CapacityState {
    pub fn as_str(self) -> &'static str {
        match self {
            CapacityState::ReadyFast => "ready-fast",
            CapacityState::Queued => "queued",
            CapacityState::ReadySlow => "ready-slow",
            CapacityState::Unavailable => "unavailable",
            CapacityState::Unknown => "unknown",
        }
    }

    /// 解析 scheduler 大写态串（容忍大小写）。未知串 → `Unknown`（降级，不炸）。
    fn parse(s: &str) -> Self {
        match s.trim().to_ascii_uppercase().as_str() {
            "READY_FAST" | "READY-FAST" | "READYFAST" => CapacityState::ReadyFast,
            "QUEUED" => CapacityState::Queued,
            "READY_SLOW" | "READY-SLOW" | "READYSLOW" => CapacityState::ReadySlow,
            "UNAVAILABLE" => CapacityState::Unavailable,
            _ => CapacityState::Unknown,
        }
    }

    /// 本地此刻能快速服务（无明显排队）。
    pub fn is_fast(self) -> bool {
        matches!(self, CapacityState::ReadyFast)
    }
}

/// 容量信号（每次查询的内存值，不落库）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapacitySignal {
    pub state: CapacityState,
    /// 预计就绪毫秒（排队/慢路径时 > 0；ReadyFast≈0）。
    pub eta_ms: u32,
    /// 该模型可用显存余量 MB（0 = 不足/未知）。
    pub mem_headroom_mb: u32,
}

impl CapacitySignal {
    /// 降级哨兵：probe 失败 / 非 K3 → Unknown（eta/headroom 归零）。
    pub const fn unknown() -> Self {
        CapacitySignal { state: CapacityState::Unknown, eta_ms: 0, mem_headroom_mb: 0 }
    }
}

/// scheduler `/capacity` 响应反序列化（容忍未知字段 + 缺字段默认）。
#[derive(Debug, Deserialize)]
struct CapacityResponse {
    #[serde(default)]
    state: String,
    #[serde(default)]
    eta_ms: u32,
    #[serde(default)]
    mem_headroom_mb: u32,
}

/// 容量探测抽象（可换实现：HTTP / mock / 未来 gRPC）。
///
/// **契约**：`query` **永不返回 Err / 永不 panic**；任何失败 → `CapacitySignal::unknown()`。
/// 这样路由层永远拿到一个信号（最坏 Unknown → 静态二分）。
pub trait CapacityProbe: Send + Sync {
    fn query(&self, model: &str) -> CapacitySignal;
}

/// 真 HTTP 客户端：`GET {base}/capacity?model=`。
pub struct HttpCapacityClient {
    base_url: String,
    client: reqwest::blocking::Client,
}

impl HttpCapacityClient {
    /// 默认指向 :8090 loopback。`timeout` 短，失败即降级。
    pub fn new() -> Self {
        Self::with_base(DEFAULT_SCHEDULER_BASE, DEFAULT_PROBE_TIMEOUT)
    }

    pub fn with_base(base_url: &str, timeout: Duration) -> Self {
        // 构建失败（极罕见）→ 仍给一个 client（default）；query 失败再降级。
        let client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            .build()
            .unwrap_or_default();
        HttpCapacityClient { base_url: base_url.trim_end_matches('/').to_string(), client }
    }
}

impl Default for HttpCapacityClient {
    fn default() -> Self {
        Self::new()
    }
}

impl CapacityProbe for HttpCapacityClient {
    fn query(&self, model: &str) -> CapacitySignal {
        let url = format!("{}/capacity", self.base_url);
        let resp = match self.client.get(&url).query(&[("model", model)]).send() {
            Ok(r) => r,
            Err(e) => {
                log::warn!("capacity probe unreachable ({e}); degrading to Unknown. error-code=capacity-probe-unreachable");
                return CapacitySignal::unknown();
            }
        };
        if !resp.status().is_success() {
            log::warn!("capacity probe non-2xx ({}); degrading to Unknown", resp.status());
            return CapacitySignal::unknown();
        }
        match resp.json::<CapacityResponse>() {
            Ok(body) => CapacitySignal {
                state: CapacityState::parse(&body.state),
                eta_ms: body.eta_ms,
                mem_headroom_mb: body.mem_headroom_mb,
            },
            Err(e) => {
                log::warn!("capacity probe parse failed ({e}); degrading to Unknown. error-code=capacity-parse-failed");
                CapacitySignal::unknown()
            }
        }
    }
}

/// Mock probe（离线测，§1.6）：预置信号 + 失败模式 + 调用计数（个人版 0 回退 guard 用）。
pub struct MockCapacityProbe {
    signal: CapacitySignal,
    calls: std::sync::atomic::AtomicUsize,
}

impl MockCapacityProbe {
    pub fn new(signal: CapacitySignal) -> Self {
        MockCapacityProbe { signal, calls: std::sync::atomic::AtomicUsize::new(0) }
    }

    /// 永远返回 Unknown（模拟 probe 不可达 / 降级路径）。
    pub fn failing() -> Self {
        Self::new(CapacitySignal::unknown())
    }

    /// 便捷：给定 state（eta/headroom 取合理默认）。
    pub fn with_state(state: CapacityState) -> Self {
        let (eta, head) = match state {
            CapacityState::ReadyFast => (0, 8192),
            CapacityState::Queued => (3000, 4096),
            CapacityState::ReadySlow => (8000, 1024),
            CapacityState::Unavailable | CapacityState::Unknown => (0, 0),
        };
        Self::new(CapacitySignal { state, eta_ms: eta, mem_headroom_mb: head })
    }

    /// query 被调用次数（spy；guard 测试断言个人版 = 0）。
    pub fn call_count(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl CapacityProbe for MockCapacityProbe {
    fn query(&self, _model: &str) -> CapacitySignal {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.signal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_state_uppercase_and_dashes() {
        assert_eq!(CapacityState::parse("READY_FAST"), CapacityState::ReadyFast);
        assert_eq!(CapacityState::parse("ready-fast"), CapacityState::ReadyFast);
        assert_eq!(CapacityState::parse("QUEUED"), CapacityState::Queued);
        assert_eq!(CapacityState::parse("READY_SLOW"), CapacityState::ReadySlow);
        assert_eq!(CapacityState::parse("UNAVAILABLE"), CapacityState::Unavailable);
    }

    #[test]
    fn unknown_state_string_degrades_not_panics() {
        // 未知态串 → Unknown（不炸）。
        assert_eq!(CapacityState::parse("WAT"), CapacityState::Unknown);
        assert_eq!(CapacityState::parse(""), CapacityState::Unknown);
    }

    #[test]
    fn unknown_signal_is_zeroed() {
        let s = CapacitySignal::unknown();
        assert_eq!(s.state, CapacityState::Unknown);
        assert_eq!(s.eta_ms, 0);
        assert_eq!(s.mem_headroom_mb, 0);
    }

    #[test]
    fn mock_probe_returns_preset_and_counts() {
        let p = MockCapacityProbe::with_state(CapacityState::ReadyFast);
        assert_eq!(p.call_count(), 0);
        let s = p.query("qwen2.5-7b");
        assert_eq!(s.state, CapacityState::ReadyFast);
        assert_eq!(p.call_count(), 1);
        p.query("x");
        assert_eq!(p.call_count(), 2);
    }

    #[test]
    fn failing_probe_yields_unknown() {
        let p = MockCapacityProbe::failing();
        assert_eq!(p.query("any").state, CapacityState::Unknown);
    }

    #[test]
    fn state_labels_stable_kebab() {
        assert_eq!(CapacityState::ReadyFast.as_str(), "ready-fast");
        assert_eq!(CapacityState::Queued.as_str(), "queued");
        assert_eq!(CapacityState::Unavailable.as_str(), "unavailable");
        assert_eq!(CapacityState::Unknown.as_str(), "unknown");
    }
}
