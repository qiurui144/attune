//! ⭐ Export accuracy gate (spec §9.1 round-trip) — the highest-value test.
//!
//! For each format we render the IR, then **re-parse the produced file with an
//! independent reader** and assert the content equals the IR — proving the export
//! is *accurate*, not merely non-empty:
//!   - xlsx → `calamine` re-reads cells
//!   - csv  → `csv` reader re-reads records
//!   - docx → unzip `word/document.xml`, assert text present
//!   - pdf  → `pdf_extract::extract_text_from_mem`, assert text present
//!
//! The hardest requirement — **Chinese text in PDF must not garble** — is the
//! `pdf_chinese_roundtrip_not_garbled` test: it extracts the PDF text back and
//! asserts the exact CJK strings survive.

use attune_core::export::{Align, Artifact, Block, Document, ExportFormat, Table};
use std::io::{Cursor, Read};

fn device_diff_table() -> Table {
    // The user's literal need: 文档中两设备参数差异 → 输出表格并下载.
    Table {
        title: Some("设备参数差异".into()),
        headers: vec!["参数".into(), "设备A".into(), "设备B".into()],
        rows: vec![
            vec!["电压".into(), "220V".into(), "110V".into()],
            vec!["额定功率".into(), "1000瓦".into(), "1500瓦".into()],
            vec!["重量".into(), "3.2kg".into(), "4.8kg".into()],
        ],
        aligns: vec![Align::Left, Align::Right, Align::Right],
    }
}

// ─────────────────────────────── CSV ────────────────────────────────────────

#[test]
fn csv_roundtrip_exact() {
    let t = device_diff_table();
    let bytes = Artifact::table(t.clone()).render(ExportFormat::Csv).unwrap();

    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(Cursor::new(bytes));
    let records: Vec<csv::StringRecord> = rdr.records().map(|r| r.unwrap()).collect();

    // header + 3 data rows
    assert_eq!(records.len(), t.rows.len() + 1, "csv row count");
    let header: Vec<&str> = records[0].iter().collect();
    assert_eq!(header, vec!["参数", "设备A", "设备B"], "csv header (CJK)");
    for (i, row) in t.rows.iter().enumerate() {
        let got: Vec<&str> = records[i + 1].iter().collect();
        let want: Vec<&str> = row.iter().map(|s| s.as_str()).collect();
        assert_eq!(got, want, "csv data row {i}");
    }
}

#[test]
fn csv_formula_injection_neutralised_on_reparse() {
    let t = Table {
        title: None,
        headers: vec!["formula".into()],
        rows: vec![vec!["=cmd|'/c calc".into()], vec!["@SUM(A1)".into()]],
        aligns: vec![],
    };
    let bytes = Artifact::table(t).render(ExportFormat::Csv).unwrap();
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(Cursor::new(bytes));
    let records: Vec<csv::StringRecord> = rdr.records().map(|r| r.unwrap()).collect();
    // dangerous cells come back prefixed with ' so a spreadsheet won't evaluate them
    assert!(records[1][0].starts_with("'="), "got {:?}", &records[1][0]);
    assert!(records[2][0].starts_with("'@"), "got {:?}", &records[2][0]);
}

// ─────────────────────────────── XLSX ───────────────────────────────────────

#[test]
fn xlsx_roundtrip_exact() {
    use calamine::{Data, Reader, Xlsx};

    let t = device_diff_table();
    let bytes = Artifact::table(t.clone()).render(ExportFormat::Xlsx).unwrap();

    let mut wb: Xlsx<_> = calamine::open_workbook_from_rs(Cursor::new(bytes)).unwrap();
    let sheet_names = wb.sheet_names().to_vec();
    assert_eq!(sheet_names.len(), 1);
    assert_eq!(sheet_names[0], "设备参数差异", "xlsx sheet name (CJK)");

    let range = wb.worksheet_range(&sheet_names[0]).unwrap();
    // header row
    let hdr: Vec<String> = (0..t.ncols())
        .map(|c| match range.get_value((0, c as u32)) {
            Some(Data::String(s)) => s.clone(),
            other => panic!("header cell {c}: {other:?}"),
        })
        .collect();
    assert_eq!(hdr, t.headers, "xlsx header (CJK)");
    // data rows
    for (r, row) in t.rows.iter().enumerate() {
        for (c, want) in row.iter().enumerate() {
            let got = match range.get_value(((r + 1) as u32, c as u32)) {
                Some(Data::String(s)) => s.clone(),
                other => panic!("cell ({r},{c}): {other:?}"),
            };
            assert_eq!(&got, want, "xlsx cell ({r},{c}) (CJK/number)");
        }
    }
}

#[test]
fn xlsx_multi_sheet_document() {
    use calamine::{Reader, Xlsx};
    let d = Document {
        title: None,
        blocks: vec![
            Block::Table(Table {
                title: Some("表一".into()),
                headers: vec!["a".into()],
                rows: vec![vec!["x".into()]],
                aligns: vec![],
            }),
            Block::Table(Table {
                title: Some("表二".into()),
                headers: vec!["b".into()],
                rows: vec![vec!["y".into()]],
                aligns: vec![],
            }),
        ],
    };
    let bytes = Artifact::document(d).render(ExportFormat::Xlsx).unwrap();
    let wb: Xlsx<_> = calamine::open_workbook_from_rs(Cursor::new(bytes)).unwrap();
    let names = wb.sheet_names().to_vec();
    assert_eq!(names, vec!["表一".to_string(), "表二".to_string()]);
}

