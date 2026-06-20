//! Integration tests for the skills orchestration runtime + the `compare_to_table` skill
//! (spec §9, 用户例 3). The headline guarantee (spec §9.1 + the task brief): **真喂一个含两设备
//! 参数的文档 → 得到正确 xlsx**, verified by re-reading the produced workbook with `calamine` and
//! asserting every cell equals the independently-computed ground truth (round-trip accuracy).
//!
//! Six §6.1 categories are covered here + golden ≥ 10 (the `GOLDEN` table) + the end-to-end
//! xlsx round-trip. The `compare_to_table` unit-level §4.5 behaviour (schema/retry/grounding/
//! degrade) is in the attune-core lib tests; this file is the **orchestration + export** E2E.

use std::collections::BTreeMap;
use std::io::Cursor;

use attune_core::export::{Artifact, ExportFormat};
use attune_core::llm::LlmProvider;
use attune_core::skill_runtime::{parse_skill_yaml, run_skill, MapResolver, SkillError};
use attune_core::usage::TokenUsage;
use serde_json::json;

const SKILL_YAML: &str = include_str!("../assets/skills/compare-to-table.yaml");

/// An LLM mock that always returns a fixed canned JSON response (the "correct" extraction for a
/// golden case). Independent of the agent's own logic — the ground truth is the test's, not the
/// agent's (CLAUDE.md Agent 验证铁律: GT must not be agent-generated).
struct ScriptedLlm(String);

impl LlmProvider for ScriptedLlm {
    fn chat(&self, _system: &str, _user: &str) -> attune_core::error::Result<(String, TokenUsage)> {
        Ok((self.0.clone(), TokenUsage::empty("scripted", "test-model")))
    }
    fn chat_with_history(
        &self,
        _messages: &[attune_core::llm::ChatMessage],
    ) -> attune_core::error::Result<(String, TokenUsage)> {
        Ok((self.0.clone(), TokenUsage::empty("scripted", "test-model")))
    }
    fn is_available(&self) -> bool {
        true
    }
    fn model_name(&self) -> &str {
        "test-model"
    }
}

fn resolver(doc: &str) -> MapResolver {
    let mut m = BTreeMap::new();
    m.insert("item-1".to_string(), doc.to_string());
    MapResolver(m)
}

/// Run the built-in compare-to-table skill end-to-end and return the produced xlsx bytes + the IR.
fn run_skill_xlsx(doc: &str, entities: [&str; 2], llm_json: &str) -> (Vec<u8>, Artifact) {
    let skill = parse_skill_yaml(SKILL_YAML).expect("built-in skill parses");
    let llm = ScriptedLlm(llm_json.to_string());
    let inputs = json!({ "doc": "item-1", "entities": [entities[0], entities[1]] });
    let res = run_skill(&skill, &inputs, true, &resolver(doc), &llm, "test-model")
        .expect("skill run ok");
    assert_eq!(res.format, ExportFormat::Xlsx);
    (res.artifact_bytes, res.artifact)
}

/// Re-read an xlsx into a Vec<Vec<String>> (header + data rows) with calamine — the round-trip oracle.
fn read_xlsx(bytes: &[u8]) -> Vec<Vec<String>> {
    use calamine::{Data, Reader, Xlsx};
    let mut wb: Xlsx<_> = calamine::open_workbook_from_rs(Cursor::new(bytes.to_vec())).unwrap();
    let name = wb.sheet_names()[0].clone();
    let range = wb.worksheet_range(&name).unwrap();
    let mut out = Vec::new();
    for r in 0..range.height() {
        let mut row = Vec::new();
        for c in 0..range.width() {
            let v = match range.get_value((r as u32, c as u32)) {
                Some(Data::String(s)) => s.clone(),
                Some(Data::Int(i)) => i.to_string(),
                Some(Data::Float(f)) => {
                    if f.fract() == 0.0 { (*f as i64).to_string() } else { f.to_string() }
                }
                Some(Data::Empty) | None => String::new(),
                other => format!("{other:?}"),
            };
            row.push(v);
        }
        out.push(row);
    }
    out
}

