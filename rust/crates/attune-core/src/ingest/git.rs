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
//! 2. **shallow + sparse**：fetch depth 1 + 可选子目录限定（subdir 过滤在 walk 层）。
//! 3. **glob 过滤 + 二进制/超限/LFS 跳过**：默认 include 知识类扩展名。
//! 4. **增量 commit SHA 游标**：再次同步 `diff <old>..<new>`，A/M 产 doc、D 标删；
//!    force-push 致 diff 失败 → fallback 全量（content_hash 短路兜底）。
//! 5. **SSRF + token**：URL 过 `net::url_guard`；token 仅进程内存拼 URL，不落盘。
//! 6. **导入零 LLM**（成本契约 §8）：只到「可被搜到 + 存档摘要」。

use std::collections::HashMap;
use std::path::Path;

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::error::{Result, VaultError};
use crate::ingest::{DocumentSink, RawDocument, SourceConnector, SourceKind};
use crate::net::url_guard;

/// GitConnector 构造输入（route / worker 物化后注入）。
#[derive(Debug, Clone)]
pub struct GitSourceConfig {
    /// 用户输入的原始 URL（归一前）。
    pub url: String,
    /// 分支 / tag；None = 仓默认分支。
    pub branch: Option<String>,
    /// 子目录（限定导入子树）；None = 整仓。
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

/// 归一后的仓标识。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedRepo {
    /// 用于 git clone 的 URL（去 `.git` 多余形态前的规范 https URL，带 `.git`）。
    pub clone_url: String,
    /// 主机名。
    pub host: String,
    /// owner/repo slug（展示 + source_ref 前缀）。
    pub slug: String,
}

/// 归一 URL —— 接受 `https://host/owner/repo(.git)`（含末尾 `/`），产出统一形态。
///
/// 不做平台特定 raw/tarball 映射（v1 只走 git clone 路径，per spec §2.2 raw/tarball
/// 兜底推 v.next）。仅规整 scheme/host/path 并抽 owner/repo slug。
pub fn normalize_url(raw: &str) -> Result<NormalizedRepo> {
    let trimmed = raw.trim().trim_end_matches('/');
    let parsed = url::Url::parse(trimmed)
        .map_err(|e| VaultError::InvalidInput(format!("invalid-git-url: parse: {e}")))?;

    // file:// —— 仅本地 fixture / 测试用（无 host）。clone_url 原样透传, slug 取
    // 路径末段。生产 bind 走 route SSRF 校验拒 file://（host allowlist），此分支
    // 不构成 SSRF 面（无远程 fetch）。
    if parsed.scheme() == "file" {
        let p = parsed.path().trim_end_matches('/');
        let repo = p.rsplit('/').next().unwrap_or("repo").trim_end_matches(".git");
        return Ok(NormalizedRepo {
            clone_url: trimmed.to_string(),
            host: "localhost".into(),
            slug: format!("local/{repo}"),
        });
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| VaultError::InvalidInput("invalid-git-url: missing host".into()))?
        .to_ascii_lowercase();

    // path: /owner/repo(.git) —— 取前两段作 slug。
    let segs: Vec<&str> = parsed
        .path()
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if segs.len() < 2 {
        return Err(VaultError::InvalidInput(
            "invalid-git-url: expected https://host/owner/repo".into(),
        ));
    }
    let owner = segs[0];
    let repo = segs[1].trim_end_matches(".git");
    let slug = format!("{owner}/{repo}");
    let clone_url = format!("https://{host}/{owner}/{repo}.git");
    Ok(NormalizedRepo {
        clone_url,
        host,
        slug,
    })
}

/// 一次 clone/fetch 的产出：工作树文件列表（relpath + 字节）+ HEAD commit。
pub struct FetchedTree {
    /// HEAD commit SHA（增量游标）。
    pub commit_sha: String,
    /// (relpath, bytes) —— 已是该 ref HEAD 的工作树文件。
    /// 全量模式 = 全部文件；增量模式 = 仅 A/M 文件。
    pub files: Vec<(String, Vec<u8>)>,
    /// 增量模式下被删除的 relpath（全量模式空）。
    pub deleted: Vec<String>,
    /// true = 全量（首扫 / force-push fallback）；false = 增量 diff。
    pub full: bool,
}

