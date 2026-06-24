# SenseVoice ONNX ASR Provider — Implementation Report

**Date:** 2026-06-24  **Branch:** `feature/sensevoice-asr`  **Base:** `origin/develop` (ccc4765)
**Worktree:** `/data/attune-wt-sensevoice`  **Spec:** `docs/superpowers/specs/2026-06-24-sensevoice-asr-provider.md`

## Verdict

**Quality gate PASS (real audio, real model, REAL_EXIT=0).** SenseVoice in-process ASR
integrated into attune-core, catalog-driven engine dispatch, whisper retained as CPU-tier
fallback + diarization, wasm boundary proven intact, full attune-core lib suite green
(2587 passed / 0 failed). One **deviation** from spec (linkage: `download-binaries` not
`static`) forced by a real link-time symbol collision — see §Deviations.

## Quality gate (hard deliverable)

`tests/sensevoice_quality_gate.rs` transcribes the real int8 ONNX model + `tests/assets/zh.wav`:

```
GT        : 开放时间早上9点至下午5点
HYP       : 开放时间早上9点至下午5点。
CER raw   : 7.69% (1 edits / 13 ref chars)   ← single trailing 。
CER clean : 0.00%
REAL_EXIT : 0   (cargo test exit captured directly, not through a pipe-tail)
```

CER 7.69% ≤ 15% threshold → matches the spike + VLM benchmark exactly. A second test
exercises the public `AsrEngine::SenseVoice` dispatch via `transcribe_with_engine`. Raw log:
`reports/runs/sensevoice/quality_gate2.log`.

## Module changes

| File | Change |
|---|---|
| `attune-core/Cargo.toml` | `sherpa-rs 0.6.8` optional dep, `download-binaries` feature, drop `tts`; new `asr-sensevoice` default-on feature |
| `attune-core/src/asr_sensevoice.rs` (new) | `SenseVoiceBackend` (detect/from_paths), `ensure_sensevoice_model()` (S8 failover fetch, honors HF_HUB_OFFLINE), `transcribe_sensevoice()` (in-process; empty→"", failure→Err, no panic); feature-off stub |
| `attune-core/src/asr.rs` | `AsrEngine { Whisper \| SenseVoice }` enum, `catalog_asr_engine()`, `detect_asr_engine()`, `transcribe_with_engine()`; whisper paths unchanged |
| `attune-core/src/parser.rs` | both audio call sites dispatch: SenseVoice = plain transcribe, Whisper = diarization path (multi-speaker unaffected) |
| `attune-server/routes/ai_stack.rs` | `asr.engine` dynamic (was hardcoded `whisper.cpp`), `ensure` fetches SenseVoice when catalog selects it else whisper ggml |
| `attune-core/assets/model-catalog.default.yaml` | amd/intel-win sensevoice ASR `repo`+`file` documented (S8 fetch source) |
| `attune-core/src/infer/catalog.rs` | regression test: catalog repo == `SENSEVOICE_REPO` (drift guard) |
| `.github/workflows/ci.yml` + split-guard | linux+windows ASR smoke (fetch model → quality gate); guard registers the new gate |

## Engine abstraction design

Catalog (`model-catalog.default.yaml` `asr.engine`) is the SSOT. `catalog_asr_engine()`
maps current hardware → tier (`tier_for_hardware(os, accel)`) → `resolve(tier, Role::Asr).engine`.
`detect_asr_engine()`: if `engine == "sensevoice"` AND `cfg!(feature="asr-sensevoice")` AND the
model is present on disk → `SenseVoice`; else fall through to whisper (`detect_asr_backend`).
A fresh install with no model yet degrades to whisper rather than blocking (spec §7). Adding a
variant (rk-asr / official k2-fsa) is a single enum arm.

## Feature-gate / WASM-not-broken evidence

- `cargo check -p attune-core` (default, asr-sensevoice on) → **PASS**.
- `cargo check -p attune-core --no-default-features` (sherpa + wasmtime OFF) → **PASS** —
  proves the gate fully excludes the C-linked sherpa dep; the `#[cfg(not(feature))]` stub covers it.