// ───────────────────── golden set (≥10 + 1 sentinel) ─────────────────────
//
// Each case: a real device-spec document, two entities, the model's (correct) JSON extraction,
// and the INDEPENDENTLY-computed expected table (param / A / B / 差异). The diff column ground
// truth is computed in the test body (`expect_diffs`), not taken from the agent.

struct Golden {
    name: &'static str,
    doc: &'static str,
    entities: [&'static str; 2],
    llm_json: &'static str,
    /// Expected (param, a, b) rows; the 差异 column is derived by the test (a != b).
    expect: &'static [(&'static str, &'static str, &'static str)],
}

const ABSENT: &str = "（未提供）";

const GOLDEN: &[Golden] = &[
    Golden {
        name: "01-camera-resolution-power",
        doc: "本文比较两款摄像头。设备 A：分辨率 1080p，功耗 5W，接口 USB。\
              设备 B：分辨率 4K，功耗 12W，接口 USB。",
        entities: ["设备 A", "设备 B"],
        llm_json: r#"{"rows":[{"name":"分辨率","value_a":"1080p","value_b":"4K"},{"name":"功耗","value_a":"5W","value_b":"12W"},{"name":"接口","value_a":"USB","value_b":"USB"}]}"#,
        expect: &[("分辨率", "1080p", "4K"), ("功耗", "5W", "12W"), ("接口", "USB", "USB")],
    },
    Golden {
        name: "02-laptop-cpu-ram",
        doc: "型号 X1：CPU i5-1240P，内存 16GB，重量 1.2kg。型号 X2：CPU i7-1260P，内存 16GB，重量 1.4kg。",
        entities: ["型号 X1", "型号 X2"],
        llm_json: r#"{"rows":[{"name":"CPU","value_a":"i5-1240P","value_b":"i7-1260P"},{"name":"内存","value_a":"16GB","value_b":"16GB"},{"name":"重量","value_a":"1.2kg","value_b":"1.4kg"}]}"#,
        expect: &[("CPU", "i5-1240P", "i7-1260P"), ("内存", "16GB", "16GB"), ("重量", "1.2kg", "1.4kg")],
    },
    Golden {
        name: "03-english-spec",
        doc: "Drive A: capacity 512GB, speed 3500MB/s, warranty 5y. Drive B: capacity 1TB, speed 7000MB/s, warranty 5y.",
        entities: ["Drive A", "Drive B"],
        llm_json: r#"{"rows":[{"name":"capacity","value_a":"512GB","value_b":"1TB"},{"name":"speed","value_a":"3500MB/s","value_b":"7000MB/s"},{"name":"warranty","value_a":"5y","value_b":"5y"}]}"#,
        expect: &[("capacity", "512GB", "1TB"), ("speed", "3500MB/s", "7000MB/s"), ("warranty", "5y", "5y")],
    },
    Golden {
        name: "04-router-all-differ",
        doc: "路由 R1：频段 2.4GHz，端口 4，速率 300Mbps。路由 R2：频段 5GHz，端口 8，速率 1200Mbps。",
        entities: ["路由 R1", "路由 R2"],
        llm_json: r#"{"rows":[{"name":"频段","value_a":"2.4GHz","value_b":"5GHz"},{"name":"端口","value_a":"4","value_b":"8"},{"name":"速率","value_a":"300Mbps","value_b":"1200Mbps"}]}"#,
        expect: &[("频段", "2.4GHz", "5GHz"), ("端口", "4", "8"), ("速率", "300Mbps", "1200Mbps")],
    },
    Golden {
        name: "05-monitor-none-differ",
        doc: "显示器甲：尺寸 27 寸，刷新率 144Hz。显示器乙：尺寸 27 寸，刷新率 144Hz。",
        entities: ["显示器甲", "显示器乙"],
        llm_json: r#"{"rows":[{"name":"尺寸","value_a":"27 寸","value_b":"27 寸"},{"name":"刷新率","value_a":"144Hz","value_b":"144Hz"}]}"#,
        expect: &[("尺寸", "27 寸", "27 寸"), ("刷新率", "144Hz", "144Hz")],
    },
    Golden {
        name: "06-phone-absent-on-one-side",
        doc: "机型甲：屏幕 6.1 寸，电池 3200mAh。机型乙：屏幕 6.7 寸，电池 4500mAh，防水 IP68。",
        entities: ["机型甲", "机型乙"],
        llm_json: r#"{"rows":[{"name":"屏幕","value_a":"6.1 寸","value_b":"6.7 寸"},{"name":"电池","value_a":"3200mAh","value_b":"4500mAh"},{"name":"防水","value_a":null,"value_b":"IP68"}]}"#,
        expect: &[("屏幕", "6.1 寸", "6.7 寸"), ("电池", "3200mAh", "4500mAh"), ("防水", ABSENT, "IP68")],
    },
    Golden {
        name: "07-printer-numbers",
        doc: "打印机 P1：速度 20ppm，分辨率 600dpi，价格 1299。打印机 P2：速度 35ppm，分辨率 1200dpi，价格 2599。",
        entities: ["打印机 P1", "打印机 P2"],
        llm_json: r#"{"rows":[{"name":"速度","value_a":"20ppm","value_b":"35ppm"},{"name":"分辨率","value_a":"600dpi","value_b":"1200dpi"},{"name":"价格","value_a":"1299","value_b":"2599"}]}"#,
        expect: &[("速度", "20ppm", "35ppm"), ("分辨率", "600dpi", "1200dpi"), ("价格", "1299", "2599")],
    },
    Golden {
        name: "08-traditional-chinese",
        doc: "設備甲：解析度 1080p，功率 5瓦。設備乙：解析度 4K，功率 12瓦。",
        entities: ["設備甲", "設備乙"],
        llm_json: r#"{"rows":[{"name":"解析度","value_a":"1080p","value_b":"4K"},{"name":"功率","value_a":"5瓦","value_b":"12瓦"}]}"#,
        expect: &[("解析度", "1080p", "4K"), ("功率", "5瓦", "12瓦")],
    },
    Golden {
        name: "09-mixed-cjk-ascii-emoji",
        doc: "Model 甲🔋: battery 5000mAh, color 黑. Model 乙🔋: battery 4000mAh, color 白.",
        entities: ["Model 甲", "Model 乙"],
        llm_json: r#"{"rows":[{"name":"battery","value_a":"5000mAh","value_b":"4000mAh"},{"name":"color","value_a":"黑","value_b":"白"}]}"#,
        expect: &[("battery", "5000mAh", "4000mAh"), ("color", "黑", "白")],
    },
    Golden {
        name: "10-single-param",
        doc: "甲方案：周期 7 天。乙方案：周期 14 天。",
        entities: ["甲方案", "乙方案"],
        llm_json: r#"{"rows":[{"name":"周期","value_a":"7 天","value_b":"14 天"}]}"#,
        expect: &[("周期", "7 天", "14 天")],
    },
    Golden {
        name: "11-many-params",
        doc: "服务器 S1：CPU 16 核，内存 64GB，硬盘 2TB，网卡 10G，电源 800W，机架 2U。\
              服务器 S2：CPU 32 核，内存 128GB，硬盘 4TB，网卡 25G，电源 1200W，机架 2U。",
        entities: ["服务器 S1", "服务器 S2"],
        llm_json: r#"{"rows":[{"name":"CPU","value_a":"16 核","value_b":"32 核"},{"name":"内存","value_a":"64GB","value_b":"128GB"},{"name":"硬盘","value_a":"2TB","value_b":"4TB"},{"name":"网卡","value_a":"10G","value_b":"25G"},{"name":"电源","value_a":"800W","value_b":"1200W"},{"name":"机架","value_a":"2U","value_b":"2U"}]}"#,
        expect: &[("CPU", "16 核", "32 核"), ("内存", "64GB", "128GB"), ("硬盘", "2TB", "4TB"), ("网卡", "10G", "25G"), ("电源", "800W", "1200W"), ("机架", "2U", "2U")],
    },
    // SENTINEL: model hallucinates a value ("8K") not in the doc → grounding guard must null it
    // (the row's B cell must be （未提供）, never the fabricated 8K).
    Golden {
        name: "12-sentinel-hallucination-grounded-out",
        doc: "设备 A：分辨率 1080p。设备 B：分辨率 4K。",
        entities: ["设备 A", "设备 B"],
        llm_json: r#"{"rows":[{"name":"分辨率","value_a":"1080p","value_b":"8K"}]}"#,
        // 8K is NOT in the doc → dropped to absent (sentinel: anti-fabrication).
        expect: &[("分辨率", "1080p", ABSENT)],
    },
];