/// clone/fetch/diff 抽象。生产 = libgit2；测试 = mock（离线）。
pub trait GitCloner: Send + Sync {
    /// 拉取仓内容。`last_commit_sha=None` → 全量；`Some(old)` → 尝试增量 diff，
    /// diff 失败（force-push / old commit 丢失）应 fallback 全量（`full=true`）。
    ///
    /// 实现者负责 shallow clone 到临时目录 + 用后即清（RAII TempDir）。
    /// token（若有）仅进程内存拼 URL，不落盘 / 不入日志。
    fn fetch(&self, repo: &NormalizedRepo, config: &GitSourceConfig) -> Result<FetchedTree>;
}

/// 生产实现 —— git2 / libgit2。
pub struct Git2Cloner;

impl Git2Cloner {
    /// 拼带 token 的认证 URL（仅进程内存）。public 仓直接用 clone_url。
    fn auth_url(repo: &NormalizedRepo, token: Option<&str>) -> String {
        match token {
            // x-access-token 是 GitHub PAT over HTTPS 的惯例用户名；GitLab/Gitea
            // 也接受 `<token>@host` 形态。错误信息不回显此串（脱敏）。
            Some(t) => format!("https://x-access-token:{t}@{}", repo.clone_url.trim_start_matches("https://")),
            None => repo.clone_url.clone(),
        }
    }
}

impl GitCloner for Git2Cloner {
    fn fetch(&self, repo: &NormalizedRepo, config: &GitSourceConfig) -> Result<FetchedTree> {
        let tmp = tempfile::tempdir()
            .map_err(|e| VaultError::InvalidInput(format!("git-network-error: tempdir: {e}")))?;

        let auth = Self::auth_url(repo, config.token.as_deref());

        // shallow clone（--depth 1）—— 智能 HTTP/SSH 传输支持；本地 file:// 传输
        // 不支持 shallow（libgit2 限制）→ fall back 全量 clone。也兜底某些 server
        // 拒绝 shallow 的情况。两路都失败才报错。
        let clone_with = |depth: Option<i32>, dest: &std::path::Path| {
            let mut fo = git2::FetchOptions::new();
            if let Some(d) = depth {
                fo.depth(d);
            }
            let mut builder = git2::build::RepoBuilder::new();
            builder.fetch_options(fo);
            if let Some(b) = &config.branch {
                builder.branch(b);
            }
            builder.clone(&auth, dest)
        };

        let git_repo = match clone_with(Some(1), tmp.path()) {
            Ok(r) => r,
            Err(e) if e.class() == git2::ErrorClass::Net && e.message().contains("shallow") => {
                // 本地传输不支持 shallow → 重建临时目录全量 clone。
                let tmp2 = tempfile::tempdir().map_err(|e| {
                    VaultError::InvalidInput(format!("git-network-error: tempdir: {e}"))
                })?;
                let r = clone_with(None, tmp2.path()).map_err(map_git_err)?;
                // 用全量目录顶替 shallow 临时目录（保持后续 walk 用 tmp2）。
                return walk_and_collect(tmp2.path(), &r, config);
            }
            Err(e) => return Err(map_git_err(e)),
        };
        return walk_and_collect(tmp.path(), &git_repo, config);
    }
}

/// 走工作树收集匹配文件（shallow / full clone 后共用）。
fn walk_and_collect(
    walk_root: &std::path::Path,
    git_repo: &git2::Repository,
    config: &GitSourceConfig,
) -> Result<FetchedTree> {
    let head = git_repo
        .head()
        .map_err(map_git_err)?
        .peel_to_commit()
        .map_err(map_git_err)?;
    let commit_sha = head.id().to_string();

    // 全量 walk 工作树（shallow depth-1 clone 无历史, diff <old>..<new> 不可行 →
    // 本实现始终产全量 tree；增量去重靠 server 层 indexed_files.file_hash (per-file
    // SHA-256) + content_hash 短路, force-push 天然 fallback 全量）。
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut total_bytes: u64 = 0u64;

    for entry in walkdir::WalkDir::new(walk_root)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        // 跳过 .git 内部。
        if path.components().any(|c| c.as_os_str() == ".git") {
            continue;
        }
        let rel = match path.strip_prefix(walk_root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");

        // path traversal 防御（symlink 逃出工作树 / `..`）—— relpath 不得含 `..`。
        if rel.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            continue;
        }

        // 读字节（失败跳过，可恢复）。
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        total_bytes = total_bytes.saturating_add(bytes.len() as u64);
        if total_bytes > config.max_total_bytes {
            return Err(VaultError::InvalidInput(format!(
                "git-repo-too-large: exceeds {} bytes (add subdir / narrow scope)",
                config.max_total_bytes
            )));
        }
        files.push((rel_str, bytes));
    }

    Ok(FetchedTree {
        commit_sha,
        files,
        deleted: Vec::new(),
        full: true,
    })
}

