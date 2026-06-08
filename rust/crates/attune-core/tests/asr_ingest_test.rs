//! Audio → ASR routing + quality tests (SDLC dimension: Audio→ASR routing/quality).
//!
//! Two layers, by design (per CLAUDE.md §6.1 + the "no false-green" rule):
//!
//!   1. ROUTING (default lane, model-state-independent, fast):
//!      Asserts that `parser::parse_file` / `parse_bytes` DISPATCH `.wav/.mp3/...`
//!      down the ASR branch — proven WITHOUT running the whisper model by using
//!      corrupt/empty fixtures whose error is ASR-domain (mentions whisper/asr)
//!      and is NOT the generic "unsupported file format" text-branch error. This
//!      holds whether the whisper model is present (it errors on garbage) or
//!      absent (`detect_asr_backend()` → None → "ASR backend unavailable").
//!
//!   2. REAL ASR (env-gated `ATTUNE_TEST_REAL_ASR=1`, `#[ignore]`):
//!      Runs the genuine whisper.cpp transcription leg on `speech_known.wav`
//!      (espeak-synthesized, known sentence) and records CER against ground
//!      truth. Skipped (not failed) when the model/binary is absent.
//!
//! Fixtures are produced by the committed `fixtures/audio/gen_audio_fixtures.py`
//! and are byte-deterministic (no RNG, no timestamps). See that script's header.
//!
//! Run:
//!   cargo test -p attune-core --test asr_ingest_test                 # routing only
//!   ATTUNE_TEST_REAL_ASR=1 cargo test -p attune-core --test asr_ingest_test -- --ignored --nocapture

use std::path::{Path, PathBuf};

use attune_core::parser::{is_supported, parse_bytes, parse_file};

/// Ground truth spoken in `speech_known.wav` (must match KNOWN_SENTENCE in
/// fixtures/audio/gen_audio_fixtures.py).
const KNOWN_TRANSCRIPT: &str = "the quick brown fox jumps over the lazy dog";

/// Acceptance ceiling for the real-ASR leg. espeak's robotic voice is harder
/// than human speech, so we accept a generous CER floor — the point is to prove
/// the model produces the right *content*, not to benchmark whisper.
const MAX_CER: f64 = 0.40;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("audio")
}

/// All audio extensions parser.rs routes to the ASR branch.
const AUDIO_EXTS: &[&str] = &["mp3", "wav", "m4a", "flac", "ogg", "aac", "opus", "wma"];

// ───────────────────────── Layer 1: ROUTING (model-independent) ─────────────────────────

/// Every audio extension is reported supported. Pure dispatch-table check.
#[test]
fn audio_extensions_are_supported() {
    for ext in AUDIO_EXTS {
        let p = format!("clip.{ext}");
        assert!(
            is_supported(Path::new(&p)),
            ".{ext} must be a supported (routable) type"
        );
        // Case-insensitivity: dispatch lowercases the extension.
        let up = format!("CLIP.{}", ext.to_uppercase());
        assert!(
            is_supported(Path::new(&up)),
            ".{} (uppercase) must also be supported",
            ext.to_uppercase()
        );
    }
}

/// Bytes-API routing: an audio extension must NOT fall through to the text
/// branch. Whether the model is present or not, the error must be ASR-domain,
/// never the "unsupported file format" text-branch error. We feed a corrupt
/// payload so the heavy model (if present) rejects it instantly.
#[test]
fn audio_bytes_route_to_asr_not_text_branch() {
    let corrupt = std::fs::read(fixtures_dir().join("corrupt.wav"))
        .expect("corrupt.wav fixture must exist (run gen_audio_fixtures.py)");

    for ext in AUDIO_EXTS {
        let filename = format!("garbage.{ext}");
        let result = parse_bytes(&corrupt, &filename);
        let err = result
            .expect_err(&format!(".{ext} corrupt payload must Err, not Ok"))
            .to_string()
            .to_lowercase();

        // The decisive routing proof: it reached the ASR branch, NOT the text
        // fallthrough. The text branch would say "unsupported file format".
        assert!(
            !err.contains("unsupported file format"),
            ".{ext} must NOT hit the unsupported-text branch; got: {err}"
        );
        assert!(
            is_asr_domain_error(&err),
            ".{ext} error must be ASR-domain (asr/whisper/transcript); got: {err}"
        );
    }
}

/// File-API routing on a real on-disk corrupt `.wav`: same discrimination,
/// exercising `parse_file` (not just the bytes path).
#[test]
fn audio_file_corrupt_routes_to_asr_graceful_err() {
    let path = fixtures_dir().join("corrupt.wav");
    let err = parse_file(&path)
        .expect_err("corrupt.wav must return a graceful Err, never panic")
        .to_string()
        .to_lowercase();
    assert!(
        !err.contains("unsupported file format"),
        "corrupt.wav routed to wrong branch; got: {err}"
    );
    assert!(
        is_asr_domain_error(&err),
        "corrupt.wav error must be ASR-domain; got: {err}"
    );
}

/// A `.wav` extension on a non-audio (e.g. .mp4 video) byte body must still be
/// rejected gracefully via the ASR branch — never silently ingested as text and
/// never a panic. This is the "wrong-content, right-extension" adversarial case.
#[test]
fn audio_extension_with_nonaudio_body_errs_gracefully() {
    // 0-byte payload with a .wav name: routes to ASR, model rejects empty input.
    let result = parse_bytes(b"", "empty.wav");
    let err = result
        .expect_err("empty .wav must Err")
        .to_string()
        .to_lowercase();
    assert!(
        !err.contains("unsupported file format"),
        "empty .wav must route to ASR branch; got: {err}"
    );
    assert!(
        is_asr_domain_error(&err),
        "empty .wav error must be ASR-domain; got: {err}"
    );
}