#[test]
fn golden_end_to_end_xlsx_accuracy() {
    assert!(GOLDEN.len() >= 10, "golden set must have ≥10 cases (+ sentinel)");
    for g in GOLDEN {
        let (bytes, _ir) = run_skill_xlsx(g.doc, g.entities, g.llm_json);
        // round-trip: re-read the produced xlsx and assert cell-exact content.
        let sheet = read_xlsx(&bytes);
        // header: 参数 | <A> | <B> | 差异
        assert_eq!(
            sheet[0],
            vec!["参数", g.entities[0], g.entities[1], "差异"],
            "[{}] header", g.name
        );
        assert_eq!(sheet.len() - 1, g.expect.len(), "[{}] row count", g.name);
        for (i, (param, a, b)) in g.expect.iter().enumerate() {
            let row = &sheet[i + 1];
            assert_eq!(&row[0], param, "[{}] row {i} param", g.name);
            assert_eq!(&row[1], a, "[{}] row {i} A value", g.name);
            assert_eq!(&row[2], b, "[{}] row {i} B value", g.name);
            // independently-computed diff ground truth: differ iff both present and unequal.
            let want_diff = *a != ABSENT && *b != ABSENT && a != b;
            let got_diff = row[3] == "✓";
            assert_eq!(got_diff, want_diff, "[{}] row {i} ({param}) diff mark", g.name);
        }
    }
}