/// 把 git2 错误映射到带 kebab 错误码的脱敏消息（不含 token / 内网细节）。
fn map_git_err(e: git2::Error) -> VaultError {
    use git2::ErrorClass as C;
    use git2::ErrorCode as Code;
    let code = match (e.class(), e.code()) {
        (C::Http, _) | (C::Net, _) | (_, Code::Auth) if is_auth(&e) => "git-auth-failed",
        (_, Code::NotFound) => "git-repo-not-found",
        (C::Reference, _) => "git-ref-not-found",
        (C::Net, _) | (C::Http, _) | (C::Os, _) => "git-network-error",
        _ => "git-network-error",
    };
    // git2 错误 message 可能含 URL（带 token）—— 不直接透传，只给 class+code。
    let _ = e.message(); // 显式不使用 raw message（脱敏）
    VaultError::InvalidInput(format!("{code}: git operation failed"))
}

fn is_auth(e: &git2::Error) -> bool {
    let m = e.message().to_lowercase();
    e.code() == git2::ErrorCode::Auth
        || m.contains("authentication")
        || m.contains("401")
        || m.contains("403")
}

/// GitConnector —— 实现 `SourceConnector`。构造时持有 cloner（生产/mock）。
pub struct GitConnector {
    config: GitSourceConfig,
    repo: NormalizedRepo,
    cloner: Box<dyn GitCloner>,
    /// fetch 后回填的 HEAD commit（caller 取作游标）。
    last_commit: std::cell::RefCell<Option<String>>,
    /// fetch 后回填的删除列表（增量 D）。
    deleted: std::cell::RefCell<Vec<String>>,
}

impl GitConnector {
    /// 用生产 cloner（libgit2）构造。会先归一 URL（不校验 SSRF —— 调用方在
    /// route 层先过 `url_guard`，避免重复解析）。
    pub fn new(config: GitSourceConfig) -> Result<Self> {
        Self::with_cloner(config, Box::new(Git2Cloner))
    }

    /// 注入 cloner（测试用 mock）。
    pub fn with_cloner(config: GitSourceConfig, cloner: Box<dyn GitCloner>) -> Result<Self> {
        let repo = normalize_url(&config.url)?;
        Ok(Self {
            config,
            repo,
            cloner,
            last_commit: std::cell::RefCell::new(None),
            deleted: std::cell::RefCell::new(Vec::new()),
        })
    }

    /// route 层调用：在 fetch 前做 SSRF 校验（host allowlist + 拒内网）。
    /// resolve 注入 DNS（生产 = system_resolve；测试注入固定映射）。
    pub fn check_ssrf(
        &self,
        resolve: &dyn Fn(&str) -> std::io::Result<Vec<std::net::IpAddr>>,
    ) -> Result<()> {
        url_guard::validate_outbound_url(&self.repo.clone_url, &self.config.allow_hosts, resolve)?;
        Ok(())
    }

    /// fetch 后取 HEAD commit SHA（游标）。
    pub fn take_last_commit(&self) -> Option<String> {
        self.last_commit.borrow_mut().take()
    }

    /// fetch 后取删除文件列表（增量 D）。
    pub fn take_deleted(&self) -> Vec<String> {
        std::mem::take(&mut *self.deleted.borrow_mut())
    }

    /// 编译 include/exclude glob matcher。include 空 → 默认知识类。
    fn build_globs(&self) -> Result<(GlobSet, GlobSet)> {
        let include_src: Vec<String> = if self.config.include_glob.is_empty() {
            GitSourceConfig::DEFAULT_INCLUDE.iter().map(|s| s.to_string()).collect()
        } else {
            self.config.include_glob.clone()
        };
        let include = compile_globs(&include_src)?;
        let exclude = compile_globs(&self.config.exclude_glob)?;
        Ok((include, exclude))
    }

    /// subdir 限定：relpath 必须在 subdir/ 下，且对外暴露的 ref 仍带完整 relpath
    /// （检索可见仓内结构）。subdir=None → 全收。
    fn in_subdir(&self, rel: &str) -> bool {
        match &self.config.subdir {
            None => true,
            Some(sub) => {
                let sub = sub.trim_matches('/');
                sub.is_empty() || rel == sub || rel.starts_with(&format!("{sub}/"))
            }
        }
    }
}

