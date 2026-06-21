pub mod agents;
pub mod ai_stack;
pub mod annotations;
pub mod audit;
pub mod auto_bookmarks;
pub mod demo;
pub mod behavior;
pub mod browse_signals;
pub mod chat;
pub mod chat_sessions;
pub mod documents;
pub mod dsar;
pub mod email;
pub mod errors;
pub mod export;  // artifact export → downloadable xlsx/csv/md/docx/pdf (零成本,无 LLM)
pub mod feedback;
pub mod folder_links;
pub mod forms;
pub mod git;
pub mod member;
pub mod memory;
pub mod ocr_profiles;
#[cfg(feature = "nontext")]
pub mod ocr_recognize;
pub mod office;
pub mod organize;
pub mod jobs;
pub mod classify;
pub mod clusters;
pub mod index;
pub mod ingest;
pub mod items;
pub mod llm;
pub mod marketplace;
pub mod plugins;
pub mod privacy;
pub mod profile;
pub mod projects;
pub mod remote;
pub mod rss;
pub mod scenarios;
pub mod search;
pub mod settings;
pub mod skills;
pub mod skill_dispatch;  // pro plugin-agent dispatcher for skill_runtime (CAP-4b)
pub mod skill_runtime;  // skills 编排运行时 REST: list / estimate / run (💰 用户触发)
pub mod status;
pub mod suggestions;  // 零成本主动建议引擎 REST(A2)
pub mod third_party_accounts;  // 第三方账号统一管理 REST(波 C / 能力 B):加密凭据 CRUD,脱敏响应
pub mod tags;
pub mod ui;
pub mod upload;
pub mod vault;
pub mod version;
pub mod watches;  // info-monitoring loop: watch / digest / triage / ask / research (spec 2026-06-19)
pub mod web_search_cache;
pub mod writing;
pub mod ws;
