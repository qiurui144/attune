//! CPU 性能档位 (Tier) 分类 + 模型推荐
//!
//! 5 档分级（per docs/superpowers/specs 2026-04-27 用户决策）：
//! - Tier 0 不支持: Passmark < 4000 OR RAM < 4GB → 启动时弹窗 + 退出
//! - Tier 1 低端: Passmark 4-9K, RAM 4-8GB
//! - Tier 2 中端: 9-18K, 8-16GB
//! - Tier 3 高端: 18-35K, 16-32GB
//! - Tier 4 旗舰: > 35K, ≥32GB
//!
//! 加速器加分（NPU ≥ 40 TOPS / 独立 GPU）= +1 tier。
//!
//! 模型推荐：embedding / reranker / asr 各按 tier 选合适大小，全部走 HuggingFace
//! 自动下载（per "全自动下载" 用户决策）。

use super::cpu_db;
use super::HardwareProfile;
use serde::{Deserialize, Serialize};

/// 5 档硬件性能 tier
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Passmark < 4000 OR RAM < 4GB — 不支持运行 attune
    Unsupported,
    /// Passmark 4-9K, RAM 4-8GB
    Low,
    /// Passmark 9-18K, RAM 8-16GB
    Mid,
    /// Passmark 18-35K, RAM 16-32GB
    High,
    /// Passmark > 35K, RAM ≥ 32GB
    Flagship,
}

impl Tier {
    /// 按 RAM-only fallback（CPU model 在 DB 中找不到时）
    pub fn from_ram_gb(ram_gb: u64) -> Self {
        match ram_gb {
            0..=3 => Tier::Unsupported,
            4..=7 => Tier::Low,
            8..=15 => Tier::Mid,
            16..=31 => Tier::High,
            _ => Tier::Flagship,
        }
    }

    /// 按 Passmark CPU Mark 分档
    pub fn from_passmark(passmark: u32) -> Self {
        match passmark {
            0..=3999 => Tier::Unsupported,
            4000..=8999 => Tier::Low,
            9000..=17999 => Tier::Mid,
            18000..=34999 => Tier::High,
            _ => Tier::Flagship,
        }
    }

    /// 加速器加分：NPU ≥ 40 TOPS 或独立 GPU = +1 tier
    pub fn bump_for_accelerator(self) -> Self {
        match self {
            Tier::Unsupported => Tier::Unsupported, // 不能从不支持升级（CPU 太弱即便有加速器也不行）
            Tier::Low => Tier::Mid,
            Tier::Mid => Tier::High,
            Tier::High => Tier::Flagship,
            Tier::Flagship => Tier::Flagship,
        }
    }

    /// 受 RAM 限制降档（CPU 高但 RAM 不够）
    pub fn cap_by_ram(self, ram_gb: u64) -> Self {
        let ram_tier = Self::from_ram_gb(ram_gb);
        std::cmp::min(self, ram_tier)
    }

    /// 是否能运行 attune
    pub fn is_supported(self) -> bool {
        self != Tier::Unsupported
    }

    /// 用户面 label
    pub fn label(self) -> &'static str {
        match self {
            Tier::Unsupported => "unsupported",
            Tier::Low => "low",
            Tier::Mid => "mid",
            Tier::High => "high",
            Tier::Flagship => "flagship",
        }
    }
}

/// 模型推荐（按 tier 选合适大小）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRecommendation {
    pub tier: Tier,
    /// HuggingFace repo id (如 "BAAI/bge-base-zh-v1.5")
    pub embedding_repo: &'static str,
    pub embedding_size_mb: u32,
    pub reranker_repo: &'static str,
    pub reranker_size_mb: u32,
    /// whisper.cpp ggml 模型名 (如 "ggml-small-q8_0.bin")
    pub asr_ggml: &'static str,
    pub asr_size_mb: u32,
}

impl ModelRecommendation {
    pub fn for_tier(tier: Tier) -> Option<Self> {
        match tier {
            Tier::Unsupported => None,
            Tier::Low => Some(Self {
                tier,
                embedding_repo: "BAAI/bge-small-zh-v1.5",
                embedding_size_mb: 100,
                reranker_repo: "Xenova/bge-reranker-base",
                reranker_size_mb: 50,
                asr_ggml: "ggml-tiny-q8_0.bin",
                asr_size_mb: 75,
            }),
            Tier::Mid => Some(Self {
                tier,
                embedding_repo: "BAAI/bge-base-zh-v1.5",
                embedding_size_mb: 400,
                reranker_repo: "Xenova/bge-reranker-base",
                reranker_size_mb: 50,
                asr_ggml: "ggml-base-q8_0.bin",
                asr_size_mb: 150,
            }),
            Tier::High => Some(Self {
                tier,
                embedding_repo: "BAAI/bge-m3",
                embedding_size_mb: 1200,
                reranker_repo: "BAAI/bge-reranker-v2-m3",
                reranker_size_mb: 570,
                asr_ggml: "ggml-small-q8_0.bin",
                asr_size_mb: 487,
            }),
            Tier::Flagship => Some(Self {
                tier,
                embedding_repo: "BAAI/bge-m3",
                embedding_size_mb: 1200,
                reranker_repo: "BAAI/bge-reranker-v2-m3",
                reranker_size_mb: 570,
                asr_ggml: "ggml-medium-q5_0.bin",
                asr_size_mb: 530,
            }),
        }
    }

    /// 总下载量（MB）
    pub fn total_download_mb(&self) -> u32 {
        self.embedding_size_mb + self.reranker_size_mb + self.asr_size_mb
    }
}

