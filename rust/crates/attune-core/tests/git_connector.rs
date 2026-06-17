//! GitConnector 集成测试 —— 用本地 bare git repo fixture（无网络，CI 可跑）。
//!
//! 通过 git2 在临时目录构造真实 bare repo（写 blob/tree/commit），然后用真实的
//! `Git2Cloner` clone 它（`file://` URL）。这是 D6 的 happy / edge / 增量真测，
//! 不依赖任何远程仓 / 网络（per docs/TESTING.md 禁随机 + 离线 CI）。

use std::collections::HashMap;
use std::path::Path;

use attune_core::ingest::git::{GitConnector, GitSourceConfig};
use attune_core::ingest::{DocumentSink, RawDocument};
use attune_core::ingest::SourceConnector;

/// 在 `dir` 建一个 bare repo，commit 一组 (relpath, content)，返回 file:// URL。
fn build_bare_repo(dir: &Path, files: &[(&str, &[u8])]) -> String {
    // 非 bare 工作仓里写文件 + commit，再 clone 成 bare 供 connector 拉。
    let work = dir.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let repo = git2::Repository::init(&work).unwrap();

    for (rel, content) in files {
        let full = work.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&full, content).unwrap();
    }

    let mut index = repo.index().unwrap();
    index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None).unwrap();
    index.write().unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();
    let sig = git2::Signature::now("Test", "test@example.com").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[]).unwrap();

    // file:// URL 指向工作仓（含 .git）—— libgit2 可从本地 .git clone。
    // 跨平台:用 url::Url::from_directory_path 生成 RFC-8089 合规的 file URL。
    // 不能用 format!("file://{}", path)——在 Windows 上 path.display() 是
    // `C:\Users\…\work`(盘符 + 反斜杠),拼成 `file://C:\…` 会被 url::Url::parse
    // 当成 host=`C:`(只有两道斜杠)且反斜杠非法,libgit2 解析失败 → git-network-error
    // (7/8 测试在 Windows CI 上 panic 的根因)。from_directory_path 在 Windows 产出
    // `file:///C:/Users/…/work/`(三斜杠 + 正斜杠),在 Unix 产出 `file:///home/…/work/`。
    url::Url::from_directory_path(&work)
        .expect("work dir is absolute (tempdir) — from_directory_path must succeed")
        .to_string()
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
fn clones_local_bare_repo_and_emits_markdown() {
    let tmp = tempfile::tempdir().unwrap();
    let url = build_bare_repo(
        tmp.path(),
        &[
            ("README.md", b"# Project\n\nHello world."),
            ("docs/guide.md", b"## Guide\n\nDetails here."),
            ("src/main.rs", b"fn main() {}"),
            ("logo.png", b"\x89PNG\r\n\x1a\n\0\0binary"),
        ],
    );
    let conn = GitConnector::new(GitSourceConfig::new(url)).unwrap();
    let docs = drain(&conn);
    let refs: HashMap<String, RawDocument> =
        docs.into_iter().map(|d| (d.source_ref.clone(), d)).collect();

    // .md + .rs 入库, .png 二进制跳过。
    assert!(refs.keys().any(|k| k.ends_with("README.md")), "README.md 应入库");
    assert!(refs.keys().any(|k| k.ends_with("docs/guide.md")), "guide.md 应入库");
    assert!(refs.keys().any(|k| k.ends_with("src/main.rs")), "main.rs 应入库");
    assert!(!refs.keys().any(|k| k.ends_with("logo.png")), "二进制 png 应跳过");

    // 内容正确 + commit metadata 存在。
    let readme = refs.iter().find(|(k, _)| k.ends_with("README.md")).unwrap().1;
    assert!(std::str::from_utf8(&readme.content).unwrap().contains("Hello world"));
    assert!(readme.metadata.contains_key("commit"));
    assert_eq!(readme.metadata.get("host").map(String::as_str), Some("localhost"));
}

#[test]
fn subdir_limits_to_subtree() {
    let tmp = tempfile::tempdir().unwrap();
    let url = build_bare_repo(
        tmp.path(),
        &[
            ("README.md", b"# root"),
            ("src/a.md", b"# a"),
            ("src/sub/b.md", b"# b"),
        ],
    );
    let mut config = GitSourceConfig::new(url);
    config.subdir = Some("src".into());
    let conn = GitConnector::new(config).unwrap();
    let docs = drain(&conn);
    let refs: Vec<String> = docs.iter().map(|d| d.source_ref.clone()).collect();
    assert!(refs.iter().any(|r| r.ends_with("src/a.md")));
    assert!(refs.iter().any(|r| r.ends_with("src/sub/b.md")));
    assert!(!refs.iter().any(|r| r.ends_with("README.md")), "subdir 外的 README 不应入库");
}

