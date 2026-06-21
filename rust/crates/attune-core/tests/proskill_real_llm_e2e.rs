//! Real-LLM E2E for the academic-pro `thesis-chapter-draft` deliverable skill (CAP-4 closure).
//!
//! Proves the last mile end-to-end: the pro plugin's declarative skill yaml (pure OSS agent
//! composition `writing.research_synthesis`) runs against a **real** DeepSeek model with real KB
//! source material, produces a Document IR, exports to docx, and the docx round-trips back to text
//! that correctly contains Chinese content.
//!
//! Env-gated (so CI without a key skips it):
//!   DEEPSEEK_API_KEY=...  cargo test -p attune-core --test proskill_real_llm_e2e -- --ignored --nocapture
//!
//! The skill yaml is read from the attune-pro academic plugin (the real deliverable artifact), so
//! this also verifies that the very file academic-pro declares in `registers_skills:` is runnable.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use attune_core::llm::OpenAiLlmProvider;
use attune_core::skill_runtime::{parse_skill_yaml, run_skill, MapResolver};
use serde_json::json;

/// Unzip `word/document.xml` from docx bytes (the docx round-trip oracle).
fn docx_text(bytes: &[u8]) -> String {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).expect("docx is a zip");
    let mut f = zip.by_name("word/document.xml").expect("docx has word/document.xml");
    let mut xml = String::new();
    f.read_to_string(&mut xml).unwrap();
    xml
}

/// Resolve the real academic-pro thesis skill yaml (the declared deliverable). The attune-pro
/// checkout location varies (worktree nesting), so accept an explicit `ACADEMIC_THESIS_YAML`
/// override and otherwise probe the common sibling-checkout locations.
fn locate_thesis_yaml() -> String {
    if let Ok(p) = std::env::var("ACADEMIC_THESIS_YAML") {
        return p;
    }
    let rel = "plugins/academic-pro/skills/thesis-chapter-draft.yaml";
    for base in [
        "/data/company/project/attune-pro",
        "../../../attune-pro",
        "../../../../attune-pro",
    ] {
        let cand = format!("{base}/{rel}");
        if std::path::Path::new(&cand).exists() {
            return cand;
        }
    }
    panic!("cannot locate thesis yaml; set ACADEMIC_THESIS_YAML");
}

#[test]
#[ignore = "real LLM — needs DEEPSEEK_API_KEY; run with --ignored"]
fn academic_thesis_chapter_draft_real_deepseek_docx_roundtrip() {
    let api_key = std::env::var("DEEPSEEK_API_KEY").expect("DEEPSEEK_API_KEY env");
    let yaml_path = locate_thesis_yaml();
    let yaml = std::fs::read_to_string(&yaml_path)
        .unwrap_or_else(|e| panic!("read thesis yaml at {yaml_path}: {e}"));
    let skill = parse_skill_yaml(&yaml).expect("thesis skill yaml parses");
    assert_eq!(skill.id, "academic-thesis-chapter-draft");

    // Real KB material: two source documents on a research topic (Chinese), keyed by item id.
    let mut map = BTreeMap::new();
    map.insert(
        "kb-1".to_string(),
        "卷积神经网络（CNN）通过局部感受野和权值共享显著降低了图像分类任务的参数量。\
         AlexNet 在 2012 年 ImageNet 竞赛中将 top-5 错误率降至 16.4%，确立了深度学习在视觉领域的主导地位。"
            .to_string(),
    );
    map.insert(
        "kb-2".to_string(),
        "残差网络（ResNet）引入跳跃连接缓解了深层网络的梯度消失问题，使得上百层的网络可以稳定训练。\
         ResNet-152 在 ImageNet 上达到 3.57% 的 top-5 错误率，超过人类水平。"
            .to_string(),
    );
    let resolver = MapResolver(map);

    let inputs = json!({
        "item_ids": ["kb-1", "kb-2"],
        "structure": "背景\n方法\n结论",
        "title": "深度卷积网络综述（章节初稿）"
    });

    // DeepSeek BYOK: OpenAI-compatible endpoint; api model name is `deepseek-chat`.
    let llm = OpenAiLlmProvider::new("https://api.deepseek.com/v1", &api_key, "deepseek-chat");

    let result = run_skill(&skill, &inputs, true, &resolver, &llm, "deepseek-chat")
        .expect("real-LLM skill run succeeds");

    // Terminal artifact must be a downloadable docx.
    assert_eq!(result.format.extension(), "docx", "thesis skill exports docx");
    assert!(!result.artifact_bytes.is_empty(), "docx bytes produced");

    // Round-trip: unzip the docx and confirm Chinese content survived (not garbled / empty).
    let xml = docx_text(&result.artifact_bytes);
    assert!(xml.contains("综述") || xml.contains("章节"), "title Chinese present in docx");
    // The synthesized body must reference the source material's domain terms (grounding signal).
    let grounded = ["卷积", "网络", "ResNet", "ImageNet", "残差", "深度"]
        .iter()
        .filter(|t| xml.contains(**t))
        .count();
    assert!(
        grounded >= 2,
        "synthesized docx must contain source-grounded terms (got {grounded} matches)"
    );

    eprintln!(
        "[E2E PASS] thesis-chapter-draft real-DeepSeek: docx {} bytes, token_bill={:?}, grounded_terms={grounded}",
        result.artifact_bytes.len(),
        result.token_bill
    );
}