/// 主分类入口：根据硬件返 Tier
pub fn classify_hardware(hw: &HardwareProfile) -> Tier {
    const GB: u64 = 1024 * 1024 * 1024;
    let ram_gb = hw.total_ram_bytes / GB;
    let cpu_entry = cpu_db::lookup(&hw.cpu_model);

    // 1. 基础 tier：CPU Passmark > RAM fallback
    let mut tier = match cpu_entry {
        Some(entry) => Tier::from_passmark(entry.passmark),
        None => Tier::from_ram_gb(ram_gb),
    };

    // 2. 加速器加分（NPU ≥ 40 TOPS 或独立 GPU）
    let has_strong_npu = cpu_entry
        .and_then(|e| e.npu_tops)
        .map(|tops| tops >= 40.0)
        .unwrap_or(false);
    let has_discrete_gpu = hw.has_nvidia_gpu || hw.has_amd_gpu;
    if has_strong_npu || has_discrete_gpu {
        tier = tier.bump_for_accelerator();
    }

    // 3. RAM 反限制（CPU 高但 RAM 不够 → 降档）
    tier = tier.cap_by_ram(ram_gb);

    tier
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_hw(cpu: &str, ram_gb: u64, gpu: bool) -> HardwareProfile {
        let mut hw = HardwareProfile::default();
        hw.cpu_model = cpu.to_string();
        hw.total_ram_bytes = ram_gb * 1024 * 1024 * 1024;
        hw.has_nvidia_gpu = gpu;
        hw
    }

    #[test]
    fn tier_unsupported_celeron_n4500() {
        let hw = mk_hw("Intel(R) Celeron(R) N4500 @ 1.10GHz", 4, false);
        assert_eq!(classify_hardware(&hw), Tier::Unsupported);
    }

    #[test]
    fn tier_unsupported_low_ram() {
        let hw = mk_hw("Intel(R) Core(TM) i9-14900K", 2, false);
        // RAM 太低反限制到 Unsupported（即便 CPU flagship）
        assert_eq!(classify_hardware(&hw), Tier::Unsupported);
    }

    #[test]
    fn tier_low_ryzen3() {
        let hw = mk_hw("AMD Ryzen 3 5300U", 8, false);
        // Passmark 8830 → Low；RAM 8GB → Mid 但 cap 不动
        assert_eq!(classify_hardware(&hw), Tier::Low);
    }

    #[test]
    fn tier_mid_intel_i5_1240p() {
        let hw = mk_hw("Intel(R) Core(TM) i5-1240P", 16, false);
        // Passmark 15570 → Mid
        assert_eq!(classify_hardware(&hw), Tier::Mid);
    }

    #[test]
    fn tier_high_apple_m1() {
        let hw = mk_hw("Apple M1", 16, false);
        // Passmark 14860 → Mid；M1 NPU 11 TOPS < 40 不加分
        assert_eq!(classify_hardware(&hw), Tier::Mid);
    }

    #[test]
    fn tier_flagship_apple_m3_max() {
        let hw = mk_hw("Apple M3 Max", 64, false);
        // Passmark 33730 → High；NPU 18 TOPS < 40 不加分
        assert_eq!(classify_hardware(&hw), Tier::High);
    }

    #[test]
    fn tier_bump_with_strong_npu() {
        let hw = mk_hw("Intel(R) Core(TM) Ultra 7 258V", 32, false);
        // Passmark 22440 → High；NPU 48 TOPS ≥ 40 → bump → Flagship
        assert_eq!(classify_hardware(&hw), Tier::Flagship);
    }

    #[test]
    fn tier_bump_with_discrete_gpu() {
        let hw = mk_hw("Intel(R) Core(TM) i5-12400", 16, true);
        // Passmark 19450 → High；GPU 加分 → Flagship
        // 但 RAM 16GB cap 到 High（cap_by_ram 反限制）
        assert_eq!(classify_hardware(&hw), Tier::High);
    }

    #[test]
    fn tier_unknown_cpu_falls_back_to_ram() {
        let hw = mk_hw("Some Unknown Future CPU XYZ", 16, false);
        // CPU 不在 DB → RAM fallback：16GB → High（边界含右）
        assert_eq!(classify_hardware(&hw), Tier::High);
    }

    #[test]
    fn tier_unknown_cpu_8gb_falls_to_mid() {
        let hw = mk_hw("Some Unknown Future CPU XYZ", 8, false);
        assert_eq!(classify_hardware(&hw), Tier::Mid);
    }

    #[test]
    fn model_recommendation_unsupported_returns_none() {
        assert!(ModelRecommendation::for_tier(Tier::Unsupported).is_none());
    }

    #[test]
    fn model_recommendation_high_uses_bge_m3() {
        let rec = ModelRecommendation::for_tier(Tier::High).unwrap();
        assert_eq!(rec.embedding_repo, "BAAI/bge-m3");
        assert_eq!(rec.reranker_repo, "BAAI/bge-reranker-v2-m3");
        assert!(rec.total_download_mb() >= 2000);
    }

    #[test]
    fn model_recommendation_low_uses_small_models() {
        let rec = ModelRecommendation::for_tier(Tier::Low).unwrap();
        assert!(rec.embedding_repo.contains("small"));
        assert!(rec.asr_ggml.contains("tiny"));
        // Total 应小（<300MB），适合低端机
        assert!(rec.total_download_mb() < 300);
    }
}
