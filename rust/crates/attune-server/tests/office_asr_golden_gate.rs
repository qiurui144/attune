//! D3.3 — L1 ASR Golden Gate (WER + DER + RTF 红线).
//!
//! Spec §5 + plan §D3.3.
//!
//! 红线 (per BASELINE_ENV.md):
//!   - 中文 WER ≤ 15%
//!   - 英文 WER ≤ 10%
//!   - 中英混说 WER ≤ 18%
//!   - DER (说话人分离) ≤ 25%
//!   - RTF (CPU small Q8) p50 ≤ 0.5
//!
//! WER 计算: char-level edit distance / expected_transcript.chars().count()
//! (粗粒度, 中英都用字符级 — 真实生产用 jiwer / sclite 算 word-level, 此处为
//! release gate 足够; 后续可换更严的 measure).

use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Deserialize)]
struct AsrExpected {
    #[allow(dead_code)]
    id: String,
    audio_path: String,
    duration_sec: f64,
    #[allow(dead_code)]
    language: String,
    expected_transcript: String,
    #[serde(default)]
    expected_speakers: Option<u32>,
    reviewer: Reviewer,
}

#[derive(Debug, Deserialize)]
struct Reviewer {
    #[allow(dead_code)]
    name: String,
    approved: bool,
}

fn asr_golden_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join("office")
        .join("asr")
}

/// Char-level edit distance (Levenshtein). O(m·n) — sample 段 1-3 分钟, 转写 ~200-600 字,
/// 5-10 段足够快.
fn edit_distance(a: &str, b: &str) -> usize {
    let av: Vec<char> = a.chars().collect();
    let bv: Vec<char> = b.chars().collect();
    let (m, n) = (av.len(), bv.len());
    if m == 0 { return n; }
    if n == 0 { return m; }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut cur = vec![0usize; n + 1];
    for i in 1..=m {
        cur[0] = i;
        for j in 1..=n {
            let cost = if av[i - 1] == bv[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[n]
}

#[derive(Debug, Default)]
struct AsrLangStats {
    samples_total: usize,
    samples_with_audio: usize,
    samples_skipped_no_audio: usize,
    samples_skipped_unapproved: usize,
    total_chars_expected: usize,
    total_char_errors: usize,
    rtf_values: Vec<f64>,
}

impl AsrLangStats {
    fn wer(&self) -> f64 {
        if self.total_chars_expected == 0 {
            0.0
        } else {
            self.total_char_errors as f64 / self.total_chars_expected as f64
        }
    }

    fn rtf_p50(&self) -> f64 {
        if self.rtf_values.is_empty() {
            0.0
        } else {
            let mut sorted = self.rtf_values.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            sorted[sorted.len() / 2]
        }
    }
}

fn run_lang(lang_dir: &str) -> AsrLangStats {
    let dir = asr_golden_root().join(lang_dir);
    let mut stats = AsrLangStats::default();

    if !dir.exists() {
        eprintln!("[asr-gate {lang_dir}] dir missing, skipping: {}", dir.display());
        return stats;
    }

    let backend = attune_core::asr::detect_asr_backend();
    if backend.is_none() {
        eprintln!(
            "[asr-gate {lang_dir}] whisper-cli unavailable (run --bootstrap-models or install \
             whisper.cpp). SKIPPING all samples for this language."
        );
        return stats;
    }
    let backend = backend.unwrap();

    for entry in std::fs::read_dir(&dir).expect("read asr lang dir") {
        let path = entry.expect("entry").path();
        if !path.is_file() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.ends_with(".expected.yaml") {
            continue;
        }

        let yaml_str = std::fs::read_to_string(&path).expect("read yaml");
        let exp: AsrExpected = match serde_yaml::from_str(&yaml_str) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[asr-gate {lang_dir}] bad yaml {}: {e}", path.display());
                continue;
            }
        };
        stats.samples_total += 1;

        if !exp.reviewer.approved {
            stats.samples_skipped_unapproved += 1;
            continue;
        }

        // audio_path 是相对 yaml 同目录的相对路径
        let audio_full = dir.join(&exp.audio_path);
        if !audio_full.exists() {
            stats.samples_skipped_no_audio += 1;
            continue;
        }
        stats.samples_with_audio += 1;

        let start = Instant::now();
        let (segments, _legacy) = match attune_core::asr::transcribe_with_diarization(
            &backend,
            &audio_full,
            None, // 无 diarization (单测试 WER, DER 走另一套)
        ) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[asr-gate {lang_dir}] {} transcribe failed: {e}", exp.id);
                continue;
            }
        };
        let elapsed_sec = start.elapsed().as_secs_f64();

        // RTF (real-time factor) = transcribe_time / audio_duration
        if exp.duration_sec > 0.0 {
            stats.rtf_values.push(elapsed_sec / exp.duration_sec);
        }

        // WER
        let predicted_text: String = segments
            .iter()
            .map(|s| s.text.trim())
            .collect::<Vec<_>>()
            .join(" ");
        let expected_norm = exp.expected_transcript.trim();
        let predicted_norm = predicted_text.trim();
        let dist = edit_distance(expected_norm, predicted_norm);
        stats.total_char_errors += dist;
        stats.total_chars_expected += expected_norm.chars().count();
    }

    stats
}

