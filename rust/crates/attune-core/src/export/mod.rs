//! Artifact export engine — OSS attune deterministic, **zero-cost** delivery layer.
//!
//! User need (原话): 文档参数差异 → "**输出表格并下载**";参考标书 →
//! "**可直接下载的 doc 或 pdf**";要求"**准确**输出"。This module turns a stable
//! [`Artifact`] intermediate representation (IR) into downloadable office files:
//! **xlsx / csv / md / docx / pdf**. Every renderer is **pure Rust** (no system
//! binary, no LLM), so export is **tier-🆓 zero-cost** (CLAUDE.md §"成本感知与触发契约").
//!
//! ## Why an IR?
//!
//! Producers (a doc-diff table, a writing draft, a knowledge synthesis) emit one
//! stable [`Artifact`]; every output format renders from that single IR. A new
//! format is one new renderer, not N×M producer wiring. The IR is a serde JSON
//! contract — callers may POST raw IR JSON to the export endpoint.
//!
//! ## Accuracy is the quality gate (spec §9.1 round-trip)
//!
//! "准确" is verified by **round-trip**: render → re-parse the produced file →
//! assert content equals the IR. xlsx is re-read with `calamine`, csv with the
//! `csv` reader, docx by unzipping `word/document.xml`, pdf with `pdf-extract`.
//! The hardest case — **Chinese text in PDF must not garble** — is guaranteed by
//! embedding a CJK subset font (`assets/fonts/AttuneCJK-subset.ttf`) via
//! `include_bytes!` into the typst renderer (spec R1, the top export risk).
//!
//! ## Cost contract — compile-time no-LLM guard
//!
//! This module **does not import** [`crate::llm::LlmProvider`]; the `cost_tier()`
//! of every [`ExportFormat`] is `Free`, enforced by a proptest in [`tests`]. Export
//! can never silently escalate to a paid tier.

pub mod render;
pub mod sanitize;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Stable export-cost tier. Export is **always** [`CostTier::Free`] — there is no
/// code path that makes it anything else (see the anti-escalation proptest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CostTier {
    /// CPU, milliseconds, "随便跑". The only tier export ever uses.
    Free,
}

/// Output formats the engine can render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    /// Excel 2007+ workbook (`rust_xlsxwriter`). Tables only.
    Xlsx,
    /// RFC-4180 CSV (`csv`). Tables only.
    Csv,
    /// GitHub-flavoured Markdown (internal renderer). Tables and documents.
    Md,
    /// Word document (`docx-rs`). Documents and tables.
    Docx,
    /// PDF via typst (`typst-as-lib`) with an embedded CJK font. Documents and tables.
    Pdf,
}

impl ExportFormat {
    /// Export never costs money or local accelerator time.
    pub const fn cost_tier(self) -> CostTier {
        CostTier::Free
    }

    /// Canonical file extension (no dot).
    pub const fn extension(self) -> &'static str {
        match self {
            ExportFormat::Xlsx => "xlsx",
            ExportFormat::Csv => "csv",
            ExportFormat::Md => "md",
            ExportFormat::Docx => "docx",
            ExportFormat::Pdf => "pdf",
        }
    }

    /// MIME type for the HTTP `Content-Type` download header.
    pub const fn mime(self) -> &'static str {
        match self {
            ExportFormat::Xlsx => {
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            }
            ExportFormat::Csv => "text/csv; charset=utf-8",
            ExportFormat::Md => "text/markdown; charset=utf-8",
            ExportFormat::Docx => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            }
            ExportFormat::Pdf => "application/pdf",
        }
    }

    /// Parse a format from a lowercase string (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "xlsx" | "excel" => Some(ExportFormat::Xlsx),
            "csv" => Some(ExportFormat::Csv),
            "md" | "markdown" => Some(ExportFormat::Md),
            "docx" | "doc" | "word" => Some(ExportFormat::Docx),
            "pdf" => Some(ExportFormat::Pdf),
            _ => None,
        }
    }

    /// Whether this format can render a table artifact.
    pub const fn supports_table(self) -> bool {
        true // every format renders tables
    }

    /// Whether this format can render a multi-block document artifact.
    pub const fn supports_document(self) -> bool {
        // csv/xlsx are tabular-only; a document is flattened to its tables there
        // only if it contains exactly one table (handled per-renderer). For the
        // capability advertised to callers, md/docx/pdf are the document formats.
        matches!(
            self,
            ExportFormat::Md | ExportFormat::Docx | ExportFormat::Pdf
        )
    }
}

/// Column horizontal alignment for table cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// A table artifact: header row + data rows, with optional per-column alignment.
///
/// Invariant (checked by [`Table::validate`]): every data row has exactly
/// `headers.len()` cells, and `aligns` (if non-empty) has `headers.len()` entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    /// Optional table title (used as sheet name / caption / heading).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Header cells (column count is defined by this).
    pub headers: Vec<String>,
    /// Data rows; each must have `headers.len()` cells.
    pub rows: Vec<Vec<String>>,
    /// Per-column alignment. Empty = all left. Otherwise length must equal headers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aligns: Vec<Align>,
}

