// 三档预设：Conservative / Balanced / Aggressive
// 每档在每个 TaskKind 上有特定 Budget。

use serde::{Deserialize, Serialize};

use super::budget::{Budget, IoPriority};

/// 系统影响档位。Balanced 为默认；Aggressive 适合插电桌面；Conservative 适合电池笔记本。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Profile {
    Conservative,
    #[default]
    Balanced,
    Aggressive,
}

/// 后台任务种类 — 决定 governor 如何分配预算。
///
/// 新增任务类型时同步更新 [`Profile::budget_for`]。
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskKind {
    EmbeddingQueue,
    SkillEvolution,
    FileScanner,
    WebDavSync,
    PatentScanner,
    BrowserSearch,
    AiAnnotator,
    /// G1：Chrome 扩展通用浏览状态摄取
    BrowseSignalIngest,
    /// G2：高 engagement 自动 bookmark
    AutoBookmark,
    /// A1：周期性 memory consolidation
    MemoryConsolidation,
}

impl TaskKind {
    /// 给前端 / `attune --diag` 用的稳定字符串 id（不要直接 Debug — 需保持向后兼容）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EmbeddingQueue => "embedding_queue",
            Self::SkillEvolution => "skill_evolution",
            Self::FileScanner => "file_scanner",
            Self::WebDavSync => "webdav_sync",
            Self::PatentScanner => "patent_scanner",
            Self::BrowserSearch => "browser_search",
            Self::AiAnnotator => "ai_annotator",
            Self::BrowseSignalIngest => "browse_signal_ingest",
            Self::AutoBookmark => "auto_bookmark",
            Self::MemoryConsolidation => "memory_consolidation",
        }
    }
}

const MB: u64 = 1024 * 1024;

