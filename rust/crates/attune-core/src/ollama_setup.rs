//! Ollama 一键就绪 (zero-terminal UX)
//!
//! 面向非技术用户：所有第三方依赖 (Ollama runtime / 本地模型) 都必须能在
//! 应用内一键拉取部署，绝不让用户去终端敲命令。本模块提供两块纯逻辑：
//!
//! 1. [`check_readiness`] — 把 "daemon 是否在 + 配置模型是否已下载" 归一成
//!    三态 [`OllamaReadiness`]，UI 据此渲染 🔴 / 🟡 / 🟢 + 对应一键按钮。
//! 2. [`install_plan`] — 给定平台返回 "如何一键安装 Ollama" 的执行计划
//!    ([`OllamaInstallPlan`])：能脚本化的平台返回命令，装不了的平台 graceful
//!    降级给官网下载链接。
//!
//! HTTP probe 本身在 server 侧做 (复用 reqwest)；core 只持有 model-match 纯逻辑，
//! 这样可以脱离网络对 model 匹配 / 安装计划做单元测试 (§1.3：测试不真跑 ollama)。

use serde::{Deserialize, Serialize};

/// Ollama 就绪三态。
///
/// - [`DaemonDown`](OllamaReadiness::DaemonDown) — `/api/tags` 不可达 → 需一键安装/启动。
/// - [`ModelMissing`](OllamaReadiness::ModelMissing) — daemon 在但配置的 chat 模型未下载 → 需一键拉取。
/// - [`Ready`](OllamaReadiness::Ready) — daemon 在 + 模型已就绪。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OllamaReadiness {
    DaemonDown,
    ModelMissing {
        /// 用户配置 (或硬件推荐) 的 chat 模型，例如 `qwen2.5:3b`。
        configured: String,
        /// daemon 上已下载的模型 (原样 tag，含 `:latest`)。
        available: Vec<String>,
    },
    Ready {
        /// 已就绪并将被使用的模型 tag (经 `:latest` 归一匹配后的实际 tag)。
        resolved: String,
    },
}

/// 把 Ollama `:latest` 隐式 tag 归一：`qwen2.5` ⇔ `qwen2.5:latest`。
///
/// Ollama `/api/tags` 永远返回带 tag 的全名 (`name:latest`)，但用户配置 /
/// `ollama pull qwen2.5` 时可省略 `:latest`。匹配时两边都归一到带 tag 形式。
fn normalize_tag(model: &str) -> String {
    if model.contains(':') {
        model.to_string()
    } else {
        format!("{model}:latest")
    }
}

/// 判断 `configured` 模型是否在 `available` 列表里 (经 `:latest` 归一)。
///
/// 同时接受精确匹配与 `:latest` 归一匹配，返回命中到的实际 tag (供 UI 展示)。
pub fn match_model<'a>(configured: &str, available: &'a [String]) -> Option<&'a str> {
    let want = normalize_tag(configured);
    available
        .iter()
        .find(|a| normalize_tag(a) == want)
        .map(|s| s.as_str())
}

/// 三态归一：根据 daemon 是否可达 + 已下载模型列表 + 配置模型，给出 [`OllamaReadiness`]。
///
/// `daemon_reachable=false` → [`DaemonDown`](OllamaReadiness::DaemonDown)
/// (此时 `available` 应为空，但函数对非空也容错)。
pub fn check_readiness(
    daemon_reachable: bool,
    available: &[String],
    configured_model: &str,
) -> OllamaReadiness {
    if !daemon_reachable {
        return OllamaReadiness::DaemonDown;
    }
    match match_model(configured_model, available) {
        Some(resolved) => OllamaReadiness::Ready {
            resolved: resolved.to_string(),
        },
        None => OllamaReadiness::ModelMissing {
            configured: configured_model.to_string(),
            available: available.to_vec(),
        },
    }
}