#[test]
fn include_glob_restricts_extensions() {
    let tmp = tempfile::tempdir().unwrap();
    let url = build_bare_repo(
        tmp.path(),
        &[("a.md", b"# md"), ("b.rs", b"fn b() {}"), ("c.txt", b"text")],
    );
    let mut config = GitSourceConfig::new(url);
    config.include_glob = vec!["**/*.md".into()];
    let conn = GitConnector::new(config).unwrap();
    let docs = drain(&conn);
    let refs: Vec<String> = docs.iter().map(|d| d.source_ref.clone()).collect();
    assert_eq!(refs.len(), 1, "仅 .md 入库");
    assert!(refs[0].ends_with("a.md"));
}

#[test]
fn empty_repo_emits_nothing() {
    // 空仓: git2 不允许空 commit tree 直接 init 无文件 commit, 用单个被 glob
    // 排除的文件模拟「无匹配文档」。
    let tmp = tempfile::tempdir().unwrap();
    let url = build_bare_repo(tmp.path(), &[("only.bin", b"\0\0\0binary")]);
    let conn = GitConnector::new(GitSourceConfig::new(url)).unwrap();
    let docs = drain(&conn);
    assert_eq!(docs.len(), 0, "全二进制仓产出 0 文档");
}

#[test]
fn chinese_content_preserved_for_jieba() {
    let tmp = tempfile::tempdir().unwrap();
    let url = build_bare_repo(
        tmp.path(),
        &[("笔记/反洗钱.md", "# 反洗钱合规\n\n本文介绍尽职调查流程。".as_bytes())],
    );
    let conn = GitConnector::new(GitSourceConfig::new(url)).unwrap();
    let docs = drain(&conn);
    assert_eq!(docs.len(), 1);
    let body = std::str::from_utf8(&docs[0].content).unwrap();
    assert!(body.contains("反洗钱"));
    assert!(body.contains("尽职调查"));
}

#[test]
fn max_file_bytes_skips_large_file() {
    let tmp = tempfile::tempdir().unwrap();
    let big = vec![b'a'; 4096];
    let url = build_bare_repo(
        tmp.path(),
        &[("small.md", b"hi"), ("big.md", big.as_slice())],
    );
    let mut config = GitSourceConfig::new(url);
    config.max_file_bytes = 1024;
    let conn = GitConnector::new(config).unwrap();
    let docs = drain(&conn);
    let refs: Vec<String> = docs.iter().map(|d| d.source_ref.clone()).collect();
    assert!(refs.iter().any(|r| r.ends_with("small.md")));
    assert!(!refs.iter().any(|r| r.ends_with("big.md")), "超 max_file_bytes 跳过");
}

#[test]
fn per_file_marker_changes_when_content_changes() {
    // 增量基石: 同一文件改内容 → marker (SHA-256) 变 → server 层会 re-ingest。
    let tmp1 = tempfile::tempdir().unwrap();
    let url1 = build_bare_repo(tmp1.path(), &[("f.md", b"version one")]);
    let conn1 = GitConnector::new(GitSourceConfig::new(url1)).unwrap();
    let m1 = drain(&conn1)[0].modified_marker.clone().unwrap();

    let tmp2 = tempfile::tempdir().unwrap();
    let url2 = build_bare_repo(tmp2.path(), &[("f.md", b"version two changed")]);
    let conn2 = GitConnector::new(GitSourceConfig::new(url2)).unwrap();
    let m2 = drain(&conn2)[0].modified_marker.clone().unwrap();

    assert_ne!(m1, m2, "内容变 → marker 变 (增量 re-ingest 触发)");
    assert_eq!(m1.len(), 64);
}

#[test]
fn invalid_url_rejected_by_normalize() {
    // 非法 URL 在构造 connector 时即拒 (normalize_url)。
    assert!(GitConnector::new(GitSourceConfig::new("not a url")).is_err());
    assert!(GitConnector::new(GitSourceConfig::new("https://github.com/only-owner")).is_err());
}