impl Table {
    /// Number of columns.
    pub fn ncols(&self) -> usize {
        self.headers.len()
    }

    /// Alignment for column `i` (default left if unspecified).
    pub fn align_for(&self, i: usize) -> Align {
        self.aligns.get(i).copied().unwrap_or_default()
    }

    /// Validate the table's shape invariant.
    pub fn validate(&self) -> Result<(), ExportError> {
        if self.headers.is_empty() {
            return Err(ExportError::MalformedIr(
                "table has zero columns (empty headers)".into(),
            ));
        }
        let n = self.headers.len();
        for (i, row) in self.rows.iter().enumerate() {
            if row.len() != n {
                return Err(ExportError::MalformedIr(format!(
                    "table row {i} has {} cells, expected {n}",
                    row.len()
                )));
            }
        }
        if !self.aligns.is_empty() && self.aligns.len() != n {
            return Err(ExportError::MalformedIr(format!(
                "table aligns has {} entries, expected {n}",
                self.aligns.len()
            )));
        }
        Ok(())
    }
}

/// A block inside a [`Document`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    /// A section heading. `level` is 1..=6 (clamped on render).
    Heading { level: u8, text: String },
    /// A plain paragraph.
    Paragraph { text: String },
    /// An ordered or unordered list.
    List {
        #[serde(default)]
        ordered: bool,
        items: Vec<String>,
    },
    /// An embedded table.
    Table(Table),
}

/// A document artifact: title + ordered blocks (headings, paragraphs, lists, tables).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    /// Document title (rendered as H1 / docx title / pdf title).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Ordered content blocks.
    pub blocks: Vec<Block>,
}

impl Document {
    /// Validate every embedded table.
    pub fn validate(&self) -> Result<(), ExportError> {
        for b in &self.blocks {
            if let Block::Table(t) = b {
                t.validate()?;
            }
        }
        Ok(())
    }

    /// Collect the tables in document order (used when flattening to xlsx/csv).
    pub fn tables(&self) -> Vec<&Table> {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Table(t) => Some(t),
                _ => None,
            })
            .collect()
    }
}

/// The stable export IR. Either a single [`Table`] or a multi-block [`Document`].
///
/// Wire shape (adjacently tagged for a clean REST contract):
/// `{ "type": "table", "data": { …Table… } }` or
/// `{ "type": "document", "data": { …Document… } }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Artifact {
    Table(Table),
    Document(Document),
}

impl Artifact {
    /// Validate the IR shape; the first step of every render.
    pub fn validate(&self) -> Result<(), ExportError> {
        match self {
            Artifact::Table(t) => t.validate(),
            Artifact::Document(d) => d.validate(),
        }
    }

    /// A human title used to derive the download filename.
    pub fn title(&self) -> Option<&str> {
        match self {
            Artifact::Table(t) => t.title.as_deref(),
            Artifact::Document(d) => d.title.as_deref(),
        }
    }

    /// Render this artifact to the requested format, returning raw file bytes.
    ///
    /// The single entry point used by the HTTP route and the CLI.
    pub fn render(&self, format: ExportFormat) -> Result<Vec<u8>, ExportError> {
        self.validate()?;
        render::render(self, format)
    }

    /// A small builder: wrap a table.
    pub fn table(table: Table) -> Self {
        Artifact::Table(table)
    }

    /// A small builder: wrap a document.
    pub fn document(doc: Document) -> Self {
        Artifact::Document(doc)
    }
}

/// Errors the export engine can return. Each has a stable kebab `code()` (spec §7).
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// The IR failed its shape invariant (ragged rows, zero columns, …).
    #[error("malformed export IR: {0}")]
    MalformedIr(String),
    /// The requested format cannot render this artifact kind
    /// (e.g. a multi-table document to csv).
    #[error("format does not support this artifact: {0}")]
    UnsupportedArtifact(String),
    /// A renderer backend (xlsx/docx/typst) failed internally.
    #[error("render backend failed: {0}")]
    RenderFailed(String),
}

impl ExportError {
    /// Stable kebab error code for the HTTP `{ "code": … }` shape.
    pub fn code(&self) -> &'static str {
        match self {
            ExportError::MalformedIr(_) => "malformed-ir",
            ExportError::UnsupportedArtifact(_) => "unsupported-artifact",
            ExportError::RenderFailed(_) => "render-failed",
        }
    }
}

/// Map well-known export error codes to HTTP status (kept here so the route layer
/// stays a thin adapter). 400 for caller-fixable, 500 for backend failure.
pub fn http_status_for(code: &str) -> u16 {
    match code {
        "malformed-ir" | "unsupported-artifact" => 400,
        _ => 500,
    }
}

/// Re-export the helper used by callers that hold extra per-export metadata.
pub type Meta = BTreeMap<String, String>;
