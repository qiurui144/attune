//! CPU 性能数据库（Passmark CPU Mark · multi-thread）+ NPU TOPS 宣称值
//!
//! 数据来源：
//! - Passmark CPU Mark: https://www.cpubenchmark.net/cpu_list.php
//!   选 multi-thread score（"CPU Mark" 列），覆盖 mobile / desktop / server 全谱
//! - NPU TOPS: 厂商官网宣称
//!   - Intel Core Ultra: https://www.intel.com/content/www/us/en/processors/core-ultra.html
//!   - AMD Ryzen AI: https://www.amd.com/en/processors/ryzen-ai
//!   - Apple Silicon Neural Engine: 苹果 keynote 数据
//!   - Snapdragon X: Qualcomm 官网
//!
//! 维护原则：
//! - 数据每季度更新一次（Passmark 实时更新但变化不大）
//! - 加速器加分阈值 NPU ≥ 40 TOPS（Intel Ultra 7 258V / AMD Ryzen AI Max+ / Snapdragon X Elite Gen2 等"Copilot+ PC"级别）
//! - 找不到 model 名 → fuzzy substring match → 仍找不到 → fallback 到 RAM-only 推荐
//!
//! 最低要求阈值（v0.6.0-rc.4 起 Tier 0 不支持启动）:
//! - Passmark < 4000 → Tier 0 不支持
//! - RAM < 4 GB → Tier 0 不支持

/// CPU 数据库条目
#[derive(Debug, Clone, Copy)]
pub struct CpuEntry {
    /// 在 sysinfo cpu_brand() 输出中的子串（用于 fuzzy match）
    pub model_substr: &'static str,
    /// Passmark CPU Mark · multi-thread (越高越快)
    pub passmark: u32,
    /// NPU TOPS（厂商宣称），None = 没 NPU
    pub npu_tops: Option<f32>,
}