/// Negative control: a `.mp4` (video, NOT in the audio set) must be rejected as
/// unsupported — proving the audio routing is specific, not "any binary → ASR".
#[test]
fn video_extension_is_not_routed_to_asr() {
    assert!(!is_supported(Path::new("movie.mp4")), ".mp4 must NOT be supported");
    let err = parse_bytes(b"\x00\x00\x00\x18ftypmp42", "movie.mp4")
        .expect_err(".mp4 must Err")
        .to_string()
        .to_lowercase();
    assert!(
        err.contains("unsupported"),
        ".mp4 must hit the unsupported branch (not ASR); got: {err}"
    );
}

/// An ASR-domain error mentions the ASR pipeline, distinguishing it from the
/// generic text-branch rejection. Covers both states:
///   - model absent  → "asr backend unavailable"
///   - model present → "whisper.cpp failed ...", "asr returned empty transcript"
fn is_asr_domain_error(err_lower: &str) -> bool {
    err_lower.contains("asr")
        || err_lower.contains("whisper")
        || err_lower.contains("transcript")
        || err_lower.contains("audio")
}

// ───────────────────────── Layer 2: REAL ASR (env-gated) ─────────────────────────

/// Real whisper.cpp transcription of a known sentence, scored by CER. Gated by
/// `ATTUNE_TEST_REAL_ASR=1` AND model presence — self-skips (not fails) when the
/// model or binary is absent (per the model-absent design of this env).
#[test]
#[ignore]
fn real_asr_transcribes_known_sentence_within_cer() {
    if std::env::var("ATTUNE_TEST_REAL_ASR").ok().as_deref() != Some("1") {
        eprintln!("skip: set ATTUNE_TEST_REAL_ASR=1 to run the real-model leg");
        return;
    }
    if !attune_core::asr::is_available() {
        eprintln!("skip: whisper.cpp backend/model absent (detect_asr_backend == None)");
        return;
    }

    let path = fixtures_dir().join("speech_known.wav");
    if !path.exists() {
        eprintln!("skip: speech_known.wav missing — run gen_audio_fixtures.py with espeak");
        return;
    }

    let (title, content) = parse_file(&path).expect("real ASR on known speech must succeed");
    let got = normalize(&content);
    let want = normalize(KNOWN_TRANSCRIPT);
    let cer = char_error_rate(&want, &got);
    eprintln!("real ASR: title={title:?}");
    eprintln!("real ASR: want = {want:?}");
    eprintln!("real ASR: got  = {got:?}");
    eprintln!("real ASR: CER  = {cer:.3} (ceiling {MAX_CER:.2})");
    assert!(
        cer <= MAX_CER,
        "real ASR CER {cer:.3} exceeded ceiling {MAX_CER:.2}\n want={want:?}\n got={got:?}"
    );
}

/// Real whisper on a pure tone / silence must NOT panic and must return either
/// Ok(empty-ish) or a graceful ASR-domain Err (whisper emits no speech). Gated
/// to avoid running the slow model in the default lane.
#[test]
#[ignore]
fn real_asr_on_tone_is_graceful() {
    if std::env::var("ATTUNE_TEST_REAL_ASR").ok().as_deref() != Some("1") {
        eprintln!("skip: set ATTUNE_TEST_REAL_ASR=1 to run the real-model leg");
        return;
    }
    if !attune_core::asr::is_available() {
        eprintln!("skip: whisper backend/model absent");
        return;
    }
    for name in ["tone_440hz.wav", "silence.wav"] {
        let path = fixtures_dir().join(name);
        match parse_file(&path) {
            Ok((_t, content)) => {
                eprintln!("{name}: Ok, {} chars of (likely no-speech) text", content.len());
            }
            Err(e) => {
                let err = e.to_string().to_lowercase();
                eprintln!("{name}: graceful Err = {err}");
                assert!(
                    is_asr_domain_error(&err) && !err.contains("unsupported file format"),
                    "{name} error must be a graceful ASR-domain Err; got: {err}"
                );
            }
        }
    }
}

// ───────────────────────── helpers ─────────────────────────

/// Lowercase, collapse whitespace, drop punctuation — compare content not casing.
fn normalize(s: &str) -> String {
    let cleaned: String = s
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c.is_whitespace() { c } else { ' ' })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Character Error Rate = Levenshtein(want, got) / len(want). 0.0 = perfect.
fn char_error_rate(want: &str, got: &str) -> f64 {
    let w: Vec<char> = want.chars().collect();
    let g: Vec<char> = got.chars().collect();
    if w.is_empty() {
        return if g.is_empty() { 0.0 } else { 1.0 };
    }
    let dist = levenshtein(&w, &g);
    dist as f64 / w.len() as f64
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod cer_self_tests {
    use super::*;

    #[test]
    fn cer_identical_is_zero() {
        assert_eq!(char_error_rate("hello world", "hello world"), 0.0);
    }

    #[test]
    fn cer_one_substitution() {
        // "cat" vs "bat" → 1 edit / 3 chars
        assert!((char_error_rate("cat", "bat") - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn cer_empty_reference() {
        assert_eq!(char_error_rate("", ""), 0.0);
        assert_eq!(char_error_rate("", "x"), 1.0);
    }

    #[test]
    fn normalize_strips_punct_and_case() {
        assert_eq!(normalize("The Quick, BROWN  fox!"), "the quick brown fox");
    }
}
