//! SenseVoice ASR **quality gate** — real-audio transcription integration test.
//!
//! Transcribes `tests/assets/zh.wav` (GT = "开放时间早上9点至下午5点") with the in-process
//! SenseVoice (sherpa-onnx) provider and asserts CER ≤ 15%. This is the hard quality
//! re-verification of the spike result (CER 7.69%); it is NOT a mock — it loads the real
//! int8 ONNX model and decodes real audio.
//!
//! Model assets (`model.int8.onnx` + `tokens.txt`) are large (239 MB), so they are read
//! from a local path (env `ATTUNE_SENSEVOICE_MODEL_DIR`, default = the VLM-benchmark copy),
//! NOT bundled in the repo. The test is gated on `asr-sensevoice` (default-on). If the
//! model dir is absent the test fails loudly with an actionable message (no silent skip),
//! because this gate must actually run.

#![cfg(feature = "asr-sensevoice")]

use attune_core::asr_sensevoice::{transcribe_sensevoice, SenseVoiceBackend};
use std::path::PathBuf;

const GT: &str = "开放时间早上9点至下午5点";
const CER_THRESHOLD: f64 = 0.15;

/// Locate the SenseVoice model dir, in priority order:
/// 1. `ATTUNE_SENSEVOICE_MODEL_DIR` (explicit; dev + CI set this).
/// 2. attune `models_dir()/asr/sensevoice` (where CI fetches + where ensure() lands).
/// 3. the local VLM-benchmark copy (dev default on this machine).
fn model_dir() -> PathBuf {
    if let Ok(d) = std::env::var("ATTUNE_SENSEVOICE_MODEL_DIR") {
        return PathBuf::from(d);
    }
    let runtime = attune_core::asr_sensevoice::sensevoice_model_dir();
    if runtime.join("model.int8.onnx").exists() {
        return runtime;
    }
    PathBuf::from(
        "/data/company/project/vlm-llm-benchmark/datasets/asr/models/\
sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17",
    )
}

/// Levenshtein edit distance over chars (CER numerator).
fn edits(a: &str, b: &str) -> usize {
    let ac: Vec<char> = a.chars().collect();
    let bc: Vec<char> = b.chars().collect();
    let (n, m) = (ac.len(), bc.len());
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in dp.iter_mut().enumerate().take(n + 1) {
        row[0] = i;
    }
    for (j, cell) in dp[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = if ac[i - 1] == bc[j - 1] { 0 } else { 1 };
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
        }
    }
    dp[n][m]
}

fn cer(gt: &str, hyp: &str) -> f64 {
    let n = gt.chars().count();
    if n == 0 {
        return if hyp.is_empty() { 0.0 } else { 1.0 };
    }
    edits(gt, hyp) as f64 / n as f64
}

/// Strip trailing CJK / ASCII punctuation so "...5点。" vs "...5点" is not penalised.
fn strip_trailing_punct(s: &str) -> String {
    s.trim_end_matches(['。', '，', '、', '.', ',', '!', '?', '！', '？', ' '])
        .to_string()
}

#[test]
fn sensevoice_zh_wav_cer_under_threshold() {
    let dir = model_dir();
    let model = dir.join("model.int8.onnx");
    let tokens = dir.join("tokens.txt");
    assert!(
        model.exists() && tokens.exists(),
        "SenseVoice model assets missing at {} (set ATTUNE_SENSEVOICE_MODEL_DIR). \
         model.int8.onnx exists={}, tokens.txt exists={}",
        dir.display(),
        model.exists(),
        tokens.exists()
    );

    let wav = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("assets")
        .join("zh.wav");
    assert!(
        wav.exists(),
        "bundled test wav missing at {}",
        wav.display()
    );

    let backend = SenseVoiceBackend::from_paths(&model, &tokens)
        .expect("construct SenseVoice backend from real assets");

    let hyp = transcribe_sensevoice(&backend, &wav).expect("transcribe zh.wav");
    let hyp_clean = strip_trailing_punct(&hyp);
    let raw_cer = cer(GT, &hyp);
    let clean_cer = cer(GT, &hyp_clean);

    eprintln!("=== SenseVoice quality gate ===");
    eprintln!("GT        : {GT}");
    eprintln!("HYP       : {hyp}");
    eprintln!("HYP_clean : {hyp_clean}");
    eprintln!(
        "CER raw   : {:.2}% ({} edits / {} ref chars)",
        raw_cer * 100.0,
        edits(GT, &hyp),
        GT.chars().count()
    );
    eprintln!("CER clean : {:.2}%", clean_cer * 100.0);

    assert!(
        raw_cer <= CER_THRESHOLD,
        "SenseVoice CER {:.2}% exceeds threshold {:.0}% (HYP={hyp})",
        raw_cer * 100.0,
        CER_THRESHOLD * 100.0
    );
}

/// Engine dispatch through the public asr API: SenseVoice backend → transcribe_with_engine.
#[test]
fn transcribe_with_engine_sensevoice_path() {
    use attune_core::asr::{transcribe_with_engine, AsrEngine};

    let dir = model_dir();
    let model = dir.join("model.int8.onnx");
    let tokens = dir.join("tokens.txt");
    if !(model.exists() && tokens.exists()) {
        panic!(
            "SenseVoice model assets missing at {} (set ATTUNE_SENSEVOICE_MODEL_DIR)",
            dir.display()
        );
    }
    let wav = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("assets")
        .join("zh.wav");

    let backend = SenseVoiceBackend::from_paths(&model, &tokens).unwrap();
    let engine = AsrEngine::SenseVoice(backend);
    assert_eq!(engine.label(), "sensevoice");
    let text = transcribe_with_engine(&engine, &wav).expect("engine dispatch transcribe");
    assert!(
        text.contains("开放时间"),
        "expected zh transcript via engine dispatch, got: {text}"
    );
}
