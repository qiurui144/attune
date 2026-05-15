//! capture / 入库 capability traits.
//!
//! 本模块定义"从外部源捕获消息并入库 attune vault"的抽象接口。
//! v0.7 scaffold：仅 trait + Mock 实现 + 单测。
//! v0.8 真生产化：分别接入 async-imap / teloxide 等真实客户端。
pub mod email;
pub mod telegram;
