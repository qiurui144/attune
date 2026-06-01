//! Git 仓库采集源（GitConnector，OSS 仓导入）。
//!
//! 第四个 `SourceConnector` 实现（继 Email/WebDAV/RSS 之后）。让 attune 直接
//! 「输入仓库 URL → clone → glob 过滤 → 入库 → 增量跟随上游 commit」。
//!
//! 设计要点（spec `2026-05-31-git-connector-oss-import.md`）：
//!
//! 1. **libgit2（git2 crate）非 shell git**：clone/fetch/diff 走库内 API，不依赖
//!    系统 git 子进程（桌面机可能没装 git；避 subprocess-env 坑）。`GitCloner`
//!    trait 隔离，测试注入 mock，未来可换实现不动 connector。
//! 2. **shallow + sparse**：`--depth 1` 等价（fetch depth 1）+ 可选子目录限定。
//! 3. **glob 过滤 + 二进制/超限/LFS 跳过**：默认 include 知识类扩展名。
//! 4. **增量 commit SHA 游标**：再次同步 `diff <old>..<new> --name-status`，
//!    force-push 致 diff 失败 → fallback 全量（content_hash 短路兜底）。
//! 5. **SSRF + token**：URL 过 `net::url_guard`；token 仅进程内存拼 URL，不落盘。
//! 6. **导入零 LLM**（成本契约 §8）：只到「可被搜到 + 存档摘要」。

/// GitConnector 构造输入（route / worker 物化后注入）。
#[derive(Debug, Clone)]
pub struct GitSourceConfig {
    /// 用户输入的原始 URL（归一前）。
    pub url: String,
    /// 分支 / tag；None = 仓默认分支。
    pub branch: Option<String>,
    /// 子目录（sparse checkout）；None = 整仓。
    pub subdir: Option<String>,
    /// include glob 列表；空 = 用默认知识类 glob。
    pub include_glob: Vec<String>,
    /// exclude glob 列表。
    pub exclude_glob: Vec<String>,
    /// 语料领域（F-Pro 透传，缺省 general；OSS 一般不填）。
    pub corpus_domain: Option<String>,
    /// 私有仓凭据明文 token（仅进程内存，不落盘）；None = 公开仓。
    pub token: Option<String>,
    /// 文件数上限。
    pub max_files: u64,
    /// 单文件字节上限。
    pub max_file_bytes: u64,
    /// 仓总字节上限（拒绝超大仓）。
    pub max_total_bytes: u64,
    /// 增量起点 commit SHA；None = 全量首扫。
    pub last_commit_sha: Option<String>,
    /// SSRF host allowlist（额外允许的自建 host；默认平台 host 见 url_guard）。
    pub allow_hosts: Vec<String>,
}

impl GitSourceConfig {
    /// 默认知识类 include glob —— 文档 / 笔记 / 常见源码扩展。
    pub const DEFAULT_INCLUDE: &'static [&'static str] = &[
        "**/*.md",
        "**/*.markdown",
        "**/*.rst",
        "**/*.txt",
        "**/*.adoc",
        "**/*.org",
        "**/*.rs",
        "**/*.py",
        "**/*.js",
        "**/*.ts",
        "**/*.go",
        "**/*.java",
        "**/*.c",
        "**/*.h",
        "**/*.cpp",
        "**/*.hpp",
    ];

    /// 三限额默认值（per plan §9 评审拍板）。
    pub const DEFAULT_MAX_FILES: u64 = 5000;
    pub const DEFAULT_MAX_FILE_BYTES: u64 = 5 * 1024 * 1024; // 5 MiB
    pub const DEFAULT_MAX_TOTAL_BYTES: u64 = 500 * 1024 * 1024; // 500 MiB

    /// 用 URL 构造默认配置（限额 / glob 取默认，公开仓无 token）。
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            branch: None,
            subdir: None,
            include_glob: Vec::new(),
            exclude_glob: Vec::new(),
            corpus_domain: None,
            token: None,
            max_files: Self::DEFAULT_MAX_FILES,
            max_file_bytes: Self::DEFAULT_MAX_FILE_BYTES,
            max_total_bytes: Self::DEFAULT_MAX_TOTAL_BYTES,
            last_commit_sha: None,
            allow_hosts: Vec::new(),
        }
    }
}