/// 一键安装 Ollama 的执行方式。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OllamaInstallMethod {
    /// 可脚本化：后台跑该 shell 命令完成安装 (Linux)。
    Script { command: String },
    /// 可下载安装器并静默执行 (Windows OllamaSetup.exe)。
    Installer { download_url: String },
    /// 无法应用内安装 → 引导用户去官网下载 (macOS / 未知平台)。
    ManualDownload { download_url: String },
}

/// 平台对应的 Ollama 一键安装计划。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaInstallPlan {
    /// 规范化平台名 (`linux` / `windows` / `macos` / `unknown`)。
    pub platform: String,
    /// 安装方式。
    pub method: OllamaInstallMethod,
    /// 始终提供官网链接作为兜底 (即便 method 是 Script/Installer)。
    pub homepage: String,
}

const OLLAMA_LINUX_INSTALL: &str = "curl -fsSL https://ollama.com/install.sh | sh";
const OLLAMA_WIN_INSTALLER: &str = "https://ollama.com/download/OllamaSetup.exe";
const OLLAMA_HOMEPAGE: &str = "https://ollama.com/download";

/// 给定 `std::env::consts::OS` 风格的平台串，返回 [`OllamaInstallPlan`]。
///
/// - `linux` → 脚本安装 (`install.sh`)。
/// - `windows` → 下载 `OllamaSetup.exe` 静默安装。
/// - `macos` → manual download (brew/dmg 引导，不在应用内静默装以免破坏用户环境)。
/// - 其他 → manual download 官网。
pub fn install_plan(os: &str) -> OllamaInstallPlan {
    match os {
        "linux" => OllamaInstallPlan {
            platform: "linux".into(),
            method: OllamaInstallMethod::Script {
                command: OLLAMA_LINUX_INSTALL.into(),
            },
            homepage: OLLAMA_HOMEPAGE.into(),
        },
        "windows" => OllamaInstallPlan {
            platform: "windows".into(),
            method: OllamaInstallMethod::Installer {
                download_url: OLLAMA_WIN_INSTALLER.into(),
            },
            homepage: OLLAMA_HOMEPAGE.into(),
        },
        "macos" => OllamaInstallPlan {
            platform: "macos".into(),
            method: OllamaInstallMethod::ManualDownload {
                download_url: OLLAMA_HOMEPAGE.into(),
            },
            homepage: OLLAMA_HOMEPAGE.into(),
        },
        other => OllamaInstallPlan {
            platform: if other.is_empty() { "unknown".into() } else { other.into() },
            method: OllamaInstallMethod::ManualDownload {
                download_url: OLLAMA_HOMEPAGE.into(),
            },
            homepage: OLLAMA_HOMEPAGE.into(),
        },
    }
}

