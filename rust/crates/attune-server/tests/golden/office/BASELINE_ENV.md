# Office Helper Golden Dataset — Baseline Environment

> 固化 L1 准入门测试的基线环境，以保证 accuracy/speed 红线在不同 runner 上可比。
> Per CLAUDE.md benchmark 铁律 + Spec §6.4。

## 软件版本

| 组件 | 版本 | 说明 |
|------|------|------|
| Rust toolchain | 1.83+ (stable) | `rust-toolchain.toml` |
| PP-OCRv5 mobile | det 21 MB + rec 21 MB ONNX (built-in) | `attune-core/src/ocr/ppocr.rs` |
| whisper.cpp CLI | per-host install (Linux/Mac/Win) | `attune-core/src/asr.rs::detect_asr_backend` |
| whisper model | `ggml-small.bin` Q8 默认 | OCR/ASR `--bootstrap-models` 时下载 |
| pyannote (optional) | venv pip install pyannote.audio | `attune-core/src/asr.rs::DiarizationBackend::Pyannote` |

## 硬件参考 (CI ubuntu-latest)

- CPU: Intel Xeon @ 2.3-2.7 GHz (GitHub-hosted 4 vCPU)
- RAM: 16 GB
- Disk: SSD
- 平台: ubuntu-22.04 / windows-server-2022 (per `.github/workflows/ci.yml`)

## OCR 红线 (per Spec §5)

| Scene | 准确度门 | 速度门 (p50) | 速度门 (p95) | 样本量 |
|-------|---------|--------------|--------------|--------|
| document | 字符级 ≥ 92% | A4 ≤ 3s | ≤ 1.5×p50 | 10 |
| receipt | 字段级 ≥ 92% (10×7=70 字段) | ≤ 2s | ≤ 1.5×p50 | 10 |
| table | cell 级 ≥ 92% | ≤ 4s | ≤ 1.5×p50 | 10 |
| card | 字段级 ≥ 92% (10×6=60 字段, Z 高标杆) | ≤ 1.5s | ≤ 1.5×p50 | 10 |
| id_card_cn | 字段级 ≥ 95% | ≤ 2s | ≤ 1.5×p50 | 5 |
| bank_card | 字段级 ≥ 95% | ≤ 2s | ≤ 1.5×p50 | 5 |
| business_license | 字段级 ≥ 95% | ≤ 2s | ≤ 1.5×p50 | 5 |

## ASR 红线 (per Spec §5)

| 指标 | 红线 | 样本量 |
|------|------|--------|
| 中文 WER | ≤ 15% | 20 段 (AISHELL-3 抽样) |
| 英文 WER | ≤ 10% | 20 段 (LibriSpeech test-clean) |
| 中英混说 WER | ≤ 18% | 5 段 (内部技术分享) |
| DER (说话人分离) | ≤ 25% | 10 段 (会议录音, 2-4 人) |
| RTF (CPU small Q8) | p50 ≤ 0.5 | 上述全集 |

## 测试运行约定

```bash
# L1 准入门 (默认跑)
cargo test -p attune-server --test office_ocr_golden_gate --release
cargo test -p attune-server --test office_asr_golden_gate --release
cargo test -p attune-server --test office_error_contract --release
cargo test -p attune-server --test office_schema_compat --release

# ENFORCE mode (六类覆盖门)
ATTUNE_ENFORCE_OFFICE_FLOOR=1 cargo test -p attune-server --test office_six_category_floor --release
```

## 跳过策略

不可避免的环境差异（PP-OCR 模型未下载 / whisper-cli 未安装）时：

- OCR golden gate：若 `attune_core::ocr::detect_default_provider()` 返 None → 整个测试 `#[ignore]`
  自动跳过，标 warning。CI 用 `--include-ignored` 跑 nightly 也可显式跑。
- ASR golden gate：若 `attune_core::asr::detect_asr_backend()` 返 None → 同上跳过。
- Error contract / schema compat 测试：不依赖外部模型，**永远跑**。

## benchmark 历史

每次 L1 gate 跑完保存 p50/p95 到 `benchmarks/YYYY-MM-DD-<commit>.json` (D3.5 实施时写入)，
跟踪准确度/速度回归。
