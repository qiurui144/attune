//! VLM text-extraction routing for scanned / image documents (spec §3, T-09).
//!
//! Document intelligence operates on TEXT. When a source has no extractable text (a scanned
//! page / a photo of a document), the Vision-role VLM extracts text first; the extracted text
//! then flows through the normal compare / deep_summary / chapters pipeline. A source that
//! already has text skips the VLM entirely (call-count 0) — vision is the cheap-VLM tier per
//! spec §8.2 and must never run on a text doc.

use crate::error::Result;
use crate::vlm::VlmProvider;
use std::path::Path;

/// What kind of source we were handed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocSource {
    /// Already-extracted text (the common case) — no VLM.
    Text(String),
    /// An image / scanned-page file path — needs VLM text extraction.
    Image(std::path::PathBuf),
}

impl DocSource {
    /// Heuristic: a path with an image extension is an image source; everything else is text.
    pub fn from_path(path: &Path, text_if_available: Option<String>) -> Self {
        if let Some(t) = text_if_available {
            if !t.trim().is_empty() {
                return DocSource::Text(t);
            }
        }
        if is_image_path(path) {
            DocSource::Image(path.to_path_buf())
        } else {
            DocSource::Text(String::new())
        }
    }
}

fn is_image_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|s| s.to_ascii_lowercase()).as_deref(),
        Some("png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff")
    )
}

/// Resolve a [`DocSource`] to plain text. Image sources go through the VLM (`caption` used as
/// the text-extraction call); text sources are returned verbatim WITHOUT touching the VLM.
pub fn resolve_text(source: &DocSource, vlm: &dyn VlmProvider) -> Result<String> {
    match source {
        DocSource::Text(t) => Ok(t.clone()),
        DocSource::Image(path) => vlm.caption(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vlm::RecordingMockVlm;
    use std::path::PathBuf;

    #[test]
    fn test_image_doc_routes_to_vision() {
        let vlm = RecordingMockVlm::new("从图片中提取的文本");
        let source = DocSource::Image(PathBuf::from("/tmp/scan.png"));
        let text = resolve_text(&source, &vlm).unwrap();
        assert_eq!(text, "从图片中提取的文本");
        assert!(vlm.total_calls() >= 1, "image source must invoke the VLM");
        assert_eq!(vlm.caption_call_count(), 1);
    }

    #[test]
    fn test_text_doc_skips_vlm() {
        let vlm = RecordingMockVlm::new("should-not-be-used");
        let source = DocSource::Text("已有文本内容".to_string());
        let text = resolve_text(&source, &vlm).unwrap();
        assert_eq!(text, "已有文本内容");
        assert_eq!(vlm.total_calls(), 0, "text source must NOT invoke the VLM");
    }

    #[test]
    fn test_from_path_prefers_existing_text() {
        // Even with an image extension, if text is already available, use the text (no VLM).
        let s = DocSource::from_path(Path::new("/tmp/scan.png"), Some("已抽取文本".into()));
        assert_eq!(s, DocSource::Text("已抽取文本".into()));
    }

    #[test]
    fn test_from_path_image_without_text_is_image() {
        let s = DocSource::from_path(Path::new("/tmp/scan.jpg"), None);
        assert_eq!(s, DocSource::Image(PathBuf::from("/tmp/scan.jpg")));
    }

    #[test]
    fn test_from_path_text_file_is_text() {
        let s = DocSource::from_path(Path::new("/tmp/notes.md"), None);
        assert_eq!(s, DocSource::Text(String::new()));
    }
}
