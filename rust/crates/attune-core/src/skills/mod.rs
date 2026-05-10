//! 内部 skills 集合.
//!
//! 设计:
//! - 纯函数 (同样输入 → 同样输出, 可缓存)
//! - 不编排多个 skill, 不做多步推理
//! - 可调 LLM 但只调一次

pub mod parse_chinese_date;