- `cargo build -p attune-agent-sdk --target wasm32-wasip1` → **PASS**. The real wasm boundary is
  the zero-native-dep leaf crate `attune-agent-sdk` (compiled to wasm32-wasip1, run by the
  wasmtime host inside attune-core). attune-core itself is **never** compiled to wasm32 (nothing
  in the repo does), and `attune-agent-sdk` is untouched by this change (empty `git diff`), so
  adding sherpa-rs to attune-core cannot break the wasm build path.

## Model runtime-fetch

`ensure_sensevoice_model()` fetches `model.int8.onnx` + `tokens.txt` from HF
`csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17` via `model_source::download_with_failover`
(company-mirror → hf-mirror → HF), into `models_dir()/asr/sensevoice/`. HF_HUB_OFFLINE +
missing → explicit `asr-model-offline-missing` error (no silent hang). sherpa-onnx **C libs**
are build-time (cargo), not runtime-fetched. Wired into `ai-stack/ensure`. Tested with the
local model (no real download triggered).

## Windows CI smoke (PENDING-VERIFY)

`ci.yml` rust-test-core (both OS legs): fetch model+tokens from HF → run quality gate
(CER ≤ 15%, zh.wav). **Windows leg = PENDING-VERIFY-on-windows-runner** — first real execution;
dist.json has x86_64-pc-windows-msvc prebuilt so build.rs picks the `.lib`/`.dll` set (no MSVC
C++ source compile). Not verifiable locally (Linux host).

## Whisper regression

whisper `AsrBackend` / `transcribe_audio` / `transcribe_with_diarization` / whisperx / pyannote
all unchanged. Catalog CPU-fallback tier still resolves `engine == "whisper"` (regression tests
`amd_intel_asr_sensevoice_cpu_whisper`, `cpu_fallback_freezes...` pass). `asr_ingest_test` 9/0.
Full lib suite **2587 passed / 0 failed**.

## Deviations

