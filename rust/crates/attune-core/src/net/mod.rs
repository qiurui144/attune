//! 网络安全辅助 —— 出站 URL 校验（SSRF 防御）。
//!
//! 当前用于 GitConnector clone / raw-tarball fetch；任意「拿用户给的 URL 去
//! 连接」的路径都应过 [`url_guard::validate_outbound_url`]，避免被诱导打内网 /
//! 云 metadata 端点。

pub mod url_guard;