// ───────────────────── §6.1: edge cases (≥5) ─────────────────────

#[test]
fn edge_empty_extraction_yields_headers_only_xlsx() {
    let (bytes, _) = run_skill_xlsx("设备 A 与设备 B 的简介。", ["设备 A", "设备 B"], r#"{"rows":[]}"#);
    let sheet = read_xlsx(&bytes);
    assert_eq!(sheet.len(), 1, "headers-only when nothing extracted");
    assert_eq!(sheet[0], vec!["参数", "设备 A", "设备 B", "差异"]);
}

#[test]
fn edge_csv_format_also_works() {
    // The skill defaults to xlsx, but export to csv via /export of the same IR must also round-trip.
    let (_b, ir) = run_skill_xlsx("甲 5W 乙 12W", ["甲", "乙"], r#"{"rows":[{"name":"功耗","value_a":"5W","value_b":"12W"}]}"#);
    let csv = ir.render(ExportFormat::Csv).unwrap();
    let text = String::from_utf8(csv).unwrap();
    assert!(text.contains("功耗"));
    assert!(text.contains("5W") && text.contains("12W"));
}

#[test]
fn edge_md_format_renders_table() {
    let (_b, ir) = run_skill_xlsx("甲 5W 乙 12W", ["甲", "乙"], r#"{"rows":[{"name":"功耗","value_a":"5W","value_b":"12W"}]}"#);
    let md = String::from_utf8(ir.render(ExportFormat::Md).unwrap()).unwrap();
    assert!(md.contains("| 参数 |") || md.contains("|参数|") || md.contains("参数"));
    assert!(md.contains("功耗"));
}

#[test]
fn edge_unicode_cjk_emoji_survive_xlsx() {
    let (bytes, _) = run_skill_xlsx(
        "甲🔋 颜色 红 乙🔋 颜色 蓝",
        ["甲", "乙"],
        r#"{"rows":[{"name":"颜色","value_a":"红","value_b":"蓝"}]}"#,
    );
    let sheet = read_xlsx(&bytes);
    assert_eq!(sheet[1][1], "红");
    assert_eq!(sheet[1][2], "蓝");
}

#[test]
fn edge_wide_value_strings_preserved() {
    let long_a = "超长参数值".repeat(40); // 200 CJK chars
    let doc = format!("甲 描述 {long_a} 乙 描述 短");
    let json = format!(r#"{{"rows":[{{"name":"描述","value_a":"{long_a}","value_b":"短"}}]}}"#);
    let (bytes, _) = run_skill_xlsx(&doc, ["甲", "乙"], &json);
    let sheet = read_xlsx(&bytes);
    assert_eq!(sheet[1][1], long_a, "wide cell preserved verbatim");
}

// ───────────────────── §6.1: error cases (≥3) ─────────────────────

#[test]
fn error_missing_required_input() {
    let skill = parse_skill_yaml(SKILL_YAML).unwrap();
    let llm = ScriptedLlm(r#"{"rows":[]}"#.into());
    let inputs = json!({ "entities": ["A", "B"] }); // no `doc`
    let err = run_skill(&skill, &inputs, true, &resolver("x"), &llm, "m").unwrap_err();
    assert!(matches!(err, SkillError::InputInvalid(_)));
    assert_eq!(err.code(), "input-invalid");
}

#[test]
fn error_cost_not_confirmed() {
    let skill = parse_skill_yaml(SKILL_YAML).unwrap();
    let llm = ScriptedLlm(r#"{"rows":[]}"#.into());
    let inputs = json!({ "doc": "item-1", "entities": ["A", "B"] });
    let err = run_skill(&skill, &inputs, false, &resolver("x"), &llm, "m").unwrap_err();
    assert_eq!(err.code(), "cost-not-confirmed");
}

#[test]
fn error_wrong_input_type() {
    let skill = parse_skill_yaml(SKILL_YAML).unwrap();
    let llm = ScriptedLlm(r#"{"rows":[]}"#.into());
    // entities should be a string_list, not a string.
    let inputs = json!({ "doc": "item-1", "entities": "not-a-list" });
    let err = run_skill(&skill, &inputs, true, &resolver("x"), &llm, "m").unwrap_err();
    assert_eq!(err.code(), "input-invalid");
}

// ───────────────────── §6.1: adversarial (≥3) ─────────────────────

#[test]
fn adversarial_csv_formula_injection_neutralized() {
    // A param value that looks like a spreadsheet formula must be neutralized in csv export
    // (leading '=' / '@' / '+' / '-' guarded by the export sanitizer). The value IS in the doc.
    let doc = "甲 公式 =cmd|'/c calc'!A1 乙 公式 安全值";
    let json = r#"{"rows":[{"name":"公式","value_a":"=cmd|'/c calc'!A1","value_b":"安全值"}]}"#;
    let (_b, ir) = run_skill_xlsx(doc, ["甲", "乙"], json);
    let csv = String::from_utf8(ir.render(ExportFormat::Csv).unwrap()).unwrap();
    // The dangerous cell must be prefixed with a single quote so a spreadsheet does not execute it.
    assert!(csv.contains("'=cmd") || csv.contains("\"'=cmd"), "formula-injection neutralized: {csv}");
}

#[test]
fn adversarial_ungrounded_value_cannot_reach_table() {
    // The model tries to inject a value NOT present in the source → grounding guard drops it.
    // (Re-validated end-to-end through the skill, not just the unit.)
    let (bytes, _) = run_skill_xlsx(
        "设备 A：分辨率 1080p。设备 B：分辨率 4K。",
        ["设备 A", "设备 B"],
        r#"{"rows":[{"name":"分辨率","value_a":"1080p","value_b":"幻觉8K不在文档"}]}"#,
    );
    let sheet = read_xlsx(&bytes);
    assert_eq!(sheet[1][2], ABSENT, "ungrounded value must be （未提供）, never the fabrication");
    assert_eq!(sheet[1][3], "", "one side absent → not flagged as a diff");
}

#[test]
fn adversarial_unknown_agent_capability_rejected() {
    // A skill referencing a non-existent agent must error (typo not silently no-op'd).
    let yaml = SKILL_YAML.replace(
        "document_intelligence.compare_to_table",
        "document_intelligence.evil_unknown",
    );
    let skill = parse_skill_yaml(&yaml).unwrap();
    let llm = ScriptedLlm(r#"{"rows":[]}"#.into());
    let inputs = json!({ "doc": "item-1", "entities": ["A", "B"] });
    let err = run_skill(&skill, &inputs, true, &resolver("x"), &llm, "m").unwrap_err();
    assert_eq!(err.code(), "partial-failure");
}

// ───────────────────── §6.1: concurrent (≥1) ─────────────────────

#[test]
fn concurrent_runs_produce_independent_artifacts() {
    use std::thread;
    let handles: Vec<_> = (0..8)
        .map(|i| {
            thread::spawn(move || {
                let doc = format!("甲 序号 {i} 乙 序号 {}", i + 100);
                let json = format!(
                    r#"{{"rows":[{{"name":"序号","value_a":"{i}","value_b":"{}"}}]}}"#,
                    i + 100
                );
                let (bytes, _) = run_skill_xlsx(&doc, ["甲", "乙"], &json);
                let sheet = read_xlsx(&bytes);
                (i, sheet[1][1].clone(), sheet[1][2].clone())
            })
        })
        .collect();
    for h in handles {
        let (i, a, b) = h.join().unwrap();
        // each run's artifact carries ITS OWN data — no cross-talk between concurrent runs.
        assert_eq!(a, i.to_string());
        assert_eq!(b, (i + 100).to_string());
    }
}

// ───────────────────── §6.1: resource-exhaust (≥1) ─────────────────────

#[test]
fn resource_cost_cap_aborts_runaway() {
    // A model that returns a gigantic response would blow the token budget. We simulate by
    // checking the skill-level cap is wired: a synthetic bill over MAX_TOTAL_TOKENS aborts.
    // (Unit-level cap math is in cost.rs; here we prove the runner enforces it via a huge response.)
    let huge_value = "x".repeat(400_000); // ~133K est tokens worth of output
    let doc = format!("甲 巨值 {huge_value} 乙 巨值 短");
    let json = format!(r#"{{"rows":[{{"name":"巨值","value_a":"{huge_value}","value_b":"短"}}]}}"#);
    let skill = parse_skill_yaml(SKILL_YAML).unwrap();
    let llm = ScriptedLlm(json);
    let inputs = json!({ "doc": "item-1", "entities": ["甲", "乙"] });
    let res = run_skill(&skill, &inputs, true, &resolver(&doc), &llm, "m");
    // The cap (50K) must trip on this oversized output.
    match res {
        Err(SkillError::CostCapExceeded { used, cap }) => {
            assert!(used > cap, "used {used} must exceed cap {cap}");
        }
        other => panic!("expected CostCapExceeded, got {other:?}"),
    }
}

// ───────────────────── §6.1: i18n (≥2) ─────────────────────

#[test]
fn i18n_traditional_chinese_roundtrips() {
    let (bytes, _) = run_skill_xlsx(
        "設備甲：功率 5瓦。設備乙：功率 12瓦。",
        ["設備甲", "設備乙"],
        r#"{"rows":[{"name":"功率","value_a":"5瓦","value_b":"12瓦"}]}"#,
    );
    let sheet = read_xlsx(&bytes);
    assert_eq!(sheet[0][1], "設備甲");
    assert_eq!(sheet[1], vec!["功率", "5瓦", "12瓦", "✓"]);
}

#[test]
fn i18n_english_roundtrips() {
    let (bytes, _) = run_skill_xlsx(
        "Drive A capacity 512GB. Drive B capacity 1TB.",
        ["Drive A", "Drive B"],
        r#"{"rows":[{"name":"capacity","value_a":"512GB","value_b":"1TB"}]}"#,
    );
    let sheet = read_xlsx(&bytes);
    assert_eq!(sheet[1], vec!["capacity", "512GB", "1TB", "✓"]);
}

// ───────────────────── §6.1: degrade (≥1) ─────────────────────

#[test]
fn degrade_llm_garbage_still_downloads() {
    // The model returns non-JSON. compare_to_table degrades to empty + warning; the skill still
    // exports a valid (headers-only) xlsx with partial=true.
    let skill = parse_skill_yaml(SKILL_YAML).unwrap();
    let llm = ScriptedLlm("完全无关的散文，没有 JSON。".into());
    let inputs = json!({ "doc": "item-1", "entities": ["设备 A", "设备 B"] });
    let res = run_skill(&skill, &inputs, true, &resolver("设备 A 1080p"), &llm, "m").unwrap();
    assert!(res.partial, "degraded run flagged partial");
    assert!(!res.warnings.is_empty());
    let sheet = read_xlsx(&res.artifact_bytes);
    assert_eq!(sheet[0], vec!["参数", "设备 A", "设备 B", "差异"], "still a valid downloadable table");
}