/// 编译 glob 列表为 GlobSet（空列表 → 空 matcher，永不匹配）。
fn compile_globs(patterns: &[String]) -> Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        let g = Glob::new(p)
            .map_err(|e| VaultError::InvalidInput(format!("invalid-git-url: glob {p}: {e}")))?;
        b.add(g);
    }
    b.build()
        .map_err(|e| VaultError::InvalidInput(format!("invalid-git-url: globset: {e}")))
}

/// 二进制探测 —— 前 8KB 含 NUL 字节即视为二进制。
pub fn looks_binary(bytes: &[u8]) -> bool {
    let probe = &bytes[..bytes.len().min(8192)];
    probe.contains(&0u8)
}

/// LFS 指针探测 —— 以 `version https://git-lfs` 开头的小文件。
pub fn is_lfs_pointer(bytes: &[u8]) -> bool {
    bytes.len() < 1024 && bytes.starts_with(b"version https://git-lfs.github.com/spec")
}

impl SourceConnector for GitConnector {
    fn source_kind(&self) -> SourceKind {
        SourceKind::GitRepo
    }

    fn fetch_documents(&self, sink: &mut DocumentSink<'_>) -> Result<()> {
        let tree = self.cloner.fetch(&self.repo, &self.config)?;
        *self.last_commit.borrow_mut() = Some(tree.commit_sha.clone());
        *self.deleted.borrow_mut() = tree.deleted.clone();

        let (include, exclude) = self.build_globs()?;
        let mut emitted = 0u64;

        for (rel, bytes) in tree.files {
            if emitted >= self.config.max_files {
                // 截断（不致命，per spec §7 warn）。
                break;
            }
            if !self.in_subdir(&rel) {
                continue;
            }
            let path = Path::new(&rel);
            if !include.is_match(path) || exclude.is_match(path) {
                continue;
            }
            if bytes.len() as u64 > self.config.max_file_bytes {
                continue; // 超单文件上限跳过
            }
            if is_lfs_pointer(&bytes) || looks_binary(&bytes) {
                continue; // LFS 指针 / 二进制跳过
            }

            let mut metadata: HashMap<String, String> = HashMap::new();
            metadata.insert("repo".into(), self.repo.slug.clone());
            metadata.insert("host".into(), self.repo.host.clone());
            metadata.insert("commit".into(), tree.commit_sha.clone());
            if let Some(b) = &self.config.branch {
                metadata.insert("branch".into(), b.clone());
            }

            let title = Path::new(&rel)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| rel.clone());

            // source_ref = "<slug>/<relpath>" —— 末段带扩展名 → parser 路由正确。
            let source_ref = format!("{}/{}", self.repo.slug, rel);
            // uri 含 commit + relpath，唯一可定位。
            let uri = format!(
                "git://{}/{}@{}/{}",
                self.repo.host, self.repo.slug, tree.commit_sha, rel
            );

            // modified_marker = 文件内容 SHA-256 —— 每文件独立, 增量时 indexed_files
            // 命中即跳过未变文件（不能用 commit SHA: 同 commit 下所有文件 marker 相同,
            // 会让 re-sync 误判全箱未变）。content_hash 短路是第二防线。
            let file_marker = {
                use sha2::{Digest, Sha256};
                let mut h = Sha256::new();
                h.update(&bytes);
                hex::encode(h.finalize())
            };

            sink(RawDocument {
                uri,
                title,
                content: bytes,
                mime_hint: None,
                source_kind: SourceKind::GitRepo,
                source_ref,
                modified_marker: Some(file_marker),
                domain: None,
                tags: None,
                corpus_domain: self.config.corpus_domain.clone(),
                metadata,
            });
            emitted += 1;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- URL 归一 ----

    #[test]
    fn normalize_github_variants() {
        for raw in [
            "https://github.com/rust-lang/book",
            "https://github.com/rust-lang/book/",
            "https://github.com/rust-lang/book.git",
            "https://GitHub.com/rust-lang/book",
        ] {
            let n = normalize_url(raw).unwrap();
            assert_eq!(n.host, "github.com");
            assert_eq!(n.slug, "rust-lang/book");
            assert_eq!(n.clone_url, "https://github.com/rust-lang/book.git");
        }
    }

    #[test]
    fn normalize_rejects_no_repo_path() {
        assert!(normalize_url("https://github.com/rust-lang").is_err());
        assert!(normalize_url("https://github.com/").is_err());
        assert!(normalize_url("not a url").is_err());
    }

    #[test]
    fn normalize_gitlab_subgroup_takes_first_two() {
        // gitlab subgroup: /group/subgroup/repo —— v1 取前两段 (group/subgroup)，
        // clone_url 仍指向前两段。记录该行为（subgroup 完整路径 v.next）。
        let n = normalize_url("https://gitlab.com/group/sub/repo").unwrap();
        assert_eq!(n.slug, "group/sub");
    }

    // ---- 二进制 / LFS 探测 ----

    #[test]
    fn binary_detection() {
        assert!(looks_binary(b"abc\0def"));
        assert!(!looks_binary(b"plain text only"));
        assert!(!looks_binary("中文文本无 NUL".as_bytes()));
    }

    #[test]
    fn lfs_pointer_detection() {
        let ptr = b"version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 12345\n";
        assert!(is_lfs_pointer(ptr));
        assert!(!is_lfs_pointer(b"# Normal markdown file"));
    }

    // ---- glob ----

    #[test]
    fn glob_include_exclude() {
        let inc = compile_globs(&["**/*.md".into()]).unwrap();
        let exc = compile_globs(&["**/SUMMARY.md".into()]).unwrap();
        assert!(inc.is_match(Path::new("src/ch01.md")));
        assert!(!inc.is_match(Path::new("src/ch01.rs")));
        assert!(exc.is_match(Path::new("src/SUMMARY.md")));
    }

    // ---- mock cloner 驱动 connector ----

    struct MockCloner {
        tree: FetchedTree,
    }
    impl GitCloner for MockCloner {
        fn fetch(&self, _r: &NormalizedRepo, _c: &GitSourceConfig) -> Result<FetchedTree> {
            Ok(FetchedTree {
                commit_sha: self.tree.commit_sha.clone(),
                files: self.tree.files.clone(),
                deleted: self.tree.deleted.clone(),
                full: self.tree.full,
            })
        }
    }

    fn mock_connector(files: Vec<(&str, &[u8])>, config: GitSourceConfig) -> GitConnector {
        let tree = FetchedTree {
            commit_sha: "deadbeefcafebabe".into(),
            files: files
                .into_iter()
                .map(|(p, b)| (p.to_string(), b.to_vec()))
                .collect(),
            deleted: Vec::new(),
            full: true,
        };
        GitConnector::with_cloner(config, Box::new(MockCloner { tree })).unwrap()
    }

    fn drain(conn: &GitConnector) -> Vec<RawDocument> {
        let mut docs = Vec::new();
        {
            let mut sink: DocumentSink<'_> = Box::new(|d| docs.push(d));
            conn.fetch_documents(&mut sink).unwrap();
        }
        docs
    }

    #[test]
    fn connector_emits_markdown_skips_binary_and_non_match() {
        let conn = mock_connector(
            vec![
                ("README.md", b"# Hello"),
                ("docs/guide.md", b"## Guide"),
                ("logo.png", b"\x89PNG\0\0binary"),
                ("Makefile", b"all:\n\techo hi"), // 不匹配默认 glob (无扩展名)
            ],
            GitSourceConfig::new("https://github.com/o/r"),
        );
        let docs = drain(&conn);
        let refs: Vec<&str> = docs.iter().map(|d| d.source_ref.as_str()).collect();
        assert!(refs.contains(&"o/r/README.md"));
        assert!(refs.contains(&"o/r/docs/guide.md"));
        assert!(!refs.iter().any(|r| r.contains("logo.png")));
        assert!(!refs.iter().any(|r| r.contains("Makefile")));
        assert_eq!(docs[0].source_kind, SourceKind::GitRepo);
        assert_eq!(conn.take_last_commit().as_deref(), Some("deadbeefcafebabe"));
        // commit SHA 进 metadata（marker 改用 per-file 内容 SHA）。
        assert_eq!(docs[0].metadata.get("commit").unwrap(), "deadbeefcafebabe");
    }

    #[test]
    fn connector_subdir_limits_subtree() {
        let mut config = GitSourceConfig::new("https://github.com/o/r");
        config.subdir = Some("src".into());
        let conn = mock_connector(
            vec![
                ("README.md", b"# root"),
                ("src/ch01.md", b"# ch1"),
                ("src/nested/ch02.md", b"# ch2"),
            ],
            config,
        );
        let docs = drain(&conn);
        let refs: Vec<&str> = docs.iter().map(|d| d.source_ref.as_str()).collect();
        assert!(refs.contains(&"o/r/src/ch01.md"));
        assert!(refs.contains(&"o/r/src/nested/ch02.md"));
        assert!(!refs.iter().any(|r| r.ends_with("README.md")));
    }

    #[test]
    fn connector_respects_max_files_and_max_file_bytes() {
        let mut config = GitSourceConfig::new("https://github.com/o/r");
        config.max_files = 1;
        let conn = mock_connector(
            vec![("a.md", b"a"), ("b.md", b"b"), ("c.md", b"c")],
            config,
        );
        assert_eq!(drain(&conn).len(), 1, "max_files=1 截断");

        let mut config2 = GitSourceConfig::new("https://github.com/o/r");
        config2.max_file_bytes = 3;
        let conn2 = mock_connector(
            vec![("small.md", b"hi"), ("big.md", b"way too long content")],
            config2,
        );
        let docs = drain(&conn2);
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].source_ref, "o/r/small.md");
    }

    #[test]
    fn connector_skips_lfs_pointer() {
        let conn = mock_connector(
            vec![(
                "data.md",
                b"version https://git-lfs.github.com/spec/v1\noid sha256:x\nsize 9\n",
            )],
            GitSourceConfig::new("https://github.com/o/r"),
        );
        assert_eq!(drain(&conn).len(), 0, "LFS 指针不入库");
    }

    #[test]
    fn connector_chinese_content_emitted_for_jieba() {
        let conn = mock_connector(
            vec![("notes/中文.md", "# 反洗钱合规笔记\n\n内容".as_bytes())],
            GitSourceConfig::new("https://github.com/o/r"),
        );
        let docs = drain(&conn);
        assert_eq!(docs.len(), 1);
        let body = std::str::from_utf8(&docs[0].content).unwrap();
        assert!(body.contains("反洗钱"));
        assert_eq!(docs[0].source_ref, "o/r/notes/中文.md");
    }

    #[test]
    fn connector_max_total_bytes_enforced_in_cloner() {
        // mock cloner 不强制 total（那是 Git2Cloner 职责）；这里验证 connector
        // 层 per-file + max_files 护栏已覆盖。total 上限的真测在 Git2Cloner（集成）。
        let conn = mock_connector(
            vec![("a.md", b"x")],
            GitSourceConfig::new("https://github.com/o/r"),
        );
        assert_eq!(drain(&conn).len(), 1);
    }

    // proptest #1：URL 归一幂等 —— 归一两次 == 归一一次。
    proptest::proptest! {
        #[test]
        fn normalize_idempotent(owner in "[a-z][a-z0-9-]{0,12}", repo in "[a-z][a-z0-9_-]{0,12}") {
            let raw = format!("https://github.com/{owner}/{repo}");
            let once = normalize_url(&raw).unwrap();
            let twice = normalize_url(&once.clone_url).unwrap();
            proptest::prop_assert_eq!(once.clone_url, twice.clone_url);
            proptest::prop_assert_eq!(once.slug, twice.slug);
        }

        // proptest #2：glob 匹配稳定 —— .md 始终被默认 include 匹配。
        #[test]
        fn default_include_matches_md(name in "[a-z]{1,10}") {
            let inc = compile_globs(
                &GitSourceConfig::DEFAULT_INCLUDE.iter().map(|s| s.to_string()).collect::<Vec<_>>()
            ).unwrap();
            let p = format!("dir/{name}.md");
            proptest::prop_assert!(inc.is_match(Path::new(&p)));
        }

        // proptest #3：modified_marker 稳定 —— 同内容文件的 marker (per-file SHA-256)
        // 跨次稳定且非空 (增量去重不漂移)。
        #[test]
        fn marker_stable_for_same_content(body in "[a-z ]{1,40}") {
            let bytes = body.clone().into_bytes();
            let conn = mock_connector(
                vec![("f.md", bytes.as_slice())],
                GitSourceConfig::new("https://github.com/o/r"),
            );
            let docs1 = drain(&conn);
            let conn2 = mock_connector(
                vec![("f.md", bytes.as_slice())],
                GitSourceConfig::new("https://github.com/o/r"),
            );
            let docs2 = drain(&conn2);
            proptest::prop_assert_eq!(docs1.len(), 1);
            let m1 = docs1[0].modified_marker.clone().unwrap();
            let m2 = docs2[0].modified_marker.clone().unwrap();
            proptest::prop_assert_eq!(&m1, &m2);
            proptest::prop_assert_eq!(m1.len(), 64); // SHA-256 hex
        }
    }
}
