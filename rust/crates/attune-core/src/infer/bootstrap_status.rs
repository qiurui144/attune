//! 底座模型后台下载状态（#2 #5）。
//!
//! 解锁不再同步阻塞在 ~330MB 模型下载上（embedding + reranker），而是解锁立即返回、
//! 后台线程拉取四类底座模型（embedding / reranker / ocr / asr）。本模块提供一个
//! 线程安全的进度快照，供 GET /ai_stack 暴露，让首次运行的用户看到"正在下载/已就绪/
//! 失败"而非静默卡住。
//!
//! 失败不向上抛 panic（§4.5）：每类记录 `Failed { error }` + 重试次数，UI 可提示重试。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// 单类底座模型的下载阶段。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ModelPhase {
    /// 尚未开始（后台任务还没轮到 / 未触发）。
    Pending,
    /// 正在下载（含 failover 镜像尝试）。
    Downloading,
    /// 已就绪（缓存命中或下载完成）。
    Ready,
    /// 失败（缓存缺失 + 所有镜像都拉取失败 / 离线）。
    Failed { error: String },
}

impl ModelPhase {
    pub fn is_ready(&self) -> bool {
        matches!(self, ModelPhase::Ready)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelEntry {
    /// flatten 让 `state` / `error` 直接出现在 entry 顶层（而非嵌套在 "phase" 下），
    /// 便于 UI 读 `models.<class>.state` / `.error`。
    #[serde(flatten)]
    pub phase: ModelPhase,
    /// 已尝试下载的次数（含失败重试）；用于 telemetry / UI "重试 N 次"。
    pub attempts: u32,
}

impl Default for ModelEntry {
    fn default() -> Self {
        Self { phase: ModelPhase::Pending, attempts: 0 }
    }
}

/// 四类底座模型的下载状态快照。`Arc<Mutex<..>>` 内部共享，clone 廉价。
///
/// key 是稳定标识：`embedding` / `reranker` / `ocr` / `asr`。
#[derive(Clone, Default)]
pub struct ModelBootstrapStatus {
    inner: Arc<Mutex<BTreeMap<String, ModelEntry>>>,
}

/// 四类底座模型的稳定 key（也定义了"全部都被调度"的预期集合）。
pub const MODEL_CLASSES: [&str; 4] = ["embedding", "reranker", "ocr", "asr"];

impl ModelBootstrapStatus {
    pub fn new() -> Self {
        let mut m = BTreeMap::new();
        for k in MODEL_CLASSES {
            m.insert(k.to_string(), ModelEntry::default());
        }
        Self { inner: Arc::new(Mutex::new(m)) }
    }

    /// 标记某类进入"下载中"，attempts += 1。
    pub fn mark_downloading(&self, class: &str) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let e = g.entry(class.to_string()).or_default();
        e.phase = ModelPhase::Downloading;
        e.attempts += 1;
    }

    /// 标记某类就绪。
    pub fn mark_ready(&self, class: &str) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.entry(class.to_string()).or_default().phase = ModelPhase::Ready;
    }

    /// 标记某类失败（带错误信息，不 panic）。
    pub fn mark_failed(&self, class: &str, error: impl Into<String>) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.entry(class.to_string()).or_default().phase = ModelPhase::Failed { error: error.into() };
    }

    /// 读取某类当前阶段。
    pub fn phase(&self, class: &str) -> Option<ModelPhase> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.get(class).map(|e| e.phase.clone())
    }

    /// 全部就绪？
    pub fn all_ready(&self) -> bool {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        MODEL_CLASSES.iter().all(|k| g.get(*k).map(|e| e.phase.is_ready()).unwrap_or(false))
    }

    /// JSON 快照（供 /ai_stack 嵌入）。
    pub fn snapshot(&self) -> serde_json::Value {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let all_ready = MODEL_CLASSES.iter().all(|k| g.get(*k).map(|e| e.phase.is_ready()).unwrap_or(false));
        let mut models = serde_json::Map::new();
        for (k, v) in g.iter() {
            models.insert(k.clone(), serde_json::to_value(v).unwrap_or(serde_json::Value::Null));
        }
        serde_json::json!({
            "all_ready": all_ready,
            "models": serde_json::Value::Object(models),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_starts_all_pending() {
        let s = ModelBootstrapStatus::new();
        for k in MODEL_CLASSES {
            assert_eq!(s.phase(k), Some(ModelPhase::Pending), "{k} should start Pending");
        }
        assert!(!s.all_ready());
    }

    #[test]
    fn downloading_increments_attempts() {
        let s = ModelBootstrapStatus::new();
        s.mark_downloading("embedding");
        s.mark_downloading("embedding");
        let snap = s.snapshot();
        assert_eq!(snap["models"]["embedding"]["attempts"], 2);
        assert_eq!(snap["models"]["embedding"]["state"], "downloading");
    }

    #[test]
    fn all_ready_only_when_all_four_ready() {
        let s = ModelBootstrapStatus::new();
        for k in MODEL_CLASSES {
            assert!(!s.all_ready());
            s.mark_ready(k);
        }
        assert!(s.all_ready());
        assert_eq!(s.snapshot()["all_ready"], true);
    }

    #[test]
    fn failed_records_error_not_panic() {
        let s = ModelBootstrapStatus::new();
        s.mark_failed("ocr", "network down");
        match s.phase("ocr").unwrap() {
            ModelPhase::Failed { error } => assert_eq!(error, "network down"),
            other => panic!("expected Failed, got {other:?}"),
        }
        assert_eq!(s.snapshot()["models"]["ocr"]["state"], "failed");
        assert_eq!(s.snapshot()["models"]["ocr"]["error"], "network down");
    }
}
