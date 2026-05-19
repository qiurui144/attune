//! sync / 远端同步 capability traits.
//!
//! 本模块定义"把 vault 数据备份/同步到远端"的抽象接口。
//! v0.7 scaffold：仅 WebDAV trait + Mock + 单测。
//! v0.8 真生产化：用 reqwest 实现 WebDAV PROPFIND/PUT/GET，
//! 支持 Nextcloud / Dropbox WebDAV / 大多数 NAS。
pub mod webdav;