impl Profile {
    /// 根据档位 + 任务种类返回预算。
    /// 数值参考 spec §6，正式发版前要 baseline 后再微调。
    pub fn budget_for(self, task: TaskKind) -> Budget {
        use Profile::*;
        use TaskKind::*;

        // 用 (Profile, TaskKind) 元组列出全部组合，保持表与 spec 完全一致。
        match (self, task) {
            // EmbeddingQueue
            (Conservative, EmbeddingQueue) => Budget {
                cpu_pct_max: 15.0,
                ram_bytes_max: 512 * MB,
                io_priority: IoPriority::Idle,
                throttle_on_exceed_ms: 2000,
                llm_calls_per_hour: None,
            },
            (Balanced, EmbeddingQueue) => Budget {
                cpu_pct_max: 25.0,
                ram_bytes_max: 1024 * MB,
                io_priority: IoPriority::BestEffort,
                throttle_on_exceed_ms: 1000,
                llm_calls_per_hour: None,
            },
            (Aggressive, EmbeddingQueue) => Budget {
                cpu_pct_max: 60.0,
                ram_bytes_max: 2048 * MB,
                io_priority: IoPriority::BestEffort,
                throttle_on_exceed_ms: 100,
                llm_calls_per_hour: None,
            },

            // SkillEvolution（含 LLM 限速）
            (Conservative, SkillEvolution) => Budget {
                cpu_pct_max: 10.0,
                ram_bytes_max: 256 * MB,
                io_priority: IoPriority::Idle,
                throttle_on_exceed_ms: 5000,
                llm_calls_per_hour: Some(5),
            },
            (Balanced, SkillEvolution) => Budget {
                cpu_pct_max: 20.0,
                ram_bytes_max: 512 * MB,
                io_priority: IoPriority::Idle,
                throttle_on_exceed_ms: 2000,
                llm_calls_per_hour: Some(10),
            },
            (Aggressive, SkillEvolution) => Budget {
                cpu_pct_max: 40.0,
                ram_bytes_max: 1024 * MB,
                io_priority: IoPriority::BestEffort,
                throttle_on_exceed_ms: 500,
                llm_calls_per_hour: Some(30),
            },

            // FileScanner
            (Conservative, FileScanner) => Budget {
                cpu_pct_max: 10.0,
                ram_bytes_max: 256 * MB,
                io_priority: IoPriority::Idle,
                throttle_on_exceed_ms: 1000,
                llm_calls_per_hour: None,
            },
            (Balanced, FileScanner) => Budget {
                cpu_pct_max: 20.0,
                ram_bytes_max: 512 * MB,
                io_priority: IoPriority::Idle,
                throttle_on_exceed_ms: 500,
                llm_calls_per_hour: None,
            },
            (Aggressive, FileScanner) => Budget {
                cpu_pct_max: 50.0,
                ram_bytes_max: 1024 * MB,
                io_priority: IoPriority::BestEffort,
                throttle_on_exceed_ms: 100,
                llm_calls_per_hour: None,
            },

            // WebDavSync
            (Conservative, WebDavSync) => Budget {
                cpu_pct_max: 10.0,
                ram_bytes_max: 128 * MB,
                io_priority: IoPriority::Idle,
                throttle_on_exceed_ms: 5000,
                llm_calls_per_hour: None,
            },
            (Balanced, WebDavSync) => Budget {
                cpu_pct_max: 15.0,
                ram_bytes_max: 256 * MB,
                io_priority: IoPriority::Idle,
                throttle_on_exceed_ms: 2000,
                llm_calls_per_hour: None,
            },
            (Aggressive, WebDavSync) => Budget {
                cpu_pct_max: 30.0,
                ram_bytes_max: 512 * MB,
                io_priority: IoPriority::BestEffort,
                throttle_on_exceed_ms: 500,
                llm_calls_per_hour: None,
            },

            // PatentScanner — 与 FileScanner 同档
            (p, PatentScanner) => p.budget_for(FileScanner),

            // BrowserSearch — 浏览器自动化天然占用大
            (Conservative, BrowserSearch) => Budget {
                cpu_pct_max: 30.0,
                ram_bytes_max: 1024 * MB,
                io_priority: IoPriority::BestEffort,
                throttle_on_exceed_ms: 1000,
                llm_calls_per_hour: None,
            },
            (Balanced, BrowserSearch) => Budget {
                cpu_pct_max: 50.0,
                ram_bytes_max: 1536 * MB,
                io_priority: IoPriority::BestEffort,
                throttle_on_exceed_ms: 500,
                llm_calls_per_hour: None,
            },
            (Aggressive, BrowserSearch) => Budget {
                cpu_pct_max: 80.0,
                ram_bytes_max: 2048 * MB,
                io_priority: IoPriority::BestEffort,
                throttle_on_exceed_ms: 100,
                llm_calls_per_hour: None,
            },

            // AiAnnotator
            (Conservative, AiAnnotator) => Budget {
                cpu_pct_max: 10.0,
                ram_bytes_max: 256 * MB,
                io_priority: IoPriority::Idle,
                throttle_on_exceed_ms: 3000,
                llm_calls_per_hour: None,
            },
            (Balanced, AiAnnotator) => Budget {
                cpu_pct_max: 20.0,
                ram_bytes_max: 512 * MB,
                io_priority: IoPriority::Idle,
                throttle_on_exceed_ms: 1000,
                llm_calls_per_hour: None,
            },
            (Aggressive, AiAnnotator) => Budget {
                cpu_pct_max: 50.0,
                ram_bytes_max: 1024 * MB,
                io_priority: IoPriority::BestEffort,
                throttle_on_exceed_ms: 200,
                llm_calls_per_hour: None,
            },

            // BrowseSignalIngest (G1) — 极轻量
            (Conservative, BrowseSignalIngest) => Budget {
                cpu_pct_max: 5.0,
                ram_bytes_max: 64 * MB,
                io_priority: IoPriority::Idle,
                throttle_on_exceed_ms: 5000,
                llm_calls_per_hour: None,
            },
            (Balanced, BrowseSignalIngest) => Budget {
                cpu_pct_max: 10.0,
                ram_bytes_max: 128 * MB,
                io_priority: IoPriority::Idle,
                throttle_on_exceed_ms: 2000,
                llm_calls_per_hour: None,
            },
            (Aggressive, BrowseSignalIngest) => Budget {
                cpu_pct_max: 20.0,
                ram_bytes_max: 256 * MB,
                io_priority: IoPriority::Idle,
                throttle_on_exceed_ms: 500,
                llm_calls_per_hour: None,
            },

            // AutoBookmark (G2)
            (Conservative, AutoBookmark) => Budget {
                cpu_pct_max: 10.0,
                ram_bytes_max: 256 * MB,
                io_priority: IoPriority::Idle,
                throttle_on_exceed_ms: 5000,
                llm_calls_per_hour: None,
            },
            (Balanced, AutoBookmark) => Budget {
                cpu_pct_max: 20.0,
                ram_bytes_max: 512 * MB,
                io_priority: IoPriority::Idle,
                throttle_on_exceed_ms: 2000,
                llm_calls_per_hour: None,
            },
            (Aggressive, AutoBookmark) => Budget {
                cpu_pct_max: 40.0,
                ram_bytes_max: 1024 * MB,
                io_priority: IoPriority::BestEffort,
                throttle_on_exceed_ms: 500,
                llm_calls_per_hour: None,
            },

            // MemoryConsolidation (A1) — 含 LLM 限速
            (Conservative, MemoryConsolidation) => Budget {
                cpu_pct_max: 15.0,
                ram_bytes_max: 512 * MB,
                io_priority: IoPriority::Idle,
                throttle_on_exceed_ms: 10000,
                llm_calls_per_hour: Some(5),
            },
            (Balanced, MemoryConsolidation) => Budget {
                cpu_pct_max: 25.0,
                ram_bytes_max: 1024 * MB,
                io_priority: IoPriority::Idle,
                throttle_on_exceed_ms: 5000,
                llm_calls_per_hour: Some(10),
            },
            (Aggressive, MemoryConsolidation) => Budget {
                cpu_pct_max: 50.0,
                ram_bytes_max: 2048 * MB,
                io_priority: IoPriority::BestEffort,
                throttle_on_exceed_ms: 1000,
                llm_calls_per_hour: Some(30),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 快照测试：防止任何人意外修改预设值。
    /// 修改预设需同步更新 spec §6 与 docs/system-impact.md 后再改这里。
    #[test]
    fn balanced_embedding_snapshot() {
        let b = Profile::Balanced.budget_for(TaskKind::EmbeddingQueue);
        assert_eq!(b.cpu_pct_max, 25.0);
        assert_eq!(b.ram_bytes_max, 1024 * MB);
        assert_eq!(b.io_priority, IoPriority::BestEffort);
        assert_eq!(b.throttle_on_exceed_ms, 1000);
        assert!(b.llm_calls_per_hour.is_none());
    }

    #[test]
    fn conservative_skill_evolution_snapshot() {
        let b = Profile::Conservative.budget_for(TaskKind::SkillEvolution);
        assert_eq!(b.cpu_pct_max, 10.0);
        assert_eq!(b.llm_calls_per_hour, Some(5));
        assert_eq!(b.throttle_on_exceed_ms, 5000);
    }

    #[test]
    fn aggressive_browser_search_has_largest_cpu() {
        let agg = Profile::Aggressive.budget_for(TaskKind::BrowserSearch);
        let bal = Profile::Balanced.budget_for(TaskKind::BrowserSearch);
        let con = Profile::Conservative.budget_for(TaskKind::BrowserSearch);
        assert!(agg.cpu_pct_max > bal.cpu_pct_max);
        assert!(bal.cpu_pct_max > con.cpu_pct_max);
    }

    #[test]
    fn task_kind_str_ids_are_stable() {
        // 这些 id 会出现在 attune --diag 输出和前端 UI；不可随意改。
        assert_eq!(TaskKind::EmbeddingQueue.as_str(), "embedding_queue");
        assert_eq!(TaskKind::BrowseSignalIngest.as_str(), "browse_signal_ingest");
        assert_eq!(TaskKind::MemoryConsolidation.as_str(), "memory_consolidation");
    }

    #[test]
    fn patent_scanner_inherits_file_scanner() {
        // Spec §6 中 PatentScanner 与 FileScanner 同档 — 验证不漂移
        for profile in [Profile::Conservative, Profile::Balanced, Profile::Aggressive] {
            let p = profile.budget_for(TaskKind::PatentScanner);
            let f = profile.budget_for(TaskKind::FileScanner);
            assert_eq!(p.cpu_pct_max, f.cpu_pct_max);
            assert_eq!(p.ram_bytes_max, f.ram_bytes_max);
        }
    }

    #[test]
    fn default_profile_is_balanced() {
        assert_eq!(Profile::default(), Profile::Balanced);
    }

    /// 全 30 组合 (3 profiles × 10 task kinds) snapshot — 防漂移。
    /// 修改任何预设值都需要同步更新此表 + spec §6 + docs/system-impact.md。
    #[test]
    fn all_30_combinations_snapshot() {
        // (profile, kind, cpu_pct, ram_mb, throttle_ms, llm_per_h)
        type Case = (Profile, TaskKind, f32, u64, u64, Option<u32>);
        let cases: &[Case] = &[
            // EmbeddingQueue
            (Profile::Conservative, TaskKind::EmbeddingQueue, 15.0, 512, 2000, None),
            (Profile::Balanced, TaskKind::EmbeddingQueue, 25.0, 1024, 1000, None),
            (Profile::Aggressive, TaskKind::EmbeddingQueue, 60.0, 2048, 100, None),
            // SkillEvolution
            (Profile::Conservative, TaskKind::SkillEvolution, 10.0, 256, 5000, Some(5)),
            (Profile::Balanced, TaskKind::SkillEvolution, 20.0, 512, 2000, Some(10)),
            (Profile::Aggressive, TaskKind::SkillEvolution, 40.0, 1024, 500, Some(30)),
            // FileScanner
            (Profile::Conservative, TaskKind::FileScanner, 10.0, 256, 1000, None),
            (Profile::Balanced, TaskKind::FileScanner, 20.0, 512, 500, None),
            (Profile::Aggressive, TaskKind::FileScanner, 50.0, 1024, 100, None),
            // WebDavSync
            (Profile::Conservative, TaskKind::WebDavSync, 10.0, 128, 5000, None),
            (Profile::Balanced, TaskKind::WebDavSync, 15.0, 256, 2000, None),
            (Profile::Aggressive, TaskKind::WebDavSync, 30.0, 512, 500, None),
            // PatentScanner — 与 FileScanner 同档（继承）
            (Profile::Conservative, TaskKind::PatentScanner, 10.0, 256, 1000, None),
            (Profile::Balanced, TaskKind::PatentScanner, 20.0, 512, 500, None),
            (Profile::Aggressive, TaskKind::PatentScanner, 50.0, 1024, 100, None),
            // BrowserSearch
            (Profile::Conservative, TaskKind::BrowserSearch, 30.0, 1024, 1000, None),
            (Profile::Balanced, TaskKind::BrowserSearch, 50.0, 1536, 500, None),
            (Profile::Aggressive, TaskKind::BrowserSearch, 80.0, 2048, 100, None),
            // AiAnnotator
            (Profile::Conservative, TaskKind::AiAnnotator, 10.0, 256, 3000, None),
            (Profile::Balanced, TaskKind::AiAnnotator, 20.0, 512, 1000, None),
            (Profile::Aggressive, TaskKind::AiAnnotator, 50.0, 1024, 200, None),
            // BrowseSignalIngest (G1)
            (Profile::Conservative, TaskKind::BrowseSignalIngest, 5.0, 64, 5000, None),
            (Profile::Balanced, TaskKind::BrowseSignalIngest, 10.0, 128, 2000, None),
            (Profile::Aggressive, TaskKind::BrowseSignalIngest, 20.0, 256, 500, None),
            // AutoBookmark (G2)
            (Profile::Conservative, TaskKind::AutoBookmark, 10.0, 256, 5000, None),
            (Profile::Balanced, TaskKind::AutoBookmark, 20.0, 512, 2000, None),
            (Profile::Aggressive, TaskKind::AutoBookmark, 40.0, 1024, 500, None),
            // MemoryConsolidation (A1)
            (Profile::Conservative, TaskKind::MemoryConsolidation, 15.0, 512, 10000, Some(5)),
            (Profile::Balanced, TaskKind::MemoryConsolidation, 25.0, 1024, 5000, Some(10)),
            (Profile::Aggressive, TaskKind::MemoryConsolidation, 50.0, 2048, 1000, Some(30)),
        ];
        assert_eq!(cases.len(), 30, "must cover 3 profiles × 10 kinds");

        for (profile, kind, expect_cpu, expect_ram_mb, expect_throttle, expect_llm) in cases {
            let b = profile.budget_for(*kind);
            assert_eq!(
                b.cpu_pct_max, *expect_cpu,
                "{:?}/{:?} cpu_pct_max", profile, kind
            );
            assert_eq!(
                b.ram_bytes_max, expect_ram_mb * MB,
                "{:?}/{:?} ram_bytes_max", profile, kind
            );
            assert_eq!(
                b.throttle_on_exceed_ms, *expect_throttle,
                "{:?}/{:?} throttle_on_exceed_ms", profile, kind
            );
            assert_eq!(
                b.llm_calls_per_hour, *expect_llm,
                "{:?}/{:?} llm_calls_per_hour", profile, kind
            );
        }
    }
}