/// CPU 性能数据库（持续维护）
///
/// 顺序：先匹配最长 substring 优先 — 例如 "Ryzen 9 7950X" 排在 "Ryzen 9" 前面。
pub const CPU_DB: &[CpuEntry] = &[
    // ─── Apple Silicon ─────────────────────────────────────────────
    // Neural Engine TOPS：M1=11, M2=15.8, M3=18, M4=38（Apple keynote）
    CpuEntry { model_substr: "Apple M4 Max",   passmark: 37450, npu_tops: Some(38.0) },
    CpuEntry { model_substr: "Apple M4 Pro",   passmark: 27620, npu_tops: Some(38.0) },
    CpuEntry { model_substr: "Apple M4",       passmark: 18570, npu_tops: Some(38.0) },
    CpuEntry { model_substr: "Apple M3 Max",   passmark: 33730, npu_tops: Some(18.0) },
    CpuEntry { model_substr: "Apple M3 Pro",   passmark: 23900, npu_tops: Some(18.0) },
    CpuEntry { model_substr: "Apple M3",       passmark: 14870, npu_tops: Some(18.0) },
    CpuEntry { model_substr: "Apple M2 Ultra", passmark: 38020, npu_tops: Some(15.8) },
    CpuEntry { model_substr: "Apple M2 Max",   passmark: 26910, npu_tops: Some(15.8) },
    CpuEntry { model_substr: "Apple M2 Pro",   passmark: 25190, npu_tops: Some(15.8) },
    CpuEntry { model_substr: "Apple M2",       passmark: 15330, npu_tops: Some(15.8) },
    CpuEntry { model_substr: "Apple M1 Ultra", passmark: 36810, npu_tops: Some(11.0) },
    CpuEntry { model_substr: "Apple M1 Max",   passmark: 23110, npu_tops: Some(11.0) },
    CpuEntry { model_substr: "Apple M1 Pro",   passmark: 22050, npu_tops: Some(11.0) },
    CpuEntry { model_substr: "Apple M1",       passmark: 14860, npu_tops: Some(11.0) },

    // ─── Intel Core Ultra (含 NPU) ─────────────────────────────────
    // Lunar Lake / Arrow Lake / Meteor Lake — Copilot+ PC 平台
    CpuEntry { model_substr: "Core Ultra 9 285H",  passmark: 28780, npu_tops: Some(13.0) },
    CpuEntry { model_substr: "Core Ultra 9 285K",  passmark: 53480, npu_tops: Some(13.0) },
    CpuEntry { model_substr: "Core Ultra 7 268V",  passmark: 24180, npu_tops: Some(48.0) },
    CpuEntry { model_substr: "Core Ultra 7 258V",  passmark: 22440, npu_tops: Some(48.0) },
    CpuEntry { model_substr: "Core Ultra 7 165U",  passmark: 16860, npu_tops: Some(11.5) },
    CpuEntry { model_substr: "Core Ultra 7 165H",  passmark: 25080, npu_tops: Some(11.5) },
    CpuEntry { model_substr: "Core Ultra 7 155H",  passmark: 23730, npu_tops: Some(11.5) },
    CpuEntry { model_substr: "Core Ultra 5 235U",  passmark: 16120, npu_tops: Some(13.0) },
    CpuEntry { model_substr: "Core Ultra 5 135U",  passmark: 15780, npu_tops: Some(11.5) },
    CpuEntry { model_substr: "Core Ultra 5 125H",  passmark: 19560, npu_tops: Some(11.5) },

    // ─── Intel Core 14th Gen (Raptor Lake Refresh) ─────────────────
    CpuEntry { model_substr: "i9-14900K",  passmark: 60880, npu_tops: None },
    CpuEntry { model_substr: "i9-14900",   passmark: 49100, npu_tops: None },
    CpuEntry { model_substr: "i7-14700K",  passmark: 50650, npu_tops: None },
    CpuEntry { model_substr: "i7-14700",   passmark: 41370, npu_tops: None },
    CpuEntry { model_substr: "i5-14600K",  passmark: 40690, npu_tops: None },
    CpuEntry { model_substr: "i5-14400",   passmark: 24570, npu_tops: None },

    // ─── Intel Core 13th Gen (Raptor Lake) ─────────────────────────
    CpuEntry { model_substr: "i9-13900K",  passmark: 59920, npu_tops: None },
    CpuEntry { model_substr: "i9-13900HX", passmark: 53200, npu_tops: None },
    CpuEntry { model_substr: "i9-13900H",  passmark: 30490, npu_tops: None },
    CpuEntry { model_substr: "i7-13700K",  passmark: 45290, npu_tops: None },
    CpuEntry { model_substr: "i7-13700H",  passmark: 28310, npu_tops: None },
    CpuEntry { model_substr: "i7-13620H",  passmark: 21330, npu_tops: None },
    CpuEntry { model_substr: "i5-13600K",  passmark: 38230, npu_tops: None },
    CpuEntry { model_substr: "i5-13500H",  passmark: 22810, npu_tops: None },
    CpuEntry { model_substr: "i5-13420H",  passmark: 18570, npu_tops: None },
    CpuEntry { model_substr: "i3-13100",   passmark: 13780, npu_tops: None },

    // ─── Intel Core 12th Gen (Alder Lake) ──────────────────────────
    CpuEntry { model_substr: "i9-12900K",  passmark: 41530, npu_tops: None },
    CpuEntry { model_substr: "i7-12700K",  passmark: 34530, npu_tops: None },
    CpuEntry { model_substr: "i7-1280P",   passmark: 18060, npu_tops: None },
    CpuEntry { model_substr: "i7-1260P",   passmark: 16880, npu_tops: None },
    CpuEntry { model_substr: "i5-12600K",  passmark: 27870, npu_tops: None },
    CpuEntry { model_substr: "i5-12500H",  passmark: 22560, npu_tops: None },
    CpuEntry { model_substr: "i5-1240P",   passmark: 15570, npu_tops: None },
    CpuEntry { model_substr: "i5-12400",   passmark: 19450, npu_tops: None },
    CpuEntry { model_substr: "i3-1215U",   passmark: 8220,  npu_tops: None },

    // ─── Intel Core 11th Gen / 老款 ────────────────────────────────
    CpuEntry { model_substr: "i7-11700K",  passmark: 26450, npu_tops: None },
    CpuEntry { model_substr: "i7-1185G7",  passmark: 11210, npu_tops: None },
    CpuEntry { model_substr: "i7-1165G7",  passmark: 10310, npu_tops: None },
    CpuEntry { model_substr: "i5-11400",   passmark: 17440, npu_tops: None },
    CpuEntry { model_substr: "i5-1135G7",  passmark: 9970,  npu_tops: None },
    CpuEntry { model_substr: "i7-10750H",  passmark: 12690, npu_tops: None },
    CpuEntry { model_substr: "i5-10210U",  passmark: 6530,  npu_tops: None },
    CpuEntry { model_substr: "i7-8750H",   passmark: 11420, npu_tops: None },

    // ─── Intel Atom / Celeron / Pentium (低端) ─────────────────────
    CpuEntry { model_substr: "i3-N305",          passmark: 7980,  npu_tops: None },
    CpuEntry { model_substr: "Celeron N5105",    passmark: 3570,  npu_tops: None },  // Tier 0
    CpuEntry { model_substr: "Celeron N5095",    passmark: 3160,  npu_tops: None },  // Tier 0
    CpuEntry { model_substr: "Celeron N4500",    passmark: 1820,  npu_tops: None },  // Tier 0
    CpuEntry { model_substr: "Celeron J4125",    passmark: 2250,  npu_tops: None },  // Tier 0
    CpuEntry { model_substr: "Celeron N100",     passmark: 5380,  npu_tops: None },
    CpuEntry { model_substr: "Atom x6212RE",     passmark: 1060,  npu_tops: None },  // Tier 0

    // ─── AMD Ryzen 桌面 ────────────────────────────────────────────
    CpuEntry { model_substr: "Ryzen 9 9950X",    passmark: 71500, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 9 9900X",    passmark: 56450, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 7 9700X",    passmark: 39520, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 5 9600X",    passmark: 32510, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 9 7950X",    passmark: 63030, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 9 7900X",    passmark: 52270, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 7 7800X3D",  passmark: 35710, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 7 7700X",    passmark: 35020, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 5 7600X",    passmark: 28430, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 5 7600",     passmark: 27090, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 9 5950X",    passmark: 45960, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 9 5900X",    passmark: 39360, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 7 5800X3D",  passmark: 28370, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 7 5800X",    passmark: 28010, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 7 5700X",    passmark: 26200, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 5 5600X",    passmark: 22020, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 5 5600",     passmark: 22020, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 5 5500",     passmark: 18980, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 5 3600",     passmark: 17880, npu_tops: None },

    // ─── AMD Ryzen AI 移动 (含 XDNA NPU) ──────────────────────────
    // Strix Halo / Strix Point / Hawk Point: NPU TOPS 16-50
    CpuEntry { model_substr: "Ryzen AI Max+ 395", passmark: 38470, npu_tops: Some(50.0) },
    CpuEntry { model_substr: "Ryzen AI Max 390",  passmark: 35580, npu_tops: Some(50.0) },
    CpuEntry { model_substr: "Ryzen AI 9 HX 375", passmark: 30650, npu_tops: Some(50.0) },
    CpuEntry { model_substr: "Ryzen AI 9 HX 370", passmark: 30270, npu_tops: Some(50.0) },
    CpuEntry { model_substr: "Ryzen AI 9 365",    passmark: 27090, npu_tops: Some(50.0) },
    CpuEntry { model_substr: "Ryzen 9 8945HS",    passmark: 27270, npu_tops: Some(16.0) },
    CpuEntry { model_substr: "Ryzen 7 8845HS",    passmark: 24650, npu_tops: Some(16.0) },
    CpuEntry { model_substr: "Ryzen 5 8645HS",    passmark: 21550, npu_tops: Some(16.0) },

    // ─── AMD Ryzen 移动（普通） ────────────────────────────────────
    CpuEntry { model_substr: "Ryzen 9 7945HX",    passmark: 51450, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 9 7940HS",    passmark: 28210, npu_tops: Some(16.0) },
    CpuEntry { model_substr: "Ryzen 7 7840HS",    passmark: 28780, npu_tops: Some(16.0) },
    CpuEntry { model_substr: "Ryzen 5 7640HS",    passmark: 22070, npu_tops: Some(16.0) },
    CpuEntry { model_substr: "Ryzen 7 6800H",     passmark: 22510, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 7 5800H",     passmark: 21260, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 5 5600H",     passmark: 17070, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 7 5800U",     passmark: 16020, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 7 5700U",     passmark: 15310, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 5 5500U",     passmark: 14070, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 5 4500U",     passmark: 11700, npu_tops: None },
    CpuEntry { model_substr: "Ryzen 3 5300U",     passmark: 8830,  npu_tops: None },
    CpuEntry { model_substr: "Ryzen 3 3250U",     passmark: 3840,  npu_tops: None },  // Tier 0

    // ─── Snapdragon (Windows on ARM) ───────────────────────────────
    CpuEntry { model_substr: "Snapdragon X Elite", passmark: 22920, npu_tops: Some(45.0) },
    CpuEntry { model_substr: "Snapdragon X Plus",  passmark: 18650, npu_tops: Some(45.0) },
    CpuEntry { model_substr: "Snapdragon 8cx Gen3", passmark: 7160,  npu_tops: None },

    // ─── ARM 嵌入式 (RPi / RK3588 / SpacemiT) ─────────────────────
    // 注：SpacemiT X100 (K3 一体机) Passmark 数据不公开，按 8 核 RVA22 估算
    CpuEntry { model_substr: "Cortex-A78AE",     passmark: 6800,  npu_tops: None },
    CpuEntry { model_substr: "Cortex-A76",       passmark: 4520,  npu_tops: None },  // RK3588 4 大核
    CpuEntry { model_substr: "Cortex-A72",       passmark: 1760,  npu_tops: None },  // Tier 0 (RPi 4)
    CpuEntry { model_substr: "Cortex-A53",       passmark: 980,   npu_tops: None },  // Tier 0
    CpuEntry { model_substr: "RK3588",           passmark: 6300,  npu_tops: Some(6.0) },  // 6 TOPS NPU
    CpuEntry { model_substr: "X100",             passmark: 5500,  npu_tops: Some(2.0) },  // K3 SpacemiT 估算
];

/// Normalize CPU model 字符串，便于 substring 匹配：
/// - 去除商标符号 (R), (TM), (C), ®, ™
/// - 去除多余空格 / 制表符
/// - 转小写
///
/// 例：
///   "Intel(R) Core(TM) i9-14900K CPU @ 3.00GHz" → "intel core i9-14900k cpu @ 3.00ghz"
///   "Intel(R) Celeron(R) N4500" → "intel celeron n4500"
fn normalize(model: &str) -> String {
    model
        .replace("(R)", "")
        .replace("(TM)", "")
        .replace("(C)", "")
        .replace('®', "")
        .replace('™', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Fuzzy substring match：在 cpu_model 中查 DB 中任一 substring，返回最长匹配的条目。
///
/// 例：cpu_model = "Intel(R) Core(TM) i9-14900K CPU @ 3.00GHz"
///     normalize → "intel core i9-14900k cpu @ 3.00ghz"
///     匹配 "i9-14900K" → CpuEntry { passmark: 60880, ... }
pub fn lookup(cpu_model: &str) -> Option<&'static CpuEntry> {
    let m = normalize(cpu_model);
    let mut best: Option<&'static CpuEntry> = None;
    let mut best_len = 0;
    for entry in CPU_DB {
        let s = normalize(entry.model_substr);
        if m.contains(&s) && s.len() > best_len {
            best = Some(entry);
            best_len = s.len();
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_intel_i9() {
        // sysinfo 输出格式
        let raw = "Intel(R) Core(TM) i9-14900K CPU @ 3.00GHz";
        let entry = lookup(raw).expect("hit i9-14900K");
        assert_eq!(entry.passmark, 60880);
        assert_eq!(entry.npu_tops, None);
    }

    #[test]
    fn lookup_intel_ultra_with_npu() {
        let raw = "Intel(R) Core(TM) Ultra 7 258V";
        let entry = lookup(raw).expect("hit Core Ultra 7 258V");
        assert!(entry.passmark >= 22000);
        assert_eq!(entry.npu_tops, Some(48.0));
    }

    #[test]
    fn lookup_apple_m3() {
        let entry = lookup("Apple M3").expect("hit M3");
        assert_eq!(entry.passmark, 14870);
        assert_eq!(entry.npu_tops, Some(18.0));
    }

    #[test]
    fn lookup_ryzen_ai_npu() {
        let raw = "AMD Ryzen AI 9 365 w/ Radeon 880M";
        let entry = lookup(raw).expect("hit Ryzen AI 9 365");
        assert_eq!(entry.npu_tops, Some(50.0));
    }

    #[test]
    fn lookup_celeron_tier0() {
        let raw = "Intel(R) Celeron(R) N4500 @ 1.10GHz";
        let entry = lookup(raw).expect("hit");
        assert!(entry.passmark < 4000, "Celeron N4500 should be Tier 0");
    }

    #[test]
    fn lookup_unknown_returns_none() {
        let raw = "Some Unknown CPU Model 9999";
        assert!(lookup(raw).is_none());
    }

    #[test]
    fn lookup_prefers_longest_match() {
        // "Apple M2" 和 "Apple M2 Pro" 都在 DB；"Apple M2 Pro" 应优先
        let raw = "Apple M2 Pro";
        let entry = lookup(raw).expect("hit");
        assert_eq!(entry.passmark, 25190, "should match M2 Pro not M2");
    }

    #[test]
    fn db_consistency_checks() {
        // 数据健全性：每个条目 substring 非空、passmark 合理
        for entry in CPU_DB {
            assert!(!entry.model_substr.is_empty());
            assert!(
                entry.passmark > 0 && entry.passmark < 200_000,
                "{} passmark out of range: {}",
                entry.model_substr,
                entry.passmark
            );
            if let Some(tops) = entry.npu_tops {
                assert!(
                    (1.0..200.0).contains(&tops),
                    "{} TOPS out of range: {}",
                    entry.model_substr,
                    tops
                );
            }
        }
    }
}
