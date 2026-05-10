//! OSS 内部 skills (per attune-plugin-protocol §2 三角色定义)
//!
//! 不暴露为独立 agent — document_classifier_agent 内部调用.
//!
//! 设计原则:
//! - 纯函数 (同样输入 → 同样输出, 可缓存)
//! - 不调多个 skill, 不做多步推理
//! - 可调 LLM 但只调一次

pub mod parse_chinese_date;