// ─────────────────────────────── DOCX ───────────────────────────────────────

/// Unzip `word/document.xml` from docx bytes.
fn docx_document_xml(bytes: &[u8]) -> String {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("docx is a zip");
    let mut f = zip
        .by_name("word/document.xml")
        .expect("docx has word/document.xml");
    let mut xml = String::new();
    f.read_to_string(&mut xml).unwrap();
    xml
}

#[test]
fn docx_table_roundtrip_contains_all_cells() {
    let t = device_diff_table();
    let bytes = Artifact::table(t.clone()).render(ExportFormat::Docx).unwrap();
    let xml = docx_document_xml(&bytes);

    // title + every header + every cell value must appear verbatim (incl CJK)
    assert!(xml.contains("设备参数差异"), "docx title");
    for h in &t.headers {
        assert!(xml.contains(h.as_str()), "docx missing header {h}");
    }
    for row in &t.rows {
        for cell in row {
            assert!(xml.contains(cell.as_str()), "docx missing cell {cell}");
        }
    }
}

#[test]
fn docx_document_roundtrip_blocks_present() {
    let d = Document {
        title: Some("标书参考文档".into()),
        blocks: vec![
            Block::Heading {
                level: 2,
                text: "项目背景".into(),
            },
            Block::Paragraph {
                text: "本项目旨在交付一套可下载的报告。".into(),
            },
            Block::List {
                ordered: true,
                items: vec!["第一项要求".into(), "第二项要求".into()],
            },
            Block::Table(device_diff_table()),
        ],
    };
    let bytes = Artifact::document(d).render(ExportFormat::Docx).unwrap();
    let xml = docx_document_xml(&bytes);
    for needle in [
        "标书参考文档",
        "项目背景",
        "本项目旨在交付一套可下载的报告。",
        "第一项要求",
        "第二项要求",
        "电压",
    ] {
        assert!(xml.contains(needle), "docx missing {needle}");
    }
}

// ─────────────────────────────── PDF ────────────────────────────────────────

/// ⭐ The top accuracy requirement: Chinese text in the PDF must survive a
/// text-extraction round-trip un-garbled. Powered by the embedded CJK font.
#[test]
fn pdf_chinese_roundtrip_not_garbled() {
    let d = Document {
        title: Some("设备参数差异报告".into()),
        blocks: vec![
            Block::Paragraph {
                text: "下面是两台设备的关键参数对比。".into(),
            },
            Block::Table(device_diff_table()),
        ],
    };
    let bytes = Artifact::document(d).render(ExportFormat::Pdf).unwrap();
    assert!(
        bytes.starts_with(b"%PDF"),
        "output is not a PDF (magic bytes)"
    );

    let text = pdf_extract::extract_text_from_mem(&bytes).expect("pdf-extract re-parse");
    let normalized: String = text.chars().filter(|c| !c.is_whitespace()).collect();

    // Each Chinese string from the IR must be readable back from the rendered PDF.
    for needle in [
        "设备参数差异报告",
        "下面是两台设备的关键参数对比",
        "电压",
        "额定功率",
        "重量",
        "设备A",
        "设备B",
    ] {
        let key: String = needle.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            normalized.contains(&key),
            "PDF round-trip lost/garbled Chinese {needle:?}; extracted (normalized) = {normalized:?}"
        );
    }
    // numbers survive too
    assert!(normalized.contains("220V") && normalized.contains("1500瓦"));
}

#[test]
fn pdf_table_roundtrip_values_present() {
    let t = device_diff_table();
    let bytes = Artifact::table(t.clone()).render(ExportFormat::Pdf).unwrap();
    let text = pdf_extract::extract_text_from_mem(&bytes).unwrap();
    let normalized: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    for h in &t.headers {
        let key: String = h.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(normalized.contains(&key), "pdf missing header {h}");
    }
    for row in &t.rows {
        for cell in row {
            let key: String = cell.chars().filter(|c| !c.is_whitespace()).collect();
            assert!(normalized.contains(&key), "pdf missing cell {cell}");
        }
    }
}

// ─────────────────────────────── MD ─────────────────────────────────────────

#[test]
fn md_table_roundtrip_structure() {
    let t = device_diff_table();
    let s = String::from_utf8(Artifact::table(t.clone()).render(ExportFormat::Md).unwrap()).unwrap();
    // GFM header separator row present + alignment markers honoured (right cols)
    assert!(s.contains("| 参数 | 设备A | 设备B |"), "md header row:\n{s}");
    assert!(s.contains("---:"), "md right-align marker:\n{s}");
    for row in &t.rows {
        let line = format!("| {} |", row.join(" | "));
        assert!(s.contains(&line), "md missing row {line}\n{s}");
    }
}
