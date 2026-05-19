//! 本地文件夹采集源。遍历目录、按扩展名过滤、把每个文件读成 RawDocument。

use std::path::PathBuf;

use walkdir::WalkDir;

use crate::error::Result;
use crate::ingest::{DocumentSink, RawDocument, SourceConnector, SourceKind};
use crate::parser;

/// 本地文件夹采集源。
pub struct LocalFolderConnector {
    root: PathBuf,
    recursive: bool,
    /// 接受的扩展名（不带点，小写）。空 = 接受全部受支持类型。
    file_types: Vec<String>,
    /// 语料领域（来自 `bound_dirs.corpus_domain`）。`Some(d)` 时回填进每份
    /// `RawDocument.corpus_domain`，驱动 `ingest_document` 的 F-Pro 前缀注入。
    corpus_domain: Option<String>,
}

impl LocalFolderConnector {
    pub fn new(
        root: PathBuf,
        recursive: bool,
        file_types: Vec<String>,
        corpus_domain: Option<String>,
    ) -> Self {
        Self { root, recursive, file_types, corpus_domain }
    }

    /// 扩展名是否被接受。
    fn ext_accepted(&self, path: &std::path::Path) -> bool {
        if self.file_types.is_empty() {
            return parser::is_supported(path);
        }
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        self.file_types
            .iter()
            .any(|t| t.trim_start_matches('.').eq_ignore_ascii_case(&ext))
    }
}

impl SourceConnector for LocalFolderConnector {
    fn source_kind(&self) -> SourceKind {
        SourceKind::LocalFolder
    }

    fn fetch_documents(&self, sink: &mut DocumentSink<'_>) -> Result<()> {
        let walker = if self.recursive {
            WalkDir::new(&self.root)
        } else {
            WalkDir::new(&self.root).max_depth(1)
        };
        for entry in walker.into_iter().filter_map(|e| {
            e.map_err(|err| log::warn!("LocalFolderConnector walk error: {err}")).ok()
        }) {
            let path = entry.path();
            if !path.is_file() || !self.ext_accepted(path) {
                continue;
            }
            // 读字节 + 算 SHA-256 作为增量 marker。单文件读失败不致命。
            let bytes = match std::fs::read(path) {
                Ok(b) => b,
                Err(e) => {
                    log::warn!("LocalFolderConnector: read {} failed: {e}", path.display());
                    continue;
                }
            };
            let marker = {
                use sha2::{Digest, Sha256};
                format!("{:x}", Sha256::digest(&bytes))
            };
            let path_str = path.to_string_lossy().to_string();
            // Windows 路径含反斜杠且缺第三个斜杠，需规范化为 RFC 8089 file URI。
            #[cfg(windows)]
            let uri = format!("file:///{}", path_str.replace('\\', "/"));
            #[cfg(not(windows))]
            let uri = format!("file://{path_str}");
            sink(RawDocument {
                uri,
                title: String::new(),
                content: bytes,
                mime_hint: None,
                source_kind: SourceKind::LocalFolder,
                source_ref: path_str,
                modified_marker: Some(marker),
                // 本地文件夹无来源域 / 用户标签；corpus_domain 从 bound_dir 透传。
                domain: None,
                tags: None,
                corpus_domain: self.corpus_domain.clone(),
                metadata: std::collections::HashMap::new(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn local_connector_enumerates_supported_files() {
        let tmp = TempDir::new().unwrap();
        let mut f1 = std::fs::File::create(tmp.path().join("a.md")).unwrap();
        f1.write_all(b"# A\n\nbody").unwrap();
        let mut f2 = std::fs::File::create(tmp.path().join("b.txt")).unwrap();
        f2.write_all(b"plain text").unwrap();
        std::fs::File::create(tmp.path().join("c.png")).unwrap(); // 不在 file_types 内

        let connector = LocalFolderConnector::new(
            tmp.path().to_path_buf(),
            true,
            vec!["md".into(), "txt".into()],
            Some("legal".into()),
        );
        let mut collected = Vec::new();
        {
            let mut sink: crate::ingest::DocumentSink<'_> = Box::new(|doc| collected.push(doc));
            connector.fetch_documents(&mut sink).unwrap();
        } // sink 在此 drop，释放对 collected 的借用

        assert_eq!(collected.len(), 2, "只应枚举 md + txt，跳过 png");
        for doc in &collected {
            assert_eq!(doc.source_kind, crate::ingest::SourceKind::LocalFolder);
            assert!(doc.modified_marker.is_some(), "本地文件应带 SHA-256 marker");
            assert!(!doc.content.is_empty());
            assert_eq!(doc.corpus_domain.as_deref(), Some("legal"), "corpus_domain 应透传");
            // RFC 8089: file:///path（三斜杠），且不含反斜杠
            assert!(doc.uri.starts_with("file:///"), "URI 应以 file:/// 开头: {}", doc.uri);
            assert!(!doc.uri.contains('\\'), "URI 不应含反斜杠: {}", doc.uri);
        }
    }

    #[test]
    fn local_connector_non_recursive_skips_subdirs() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("top.md"), b"# top").unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("sub").join("nested.md"), b"# nested").unwrap();

        let connector =
            LocalFolderConnector::new(tmp.path().to_path_buf(), false, vec!["md".into()], None);
        let mut count = 0usize;
        {
            let mut sink: crate::ingest::DocumentSink<'_> = Box::new(|_| count += 1);
            connector.fetch_documents(&mut sink).unwrap();
        } // sink drop，释放对 count 的借用
        assert_eq!(count, 1, "non-recursive 只枚举顶层");
    }
}