1. **Linkage `download-binaries` (dynamic) instead of spec §3/§4 `static`.** With `static`,
   sherpa-rs statically links its OWN bundled onnxruntime `.a`, colliding with attune-core's
   existing `ort` crate (`ort_sys` also archives onnxruntime) → rust-lld **duplicate symbol**
   `onnxruntime::common::Status::*` on the test binary (`cargo check` passed, link failed). Raw
   evidence: `reports/runs/sensevoice/quality_gate.log` (first run, pre-fix). `download-binaries`
   links sherpa-onnx's onnxruntime dynamically (libonnxruntime.so/.dll at runtime), no static-archive
   clash — this is exactly what the spike used and ran. **Packaging follow-up (not yet done — out
   of this worktree's code scope):** the desktop/server release workflow must place the 3 shared
   libs (libonnxruntime / libsherpa-onnx-c-api / -cxx-api, ~20 MB) beside the binary or set rpath
   `$ORIGIN`; on Windows a `.dll` set. Original bug (Linux ELF in Win pkg) is still gone — these
   are per-target-correct prebuilts, not a wrong-OS binary.

## Verification commands (all green, CARGO_TARGET_DIR=/data/attune-target TMPDIR=/data/tmp)

- `cargo test -p attune-core --test sensevoice_quality_gate` → 2 passed, CER 7.69%, exit 0
- `cargo test -p attune-core --lib` → 2587 passed / 0 failed / 2 ignored
- `cargo check -p attune-core` / `--no-default-features` / `-p attune-server` → all PASS
- `cargo build -p attune-agent-sdk --target wasm32-wasip1` → PASS
- `cargo clippy -p attune-core -p attune-server --all-targets` → 0 warnings in changed files
  (only pre-existing power.rs:154 warning remains)
- split-guard → OK (93 files = 4 gates + 45 half-A + 44 half-B)

## Disk

`df /data` → 241G avail (green). `/data/attune-target` = 48G. Worktree retained for review.

## Commits (7, one logic each — not pushed/merged/tagged)

```
99cd023 build(asr): add sherpa-rs (SenseVoice) behind asr-sensevoice feature
9ecf896 feat(asr): SenseVoice in-process provider (sherpa-onnx)
c868c81 feat(asr): catalog-driven AsrEngine abstraction + parser dispatch
46ecaa2 feat(ai-stack): dynamic ASR engine field + SenseVoice one-click fetch
14b07d0 test(asr): SenseVoice real-audio quality gate (CER 7.69% <= 15%)
33f1968 feat(catalog): document SenseVoice S8 fetch repo on amd/intel-win ASR
97d919e ci(asr): SenseVoice cross-platform smoke (linux + windows PENDING-VERIFY)
```

---

# 对抗 review 修复（2026-06-24 second pass）

Source: `reports/2026-06-24-sensevoice-adversarial-review.md` (Critical 2 · Important 4 · Minor 4).
All fixes TDD, one-logic-per-commit, on `feature/sensevoice-asr` (not pushed/merged).

## 🔴 #1 (make-or-break) — two libonnxruntime in ONE process: **PASS (verified)**

ort (embedding/rerank/OCR) and sherpa-rs (ASR) both `download-binaries` → two **dynamic**
libonnxruntime loaded in one process. Review I1 flagged possible dlopen clash (symbol
interposition / version skew / shared global Ort* state).

**`tests/onnxruntime_coexistence.rs`** (new): in the SAME process runs (a) a real `ort::Session`
inference on a tiny hand-built Add ONNX (no download — exercises ort's onnxruntime exactly like
the embedding path) and (b) a real sherpa SenseVoice transcription of zh.wav — in **both load
orders** (`ort_then_sherpa`, `sherpa_then_ort`) **+ interleaved repeat**. All assert success, no
crash/abort/symbol error (a SIGABRT would itself be the BLOCKED signal via non-zero exit).

```
running 3 tests
ort_then_sherpa_coexist ... [coexist ort→sherpa] ort=ok sherpa="开放时间早上9点至下午5点。"
sherpa_then_ort_coexist ... [coexist sherpa→ort] sherpa="开放时间早上9点至下午5点。" ort=ok
interleaved_ort_sherpa_repeat ... ok
test result: ok. 3 passed; 0 failed   REAL_EXIT=0
```

**Conclusion: NOT BLOCKED — coexistence is real and safe on linux.** Windows leg is wired as a
blocking CI gate (PENDING-VERIFY-on-windows-runner, same as the quality gate).

## 🔴 #2 mp3/non-WAV — **fixed (whisper fallback) + tested**

sherpa `read_audio_file` is WAV-only + 16kHz-i16-only (confirmed from sherpa-rs 0.6.8 source:
`bail!("sample rate must be 16000")`, hound `WavReader`). Parser feeds the raw temp file with its
original extension straight to sherpa → mp3/m4a/flac/non-16k WAV would `Err`. **Fix = transcribe-time
whisper fallback (#3 below), the more robust choice than adding a symphonia decoder** (whisper-cli
decodes containers natively; no thin-deb bloat). `tests/sensevoice_mp3_fallback.rs` (+ zh.mp3 asset,
ffmpeg-transcoded from zh.wav, 45KB): sherpa rejects mp3 directly (asserted); mp3 transcribes
non-empty via whisper fallback → `"开放时间早上九点至下午五点"`. **2 passed, exit 0.**

## 🔴 #3 transcribe-time whisper fallback — **really wired (not just a comment)**

`asr::transcribe_with_engine` SenseVoice arm: on **any** `transcribe_sensevoice` Err → if
`detect_asr_backend()` (whisper-cli + ggml) available, retry via `transcribe_audio` (whisper); else
surface the original SenseVoice Err (no silent swallow, no panic). Doc-comments in asr.rs +
asr_sensevoice.rs corrected to describe the real behavior (M1 + M3). Exercised by the mp3 test above
(whisper-cli + large-v3-turbo present on this host).

## #4 Cargo.toml linkage rationale — **corrected (I1)**

Old comment: "attune-core ALREADY **statically** links onnxruntime via ort" — inaccurate; ort is
`download-binaries` (dynamic). Corrected to: the conflict was sherpa's `static` feature bundling its
OWN onnxruntime `.a` (symbol collision with ort's onnxruntime); both on `download-binaries` → each
dynamic, no static-archive clash; two-dynamic-in-one-process explicitly cited to the coexistence test.

## #5 CPU quality evidence + catalog wording — **reconciled (C1)**

The quality gate (`sensevoice_quality_gate.rs`) AND the coexistence test BOTH construct the backend
with `provider="cpu"` → **7.69% is the CPU-provider int8 figure** (double evidence: spike + 主控复跑,
now also my two independent local runs). Catalog corrected: amd/intel-win ASR `ep: directml/openvino`
→ `ep: cpu` (sherpa doesn't use ORT EPs; `ep` was misleading), metric annotated "sherpa CPU provider —
int8". cpu-fallback `23.08%` note reconciled: it is a **different sensevoice config** (provenance
PENDING-VERIFY — source matrix report not in-repo, so NOT silently rewritten as wrong); whisper kept on
cpu-fallback for diarization + format breadth, not because int8-CPU is unfit. CI smoke confirmed to run
real on ubuntu (no `if:` guard, no `continue-on-error` — blocking).

## #6 spec committed — **done (C2)**

`docs/superpowers/specs/2026-06-24-sensevoice-asr-provider.md` force-added (`git add -f` — the dir is
gitignored but origin/develop already carries other force-added specs). Updated to match as-shipped:
§3 linkage deviation + coexistence citation, §7 real whisper fallback for mp3, §9 rc.5 packaging hard
gate + coexistence proof.

## #7 packaging rc.5 hard gate — **documented (I2)**

Not coded here (no tauri packaging in this worktree). Recorded as an **rc.5 HARD GATE** in spec §9:
`tauri.conf.json` resources / deb layout / Windows `.dll` set must bundle the **3 native shared libs**
(libonnxruntime / libsherpa-onnx-c-api / libsherpa-onnx-cxx-api, ~20MB/platform, build.rs fetches the
correct arch into `target/`) + rpath `$ORIGIN` + a **real packaged-artifact post-install ASR-load smoke**
per §7.3 (lib-missing must `SenseVoiceRecognizer::new` Err clean, not crash). RELEASE.md Known Limitations
to carry this now. The in-process design narrows but does not eliminate the bundling bug class — the
human error of a hand-copied wrong-OS binary is gone (per-target prebuilts), but bundling is the new SPOF.

## #8 minor (M2/M4)

M2 (`provider` field comment lists cuda/directml but only cpu constructed): left as-is — the field is a
real sherpa knob and the doc-comment already says attune keeps "cpu"; the catalog ep→cpu fix removes the
C1-feeding confusion. M4 (env-dependent `detect_none_when_assets_missing`): acceptable smoke, untouched.

## Second-pass verification (CARGO_TARGET_DIR=/data/attune-target TMPDIR=/data/tmp, ATTUNE_SENSEVOICE_MODEL_DIR set)

- `cargo test -p attune-core --test onnxruntime_coexistence` → 3 passed, exit 0
- `cargo test -p attune-core --test sensevoice_mp3_fallback` → 2 passed, exit 0
- `cargo test -p attune-core --test sensevoice_quality_gate` → 2 passed (CER 7.69%)
- `cargo test -p attune-core --lib` → see below (full re-run)
- `cargo clippy -p attune-core --all-targets` → clean in changed files (only pre-existing power.rs:154)
- `cargo check -p attune-core --no-default-features` → PASS (sherpa gated out, whisper-only)
- core split-guard → OK (95 files = 6 gates + 45 half-A + 44 half-B)

## Fix commits (6 more, one logic each — not pushed/merged/tagged)

```
b25dc0d test(asr): prove ort + sherpa libonnxruntime coexist in one process
026fd20 fix(asr): real transcribe-time whisper fallback for non-WAV (mp3) audio
740fcc3 docs(asr): correct onnxruntime linkage rationale + ASR CER provider attribution
158f471 ci(asr): run coexistence + mp3-fallback SenseVoice gates on both OSes
e19fa97 docs(spec): commit SenseVoice ASR provider 11-section spec (§3.1)
<this report commit follows>
```