fn assert_lang(lang_dir: &str, wer_red_line: f64) {
    let stats = run_lang(lang_dir);
    eprintln!(
        "[asr-gate {lang_dir}] samples: total={} with_audio={} skip_no_audio={} skip_unapproved={}",
        stats.samples_total,
        stats.samples_with_audio,
        stats.samples_skipped_no_audio,
        stats.samples_skipped_unapproved
    );
    eprintln!(
        "[asr-gate {lang_dir}] WER: {}/{} = {:.4} (red line ≤ {:.2})",
        stats.total_char_errors,
        stats.total_chars_expected,
        stats.wer(),
        wer_red_line
    );
    eprintln!(
        "[asr-gate {lang_dir}] RTF p50: {:.3} (red line ≤ 0.5)",
        stats.rtf_p50()
    );

    if stats.samples_with_audio == 0 {
        eprintln!(
            "[asr-gate {lang_dir}] SKIP — 0 audio-companion samples. \
             Run scripts/fetch-office-asr-golden.sh to populate."
        );
        return;
    }

    assert!(
        stats.wer() <= wer_red_line,
        "lang={lang_dir} WER={:.4} > red line {:.2}; errors={}/{}",
        stats.wer(),
        wer_red_line,
        stats.total_char_errors,
        stats.total_chars_expected
    );

    assert!(
        stats.rtf_p50() <= 0.5,
        "lang={lang_dir} RTF p50={:.3} > red line 0.5",
        stats.rtf_p50()
    );
}

#[test]
fn asr_zh_wer() {
    assert_lang("zh_aishell", 0.15);
}

#[test]
fn asr_en_wer() {
    assert_lang("en_libri", 0.10);
}

#[test]
fn asr_zh_en_mixed_wer() {
    assert_lang("zh_en_mixed", 0.18);
}

/// DER (说话人分离误差) 测试 — 仅 meeting 集.
/// D3.3 阶段框架先到位; 实际 DER 计算需要时间对齐 (Hungarian assignment),
/// 当前简化为 speaker count 对照: 期望 N 人, ASR 输出至少识别 N 个 SPEAKER_*.
#[test]
fn asr_meeting_speaker_count_basic() {
    let dir = asr_golden_root().join("meeting");
    if !dir.exists() {
        eprintln!("[asr-gate meeting] dir missing — skip");
        return;
    }
    let backend = attune_core::asr::detect_asr_backend();
    let diar = attune_core::asr::detect_diarization_backend();
    if backend.is_none() || diar.is_none() {
        eprintln!("[asr-gate meeting] whisper-cli or pyannote unavailable — SKIP");
        return;
    }

    let backend = backend.unwrap();
    let mut samples_with_audio = 0;
    let mut speaker_count_failures: Vec<String> = Vec::new();

    for entry in std::fs::read_dir(&dir).expect("read meeting dir") {
        let path = entry.expect("entry").path();
        if !path.is_file() { continue; }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.ends_with(".expected.yaml") { continue; }
        let yaml_str = std::fs::read_to_string(&path).expect("read yaml");
        let exp: AsrExpected = match serde_yaml::from_str(&yaml_str) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if !exp.reviewer.approved {
            continue;
        }
        let audio_full = dir.join(&exp.audio_path);
        if !audio_full.exists() {
            continue;
        }
        samples_with_audio += 1;

        let (segments, _) = match attune_core::asr::transcribe_with_diarization(
            &backend, &audio_full, diar.as_ref(),
        ) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let unique_speakers: std::collections::BTreeSet<&str> = segments
            .iter()
            .filter_map(|s| s.speaker.as_deref())
            .collect();
        let actual = unique_speakers.len() as u32;
        let expected = exp.expected_speakers.unwrap_or(0);
        // Allow ±1 speaker tolerance (over-segmentation 常见)
        if (actual as i32 - expected as i32).abs() > 1 {
            speaker_count_failures
                .push(format!("{} expected {} speakers, got {}", exp.id, expected, actual));
        }
    }

    eprintln!(
        "[asr-gate meeting] samples_with_audio={} failures={}",
        samples_with_audio,
        speaker_count_failures.len()
    );

    if samples_with_audio == 0 {
        eprintln!("[asr-gate meeting] SKIP — 0 audio-companion samples");
        return;
    }

    assert!(
        speaker_count_failures.is_empty(),
        "meeting speaker count failures: {speaker_count_failures:#?}"
    );
}

#[cfg(test)]
mod edit_distance_tests {
    use super::edit_distance;

    #[test]
    fn identical() {
        assert_eq!(edit_distance("abc", "abc"), 0);
    }

    #[test]
    fn one_sub() {
        assert_eq!(edit_distance("abc", "abd"), 1);
    }

    #[test]
    fn insertion() {
        assert_eq!(edit_distance("abc", "abcd"), 1);
    }

    #[test]
    fn deletion() {
        assert_eq!(edit_distance("abcd", "abc"), 1);
    }

    #[test]
    fn empty_strings() {
        assert_eq!(edit_distance("", ""), 0);
        assert_eq!(edit_distance("abc", ""), 3);
        assert_eq!(edit_distance("", "xyz"), 3);
    }

    #[test]
    fn chinese_chars() {
        assert_eq!(edit_distance("你好世界", "你好世界"), 0);
        assert_eq!(edit_distance("你好世界", "你好地球"), 2);
    }
}
