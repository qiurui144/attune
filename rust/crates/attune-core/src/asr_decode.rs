//! Pure-Rust audio pre-decode → canonical 16 kHz mono 16-bit WAV.
//!
//! sherpa-onnx's `read_audio_file` is WAV-only AND requires exactly 16 kHz `i16`. Before
//! this module, any `.mp3` / `.m4a` / `.flac` / `.ogg` / non-16 kHz WAV had to fall back to
//! the `whisper-cli` subprocess just to transcribe — re-introducing the per-platform binary
//! dependency that the SenseVoice work exists to remove.
//!
//! This module decodes those containers **in-process** with `symphonia` (pure Rust), mixes
//! to mono, resamples to 16 kHz, and writes a temp WAV with `hound`. The SenseVoice engine
//! then transcribes that WAV directly, so transcription no longer needs whisper-cli. (Whisper
//! is still kept for diarization + the explicit CPU-tier fallback; see `asr.rs`.)
//!
//! Resampling uses a simple linear interpolator. For ASR (16 kHz target, speech) linear
//! resampling is adequate — SenseVoice's own front-end is robust to it, and we are not doing
//! high-fidelity audio. This avoids pulling a heavier DSP dependency (`rubato`) for no quality
//! gain on the ASR task.

#[cfg(feature = "asr-sensevoice")]
use crate::error::{Result, VaultError};
#[cfg(feature = "asr-sensevoice")]
use std::path::{Path, PathBuf};

/// The sample rate SenseVoice (and whisper) expect.
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Decode `src` (any symphonia-supported container) into a freshly-created temp WAV file
/// that is 16 kHz, mono, 16-bit PCM — the format sherpa's `read_audio_file` accepts.
///
/// Returns `(temp_dir, wav_path)`. The caller must keep `temp_dir` alive until the WAV has
/// been consumed (dropping it deletes the file). Errors (never panics) on unreadable /
/// unsupported / empty audio so the engine dispatcher can fall back to whisper-cli.
#[cfg(feature = "asr-sensevoice")]
pub fn decode_to_wav16k_mono(src: &Path) -> Result<(tempfile::TempDir, PathBuf)> {
    let (samples, sample_rate, channels) = decode_to_f32(src)?;
    if samples.is_empty() {
        return Err(VaultError::InvalidInput(format!(
            "audio pre-decode produced no samples for {}",
            src.display()
        )));
    }
    let mono = downmix_to_mono(&samples, channels);
    let resampled = resample_linear(&mono, sample_rate, TARGET_SAMPLE_RATE);

    let tmp = tempfile::TempDir::new().map_err(VaultError::Io)?;
    let wav_path = tmp.path().join("asr_input_16k_mono.wav");
    write_wav16(&wav_path, &resampled)?;
    Ok((tmp, wav_path))
}

/// Decode any supported container into interleaved f32 samples + (sample_rate, channels).
#[cfg(feature = "asr-sensevoice")]
fn decode_to_f32(src: &Path) -> Result<(Vec<f32>, u32, u16)> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::errors::Error as SymphoniaError;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(src).map_err(VaultError::Io)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = src.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| VaultError::InvalidInput(format!("audio probe failed: {e}")))?;
    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or_else(|| VaultError::InvalidInput("audio has no decodable track".to_string()))?;
    let track_id = track.id;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| VaultError::InvalidInput(format!("audio codec unsupported: {e}")))?;

    let mut samples: Vec<f32> = Vec::new();
    let mut sample_rate: u32 = 0;
    let mut channels: u16 = 0;
    let mut sample_buf: Option<SampleBuffer<f32>> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            // Clean EOF (symphonia signals end-of-stream as an IoError UnexpectedEof).
            Err(SymphoniaError::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => {
                return Err(VaultError::InvalidInput(format!("audio read error: {e}")));
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                if sample_buf.is_none() {
                    let spec = *decoded.spec();
                    sample_rate = spec.rate;
                    channels = spec.channels.count() as u16;
                    let cap = decoded.capacity() as u64;
                    sample_buf = Some(SampleBuffer::<f32>::new(cap, spec));
                }
                if let Some(buf) = sample_buf.as_mut() {
                    buf.copy_interleaved_ref(decoded);
                    samples.extend_from_slice(buf.samples());
                }
            }
            // Decode errors on a single packet are recoverable — skip it.
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => return Err(VaultError::InvalidInput(format!("audio decode error: {e}"))),
        }
    }

    if sample_rate == 0 || channels == 0 {
        return Err(VaultError::InvalidInput(
            "audio decoded to zero frames (empty or corrupt input)".to_string(),
        ));
    }
    Ok((samples, sample_rate, channels))
}

/// Average interleaved channels down to a single mono track.
#[cfg(feature = "asr-sensevoice")]
fn downmix_to_mono(interleaved: &[f32], channels: u16) -> Vec<f32> {
    let ch = channels.max(1) as usize;
    if ch == 1 {
        return interleaved.to_vec();
    }
    let frames = interleaved.len() / ch;
    let mut mono = Vec::with_capacity(frames);
    for f in 0..frames {
        let base = f * ch;
        let sum: f32 = interleaved[base..base + ch].iter().sum();
        mono.push(sum / ch as f32);
    }
    mono
}