/// 安装计划是否可在应用内自动执行 (Script/Installer) — UI 决定按钮文案
/// ("一键安装" vs "前往下载")。
pub fn is_auto_installable(plan: &OllamaInstallPlan) -> bool {
    matches!(
        plan.method,
        OllamaInstallMethod::Script { .. } | OllamaInstallMethod::Installer { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── normalize_tag / match_model ────────────────────────────────────────

    #[test]
    fn normalize_adds_latest_when_no_tag() {
        assert_eq!(normalize_tag("qwen2.5"), "qwen2.5:latest");
    }

    #[test]
    fn normalize_keeps_existing_tag() {
        assert_eq!(normalize_tag("qwen2.5:3b"), "qwen2.5:3b");
    }

    #[test]
    fn match_exact_tag() {
        let avail = vec!["qwen2.5:3b".to_string(), "bge-m3:latest".to_string()];
        assert_eq!(match_model("qwen2.5:3b", &avail), Some("qwen2.5:3b"));
    }

    #[test]
    fn match_latest_normalization_both_directions() {
        // configured 无 tag, available 带 :latest
        let avail = vec!["qwen2.5:latest".to_string()];
        assert_eq!(match_model("qwen2.5", &avail), Some("qwen2.5:latest"));
        // configured 带 :latest, available 无 tag (理论少见，容错)
        let avail2 = vec!["qwen2.5".to_string()];
        assert_eq!(match_model("qwen2.5:latest", &avail2), Some("qwen2.5"));
    }

    #[test]
    fn match_miss_returns_none() {
        let avail = vec!["llama3.2:1b".to_string()];
        assert_eq!(match_model("qwen2.5:3b", &avail), None);
    }

    #[test]
    fn match_no_partial_prefix_false_positive() {
        // "qwen2" 不应匹配 "qwen2.5:3b"
        let avail = vec!["qwen2.5:3b".to_string()];
        assert_eq!(match_model("qwen2", &avail), None);
    }

    // ── check_readiness 三态 ───────────────────────────────────────────────

    #[test]
    fn readiness_daemon_down() {
        assert_eq!(
            check_readiness(false, &[], "qwen2.5:3b"),
            OllamaReadiness::DaemonDown
        );
    }

    #[test]
    fn readiness_daemon_down_ignores_stale_models() {
        // 即便误传了非空 available，daemon 不可达仍判 DaemonDown
        let avail = vec!["qwen2.5:3b".to_string()];
        assert_eq!(
            check_readiness(false, &avail, "qwen2.5:3b"),
            OllamaReadiness::DaemonDown
        );
    }

    #[test]
    fn readiness_model_missing() {
        let avail = vec!["llama3.2:1b".to_string()];
        match check_readiness(true, &avail, "qwen2.5:3b") {
            OllamaReadiness::ModelMissing { configured, available } => {
                assert_eq!(configured, "qwen2.5:3b");
                assert_eq!(available, avail);
            }
            other => panic!("expected ModelMissing, got {other:?}"),
        }
    }

    #[test]
    fn readiness_model_missing_empty_daemon() {
        // daemon 在但一个模型都没有
        match check_readiness(true, &[], "qwen2.5:3b") {
            OllamaReadiness::ModelMissing { configured, available } => {
                assert_eq!(configured, "qwen2.5:3b");
                assert!(available.is_empty());
            }
            other => panic!("expected ModelMissing, got {other:?}"),
        }
    }

    #[test]
    fn readiness_ready_exact() {
        let avail = vec!["qwen2.5:3b".to_string()];
        assert_eq!(
            check_readiness(true, &avail, "qwen2.5:3b"),
            OllamaReadiness::Ready { resolved: "qwen2.5:3b".into() }
        );
    }

    #[test]
    fn readiness_ready_via_latest() {
        let avail = vec!["bge-m3:latest".to_string()];
        assert_eq!(
            check_readiness(true, &avail, "bge-m3"),
            OllamaReadiness::Ready { resolved: "bge-m3:latest".into() }
        );
    }

    // ── install_plan 平台差异 ──────────────────────────────────────────────

    #[test]
    fn install_plan_linux_is_script() {
        let p = install_plan("linux");
        assert_eq!(p.platform, "linux");
        assert!(matches!(p.method, OllamaInstallMethod::Script { .. }));
        assert!(is_auto_installable(&p));
    }

    #[test]
    fn install_plan_windows_is_installer() {
        let p = install_plan("windows");
        assert_eq!(p.platform, "windows");
        assert!(matches!(p.method, OllamaInstallMethod::Installer { .. }));
        assert!(is_auto_installable(&p));
    }

    #[test]
    fn install_plan_macos_is_manual() {
        let p = install_plan("macos");
        assert_eq!(p.platform, "macos");
        assert!(matches!(p.method, OllamaInstallMethod::ManualDownload { .. }));
        assert!(!is_auto_installable(&p));
    }

    #[test]
    fn install_plan_unknown_is_manual() {
        let p = install_plan("freebsd");
        assert_eq!(p.platform, "freebsd");
        assert!(matches!(p.method, OllamaInstallMethod::ManualDownload { .. }));
        assert!(!is_auto_installable(&p));
        // empty → "unknown"
        assert_eq!(install_plan("").platform, "unknown");
    }

    #[test]
    fn install_plan_always_has_homepage() {
        for os in ["linux", "windows", "macos", "weirdos", ""] {
            assert!(install_plan(os).homepage.starts_with("https://"));
        }
    }
}
