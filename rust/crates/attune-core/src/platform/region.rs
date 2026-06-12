//! 区域检测 + 模型下载源选择
//!
//! 用户决策（2026-04-27）："中国区域默认走代理地址" — 启动时根据 timezone + locale
//! 自动检测，给国内用户选国内源，避免 HuggingFace 直连慢/失败。
//!
//! 2026-06-12 修正(§6.3 实测拍板 CN→ModelScope/海外→HF):旧 `hf-mirror.com` 在 CN 已死
//! (连不上/卡死),改默认 `modelscope.cn`(实测唯一活源,4MB/s,HF-resolve 兼容)。
//!
//! ⚠️ per-model 覆盖差异(ModelScope 非全镜像):
//!   - ✅ embedding/reranker(Xenova ONNX)— ModelScope 有 `Xenova/bge-m3` /
//!     `Xenova/bge-reranker-base`(实测 206)
//!   - ❌ ASR(`ggerganov/whisper.cpp`)/ OCR(`SWHL/RapidOCR`)— ModelScope 无(404)
//!
//! 注:本 `Region::hf_endpoint()` 现仅作启动期 `HF_ENDPOINT` env 的**静态默认**(state.rs)
//! + 显式覆盖逃生门。模型**下载**已升级到 S8 动态多源选择(`infer::model_source`:候选注册表
//! company-mirror > ModelScope > hf-mirror > HF + 健康探测 + failover),对 ModelScope
//! 无覆盖的 whisper/PP-OCR 自动跳过改走 company-mirror/HF,不再 404 degrade
//! (spec docs/superpowers/specs/2026-06-11-modelstack-lifecycle.md §12;company-mirror
//! host 归 cloud R2.E)。
//!
//! 区域分类：
//! - China: timezone Asia/{Shanghai/Chongqing/Urumqi/Harbin/Hong_Kong/Taipei/Macau}
//!   OR locale zh-CN / zh_CN / zh-HK / zh-TW
//! - International: 其他

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Region {
    China,
    International,
}

impl Region {
    /// HuggingFace 兼容模型下载 endpoint(hf-hub crate 读 `HF_ENDPOINT`)。
    /// CN → ModelScope(HF-resolve 兼容,实测唯一活源);海外 → HF 官方。
    /// per-model 覆盖差异见模块 doc(embedding/reranker 有;ASR/OCR 无 → degrade)。
    pub fn hf_endpoint(self) -> &'static str {
        match self {
            // hf-hub 拼 `{endpoint}/{repo}/resolve/{rev}/{file}`;ModelScope 的 HF-resolve
            // 兼容路径在 `/models/` 下,故 endpoint 含 `/models` 才能命中。
            Region::China => "https://modelscope.cn/models",
            Region::International => "https://huggingface.co",
        }
    }

    /// GitHub release 文件下载代理（whisper-cli binary / tesseract trained data 等）
    pub fn github_proxy(self) -> Option<&'static str> {
        match self {
            Region::China => Some("https://ghproxy.com/"),
            Region::International => None, // 直连
        }
    }

    /// tesseract 训练数据 mirror
    pub fn tesseract_data_base(self) -> &'static str {
        match self {
            // gitee mirror 速度好；备选 ghproxy
            Region::China => "https://gitee.com/mirrors/tessdata_fast/raw/main",
            Region::International => "https://github.com/tesseract-ocr/tessdata_fast/raw/main",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Region::China => "China (modelscope.cn)",
            Region::International => "International (huggingface.co)",
        }
    }
}

/// 启动时自动检测区域：timezone + locale 任一命中中国信号 → China
///
/// 失败 fallback：International（保守默认，海外用户更多）
pub fn detect_region() -> Region {
    if is_china_by_timezone() || is_china_by_locale() {
        Region::China
    } else {
        Region::International
    }
}

fn is_china_by_timezone() -> bool {
    // /etc/timezone (Linux) / Tauri 跨平台 timezone 检测稍复杂；最务实方法：
    // 读 TZ 环境变量 + /etc/timezone 文件 + tzdata symlink 解析。
    let tz = read_timezone();
    matches!(
        tz.as_deref(),
        Some("Asia/Shanghai")
            | Some("Asia/Chongqing")
            | Some("Asia/Urumqi")
            | Some("Asia/Harbin")
            | Some("Asia/Hong_Kong")
            | Some("Asia/Taipei")
            | Some("Asia/Macau")
    )
}

fn read_timezone() -> Option<String> {
    // 优先 TZ 环境变量
    if let Ok(tz) = std::env::var("TZ") {
        if !tz.is_empty() {
            return Some(tz);
        }
    }
    // Linux: /etc/timezone
    #[cfg(target_os = "linux")]
    {
        if let Ok(s) = std::fs::read_to_string("/etc/timezone") {
            let trimmed = s.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
        // /etc/localtime symlink 指向 /usr/share/zoneinfo/Asia/Shanghai
        if let Ok(link) = std::fs::read_link("/etc/localtime") {
            if let Some(s) = link.to_str() {
                if let Some(idx) = s.find("zoneinfo/") {
                    return Some(s[idx + 9..].to_string());
                }
            }
        }
    }
    None
}

fn is_china_by_locale() -> bool {
    // LANG / LC_ALL / LC_CTYPE 任一含 zh
    for var in &["LANG", "LC_ALL", "LC_CTYPE", "LANGUAGE"] {
        if let Ok(v) = std::env::var(var) {
            let lower = v.to_lowercase();
            if lower.starts_with("zh-cn")
                || lower.starts_with("zh_cn")
                || lower.starts_with("zh-hk")
                || lower.starts_with("zh_hk")
                || lower.starts_with("zh-tw")
                || lower.starts_with("zh_tw")
                || lower.starts_with("zh.")
                || lower == "zh"
            {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_endpoints_differ() {
        assert_ne!(
            Region::China.hf_endpoint(),
            Region::International.hf_endpoint()
        );
        // S2: CN 默认源改 ModelScope(实测唯一活源;旧 hf-mirror 已死),海外保持 HF 官方。
        assert!(
            Region::China.hf_endpoint().contains("modelscope"),
            "CN endpoint must be ModelScope, got {}",
            Region::China.hf_endpoint()
        );
        assert!(!Region::China.hf_endpoint().contains("hf-mirror"));
        assert!(Region::International.hf_endpoint().contains("huggingface.co"));
    }

    #[test]
    fn china_has_github_proxy() {
        assert!(Region::China.github_proxy().is_some());
        assert!(Region::International.github_proxy().is_none());
    }

    #[test]
    fn detect_returns_some_region() {
        // 无论何环境都应返回某 region 不 panic
        let _ = detect_region();
    }
}