/// Linear-interpolation resample from `from_rate` to `to_rate`.
#[cfg(feature = "asr-sensevoice")]
fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = to_rate as f64 / from_rate as f64;
    let out_len = ((input.len() as f64) * ratio).round().max(1.0) as usize;
    let mut out = Vec::with_capacity(out_len);
    let last = input.len() - 1;
    for i in 0..out_len {
        // Position in the input timeline for this output sample.
        let src_pos = i as f64 / ratio;
        let idx = src_pos.floor() as usize;
        if idx >= last {
            out.push(input[last]);
        } else {
            let frac = (src_pos - idx as f64) as f32;
            out.push(input[idx] * (1.0 - frac) + input[idx + 1] * frac);
        }
    }
    out
}

/// Write mono f32 samples (in [-1, 1]) as 16-bit PCM WAV at `TARGET_SAMPLE_RATE`.
#[cfg(feature = "asr-sensevoice")]
fn write_wav16(path: &Path, mono: &[f32]) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .map_err(|e| VaultError::InvalidInput(format!("wav create failed: {e}")))?;
    for &s in mono {
        let clamped = s.clamp(-1.0, 1.0);
        let v = (clamped * i16::MAX as f32).round() as i16;
        writer
            .write_sample(v)
            .map_err(|e| VaultError::InvalidInput(format!("wav write failed: {e}")))?;
    }
    writer
        .finalize()
        .map_err(|e| VaultError::InvalidInput(format!("wav finalize failed: {e}")))?;
    Ok(())
}

#[cfg(all(test, feature = "asr-sensevoice"))]
mod tests {
    use super::*;

    #[test]
    fn resample_identity_when_rates_equal() {
        let input = vec![0.1, 0.2, 0.3, 0.4];
        let out = resample_linear(&input, 16_000, 16_000);
        assert_eq!(out, input);
    }

    #[test]
    fn resample_downsample_halves_length() {
        // 32 kHz → 16 kHz halves the sample count (±1 for rounding).
        let input: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
        let out = resample_linear(&input, 32_000, 16_000);
        assert!(
            (out.len() as i64 - 50).abs() <= 1,
            "expected ~50 samples, got {}",
            out.len()
        );
    }

    #[test]
    fn resample_upsample_doubles_length() {
        let input: Vec<f32> = (0..50).map(|i| i as f32 / 50.0).collect();
        let out = resample_linear(&input, 8_000, 16_000);
        assert!(
            (out.len() as i64 - 100).abs() <= 1,
            "expected ~100 samples, got {}",
            out.len()
        );
    }

    #[test]
    fn resample_empty_is_empty() {
        assert!(resample_linear(&[], 44_100, 16_000).is_empty());
    }

    #[test]
    fn downmix_stereo_averages_channels() {
        // interleaved L,R,L,R: (1,-1),(0.5,0.5) → mono 0.0, 0.5
        let interleaved = vec![1.0, -1.0, 0.5, 0.5];
        let mono = downmix_to_mono(&interleaved, 2);
        assert_eq!(mono, vec![0.0, 0.5]);
    }

    #[test]
    fn downmix_mono_passthrough() {
        let mono = vec![0.1, 0.2, 0.3];
        assert_eq!(downmix_to_mono(&mono, 1), mono);
    }

    #[test]
    fn write_then_read_roundtrip_wav() {
        // Write a known mono signal, read it back via hound, assert format + sample count.
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("rt.wav");
        let mono: Vec<f32> = (0..160).map(|i| ((i as f32) / 160.0) * 0.5).collect();
        write_wav16(&path, &mono).unwrap();

        let reader = hound::WavReader::open(&path).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, TARGET_SAMPLE_RATE);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(reader.into_samples::<i16>().count(), mono.len());
    }

    #[test]
    fn decode_synthetic_wav_roundtrips_to_16k_mono() {
        // Build a 44.1 kHz stereo WAV in-memory, then decode_to_wav16k_mono should produce a
        // mono 16 kHz WAV with roughly rate-scaled sample count (no whisper-cli involved).
        let tmp_in = tempfile::TempDir::new().unwrap();
        let src = tmp_in.path().join("synthetic.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&src, spec).unwrap();
        // 44100 stereo frames = 1s. Simple tone so decode has real content.
        for i in 0..44_100 {
            let s = ((i as f32 * 0.05).sin() * 0.3 * i16::MAX as f32) as i16;
            w.write_sample(s).unwrap(); // L
            w.write_sample(s).unwrap(); // R
        }
        w.finalize().unwrap();

        let (_keep, wav16) = decode_to_wav16k_mono(&src).unwrap();
        let reader = hound::WavReader::open(&wav16).unwrap();
        let out_spec = reader.spec();
        assert_eq!(out_spec.channels, 1, "must be mono");
        assert_eq!(out_spec.sample_rate, TARGET_SAMPLE_RATE, "must be 16 kHz");
        let n = reader.into_samples::<i16>().count();
        // 1s of audio at 16 kHz ≈ 16000 samples (±2% for resample rounding).
        assert!(
            (n as i64 - 16_000).abs() < 400,
            "expected ~16000 mono samples, got {n}"
        );
    }

    #[test]
    fn decode_missing_file_errs_not_panics() {
        let r = decode_to_wav16k_mono(Path::new("/nonexistent/audio-xyz.mp3"));
        assert!(r.is_err());
    }
}
