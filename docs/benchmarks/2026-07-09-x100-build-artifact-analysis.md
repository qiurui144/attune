# X100 Build Artifact Analysis

Date: 2026-07-09

Scope: scheduler-runtime slimming, X100/RVA23 cross-compilation with the
SpacemiT private toolchain, and live artifact validation on `192.168.100.140`.
Older host `x86_64` debug artifacts are kept below as background only.

## Live 140 Result

Host `192.168.100.140`:

| Field | Value |
| --- | --- |
| Hostname | `k3` |
| OS | Bianbu 4.0.1 |
| Kernel | `6.18.3-generic` |
| Arch | `riscv64` |
| CPU topology | CPU 0-7 `Spacemit(R) X100`, CPU 8-15 `Spacemit(R) A100` |
| ISA | `rv64imafdcvh`, `zba/zbb/zbc/zbs`, `zve*`, `zvfh`, `zvbb`, crypto vector extensions |
| Memory | 15 GiB total, about 14 GiB available during validation |
| Root disk | 117 GiB total, about 105 GiB available |
| glibc | 2.43 |

Build command:

```bash
bash scripts/build-optimized.sh --profile rva23 \
  --package attune-server \
  --features scheduler-runtime \
  --no-default-features
```

Build profile:

| Layer | Setting |
| --- | --- |
| Toolchain | `/data/RV/rv-spacemit-toolchain/spacemit-toolchain-linux-glibc-x86_64-v1.2.2` |
| C/C++ compiler | `riscv64-unknown-linux-gnu-gcc` 15.2.0 |
| Rust target | `riscv64gc-unknown-linux-gnu` |
| Rust flags | `-C target-cpu=generic-rv64 -C target-feature=+zba,+zbb,+zbs` |
| C/C++ flags | `-march=rv64gcv_zba_zbb_zbs_zvfh_zvbb -mabi=lp64d` |

The Rust `+v` target feature is intentionally not enabled by default. Rust
1.95/LLVM 22 crashed in RISC-V loop vectorization during ThinLTO for the final
server binary. RVV is still enabled for native C/C++ kernels, which is where the
current vector-search hot path lives.

Produced artifact:

| Artifact | Size | Type | Section size |
| --- | ---: | --- | ---: |
| first scheduler-runtime pass | 62 MB | RISC-V PIE, stripped | 64,623,395 bytes |
| second pass, rich export disabled | 32 MB | RISC-V PIE, stripped | 33,542,443 bytes |
| 2026-07-09 SRAS/e2e pass | 32 MB | RISC-V PIE, stripped | sha256 `9bd1d5c8e0c19f1c06088553c5d997114c9c8cac9a5dec500a229b9772d3dc01` |

The second pass moved rich artifact export backends behind
`artifact-export-rich`, which stays enabled in default builds but is not part of
the scheduler-runtime profile. The removed scheduler-runtime tree includes
`typst`, `typst-as-lib`, `typst-pdf`, `typst-library`, `docx-rs`,
`rust_xlsxwriter`, `wasmi`, `wasmparser`, `read-fonts`, `write-fonts`, `krilla`,
and `hayro`.

The route contract remains explicit: in scheduler-runtime, Markdown/CSV export
still works, while xlsx/docx/pdf return HTTP 400 with code
`unsupported-artifact` and a message to enable `artifact-export-rich`.

Dynamic dependencies on 140:

- `libstdc++.so.6`
- `libgcc_s.so.1`
- `libm.so.6`
- `libc.so.6`
- `/lib/ld-linux-riscv64-lp64d.so.1`

The artifact loads on 140. `--help`, `--version`, and `--bootstrap-only` all
exit successfully. `bootstrap-only` reports that OCR/ASR/embedding/rerank model
lifecycle is scheduler-owned.

ELF attributes include RVV and half/vector bitmanip extensions:

```text
rv64..._b1p0_v1p0_..._zba1p0_zbb1p0_zbc1p0_zbs1p0_zvbb1p0_...
zve32f1p0_zve64d1p0_zve64f1p0_zvfh1p0_zvfhmin1p0_zvl128b1p0
```

The post-build audit is now scripted:

```bash
ATTUNE_RVV_AUDIT_SAMPLE_LINES=3 \
  bash scripts/audit-rvv-vectorization.sh \
  rust/target/riscv64gc-unknown-linux-gnu/release/attune-server-headless
```

Audit result on the current RVA23 artifact:

| Component | RVV evidence |
| --- | ---: |
| `attune-server-headless` ELF attributes | `attribute_rvv_evidence=1` |
| `attune-server-headless` disassembly | `rvv_instruction_lines=34011` |
| `libnumkong-*.rlib` disassembly | `rvv_instruction_lines=7925` per observed rlib |
| `libusearch-*.rlib` disassembly | `rvv_instruction_lines=9676` per observed rlib |

Observed mnemonics include `vsetvli`, `vsetivli`, `vle*.v`, `vse*.v`,
`vs1r.v`, and `vfmul.vv`. This confirms the current artifact has RVV evidence
in both the final server binary and the core vector-search libraries.

## Clear Conclusions

1. Scheduler-runtime slimming is effective. The checked dependency graph no
   longer includes `ort`, `ort-sys`, `sherpa-rs`, `sherpa-rs-sys`,
   `kreuzberg-paddle-ocr`, `imageproc`, `wasmtime`, or `wasmtime-wasi`.

2. Rich export must stay out of the X100 scheduler-runtime image by default.
   Moving xlsx/docx/pdf backends behind `artifact-export-rich` reduced the
   RISC-V release binary from 62 MB to 32 MB while preserving md/csv export and
   the export IR/privacy gate.

3. Use `/data/RV/rv-spacemit-toolchain/...v1.2.2` for RVA23 artifacts. The
   system GCC 13 can compile basic RVV, but the SpacemiT GCC 15.2 toolchain has
   better extension macro support and matches the Bianbu 4.0.1 deployment host.

4. RVV should be enabled through native kernel C/C++ flags first, not through
   global Rust `+v`. Current Rust `+v` release ThinLTO is unstable for this
   binary. This is a toolchain/compiler constraint, not an Attune logic bug.

5. The Attune server process is now a viable X100 control-plane artifact:
   policy, storage, retrieval, scheduler contracts, and web/API serving remain
   local; OCR/ASR/embedding/rerank/LLM/VLM execution remains scheduler-owned.

6. Remaining size/performance work is not inference-runtime removal anymore. It
   is now component-specific: Tantivy/Jieba/tokenizers, usearch/numkong,
   SQLite/Git, compression, and crypto. Rich export is already separated.

7. The highest-risk remaining X100 path is vector retrieval behavior under real
   long-text KB load. The binary and `usearch/numkong` now have post-build RVV
   instruction evidence, but runtime diagnostics and long-document
   recall/latency gates still matter.

8. The 48-document airplane-manual gate now proves the retrieval shape on the
   X100 pilot: Hit@5 = 1.0, Hit@10 = 1.0, Recall@10 = 0.952, MRR@10 = 0.897,
   search p95 = 1.28s. The remaining search miss is performance only: p50 =
   906ms is above the current 800ms target.

9. The answer path before the extractive-answer fast path was not a 10s
   full-suite path. API chat citation hit was 1.0, but answer accuracy was
   0.810 and p95 was 20.9s. This pointed to scheduler answer worker selection,
   output length, safety refusal templates, and prompt/cache reuse, not missing
   vector recall.

10. After adding the Attune-owned local extractive answer path for
    high-confidence source lookup and safety-refusal queries, the 42-query API
    chat suite reached citation hit 1.0, answer accuracy 1.0, term hit 1.0,
    unsafe rate 0.0, p50 864ms, p95 1.59s, and max 12.53s. The Web UI
    `a320_qrh_abnormal` e2e passed at 9.26s including UI reveal, visible
    citations, and visible local scheduler status.

## Current Artifacts

Observed deployable/debug artifacts:

| Artifact | File size | ELF machine | Notes |
| --- | ---: | --- | --- |
| `rust/target/debug/attune-server-headless` | 470 MB | x86-64 | PIE, debug info, not stripped |
| `rust/target/debug/libonnxruntime.so` | 15 MB | x86-64 | stripped shared object |
| `rust/target/debug/libsherpa-onnx-c-api.so` | 5.0 MB | x86-64 | shared object, not stripped |
| `rust/target/debug/libsherpa-onnx-cxx-api.so` | 76 KB | x86-64 | shared object |

Section size from `size`:

| Artifact | text | data | bss |
| --- | ---: | ---: | ---: |
| `attune-server-headless` | 147,742,237 | 7,184,072 | 67,124 |
| `libonnxruntime.so` | 15,336,645 | 166,296 | 154,096 |
| `libsherpa-onnx-c-api.so` | 3,843,799 | 76,016 | 107,688 |

`attune-server-headless` dynamic dependencies include `libsherpa-onnx-c-api.so`,
`libstdc++`, `libz`, `liblzma`, `libmvec`, and libc. The main binary does not
carry an `$ORIGIN` rpath for `libsherpa-onnx-c-api.so`, so packaging must either
place the library in the loader path or avoid linking this path in X100
scheduler-only artifacts.

## Symbol Weight

Approximate grouped symbol sizes in `attune-server-headless`:

| Group | Symbol bytes | Interpretation |
| --- | ---: | --- |
| ONNX Runtime / sherpa | 13.9 MB | in-process inference footprint |
| ingest/media/export | 8.5 MB | PDF/audio/image/Typst/export parsing/rendering |
| wasm/cranelift | 8.4 MB | plugin WASM runtime |
| Attune server/core | 7.1 MB | API, policy, state, routing |
| crypto/hash | 4.7 MB | aws-lc/ring/blake3/crypto support |
| usearch/numkong | 4.1 MB | vector distance/index kernels |
| tokenizer/regex | 1.4 MB | tokenization and string matching |
| tantivy | 1.1 MB | full-text search |
| sqlite | 0.6 MB | bundled SQLite |

This is a debug artifact, so absolute size is not a release-size estimate. The
relative grouping is still useful: inference, WASM, media ingest, and vector
search are the meaningful native boundaries.

## Feature Findings

`cargo check -p attune-server --no-default-features` currently fails because
`attune-core` requires exactly one of `ort-bundled` or `ort-dynamic`:

```text
one of `ort-bundled` (default, download-binaries) or `ort-dynamic`
(load-dynamic) must be enabled
```

`cargo check -p attune-server --no-default-features --features ort-dynamic`
passes. That build removes default `sherpa-rs`, `symphonia`, and `wasmtime`, but
still includes these production dependencies:

- `ort`
- `kreuzberg-paddle-ocr`
- `image` / `imageproc`
- `usearch` / `numkong`
- `tantivy` / `tantivy-jieba`
- `tokenizers`
- `rusqlite`

The source-level scheduler boundary audit passes. The remaining mismatch is
therefore build-shape, not route-level direct invocation: Attune no longer needs
server/UI code to call concrete local runtimes directly, but the default and
minimal production dependency graph still carries local inference/OCR runtime
crates.

## X100 Optimization Targets

### P0: X100 Attune Server Build Shape

Target program: `attune-server-headless` when deployed on the X100 appliance or
same class CPU host.

Required optimization:

- Add a true scheduler-only build surface that does not require ORT linkage.
- Make `ort`, `kreuzberg-paddle-ocr`, and image preprocessing dependencies
  optional behind local-inference/nontext features.
- Keep rich xlsx/docx/pdf export behind `artifact-export-rich`; the X100
  scheduler-runtime artifact should keep only md/csv foreground export.
- Keep scheduler DTO/client, retrieval policy, context admission, privacy, cloud
  fallback, SQLite store, and local vector/full-text search in the Attune binary.
- Keep `wasm-runtime` off by default for X100 images unless plugin execution is
  explicitly required.
- Keep ASR/OCR/VLM/embedding/rerank/LLM workers out of the Attune process; those
  belong in the scheduler worker artifact.

This is the highest priority because it directly reduces binary size, native
cross-compile risk, dynamic-library packaging, and CPU/RAM pressure on X100.

### P0: Vector Search Path

Target components:

- `attune-core::vectors::VectorIndex`
- `attune-core::memory::MemoryVectorIndex`
- `usearch` + `numkong`

Current status:

- Attune uses `usearch` with F16 scalar storage for HNSW vector search.
- `usearch` enables `numkong` by default.
- Current host artifact built x86 probes (`haswell`, `skylake`, etc.).
- `numkong` has RISC-V probes for `rv64gcv`, `rv64gcv_zvfh`,
  `rv64gcv_zvfbfwma`, and `rv64gcv_zvbb`.
- The RVA23 build uses the SpacemiT GCC 15.2 toolchain with
  `-march=rv64gcv_zba_zbb_zbs_zvfh_zvbb`; Rust codegen remains
  `generic-rv64 + zba/zbb/zbs` because global Rust `+v` crashed during release
  ThinLTO.
- The current long-text result shows algorithmic retrieval changes mattered
  more than raw vector-kernel speed for correctness: metadata fallback,
  chunk-hit de-duplication, and source-aware SRAS moved Hit@5 to 1.0.

Required optimization:

- Produce an X100/RVA23 artifact and verify that `numkong` actually builds and
  reports RVV/RVV-half acceleration.
- Add a vector-index microbenchmark for scalar vs RVV build:
  insert throughput, search p50/p95/p99, recall@K, RSS.
- Emit runtime acceleration metadata into diagnostics/benchmark output.
- Keep this path in Attune unless vector DB ownership is moved behind scheduler
  as a retrieval service.
- Add an e2e-coupled search profile: the vector microbenchmark must be reported
  beside airplane-manual Hit@K/MRR/p50/p95 so RVV work cannot improve synthetic
  distance throughput while regressing retrieval quality.

### P1: Retrieval Planner, Tantivy, Tokenization

Target components:

- `tantivy`
- `tantivy-jieba`
- `tokenizers`
- `regex` / `memchr`
- SRAS retrieval planning and index partition selection

Required optimization:

- Prefer algorithmic and pipeline optimization over hand-written RVV:
  partition pruning, bounded top-k, segment cache, BM25/vector fusion ordering,
  and rerank budget control.
- Keep a benchmark lane for long-document KB search under X100 profile.
- Do not invest in custom RVV for Tantivy/tokenization until profiling shows
  this path dominates p95 query latency.

### P1: Scheduler Worker Artifacts

Target programs live outside Attune's main process:

- embedding worker
- rerank worker
- OCR/layout worker
- ASR worker
- LLM/VLM worker

Required optimization:

- Optimize these in the scheduler repo/build, not in Attune product code.
- For X100, use RVA23/RVV/IME-capable builds of the actual inference runtimes.
- Scheduler must report capabilities and measured throughput through the
  scheduler contract; Attune consumes that metadata for admission and routing.

### P2: SQLite, Crypto, Compression, Git

Target components:

- `rusqlite` / bundled SQLite
- `aws-lc`, `ring`, `aes-gcm`, `sha2`, `blake3`, `argon2`
- `zstd`, `flate2`, `bzip2`, `libgit2`

Required optimization:

- SQLite: tune transaction batching, WAL/checkpoint policy, statement reuse, and
  index layout before ISA work.
- Crypto/hash: do not fork or hand-optimize for RVV unless profiling shows a real
  ingest/backup bottleneck.
- Compression/git/media import: keep off the foreground chat path; optimize only
  if ingest p95 or indexing throughput requires it.

## Projects That Need Our Optimization

1. `attune-server-headless` X100 profile
   - true scheduler-only build
   - no mandatory ORT
   - no default sherpa/wasmtime in X100 image
   - release build with `rva23` flags and packaging checks

2. `attune-core` feature topology
   - split local inference from scheduler client/contracts
   - make ORT/OCR dependencies optional
   - allow `--no-default-features --features scheduler-runtime` or equivalent

3. `attune-core` vector retrieval
   - verify `usearch/numkong` RVV on X100
   - add microbench and diagnostics
   - preserve recall/latency gates for long-document KB

4. Long-text KB e2e
   - keep the airplane-manual dataset as the regression gate
   - measure API and web UI path
   - enforce 10s response target and citation correctness under X100/scheduler profile

5. Scheduler worker builds
   - embedding/rerank/OCR/ASR/LLM/VLM are scheduler worker optimization targets
   - Attune should not carry platform-specific inference branches for those workers

## Projects That Do Not Need Priority X100 Optimization

- Browser extension and web UI bundle.
- Desktop wrapper unless it is deployed directly on the X100 appliance.
- Cloud account/member flows.
- Crypto/hash libraries, unless profiling proves they dominate ingest or backup.
- Typst/export rendering, unless X100 is expected to generate large PDFs in the
  foreground path.
- WASM plugin runtime for the first X100 profile; keep it opt-in.

## Recommended Next Build Gates

1. Add `scheduler-only` or `scheduler-runtime` feature profile.
2. `cargo check -p attune-server --no-default-features --features scheduler-runtime`
   must pass without compiling `ort`, `sherpa-rs`, `kreuzberg-paddle-ocr`, or
   `wasmtime`.
3. `scripts/build-optimized.sh --profile rva23 --package attune-server
   --no-default-features --features scheduler-runtime` must produce the X100
   server artifact.
4. Run artifact audit:
   - `file`
   - `readelf -d`
   - `size`
   - `cargo tree -e normal,features`
   - vector RVV runtime diagnostics
5. Run long-text API and web e2e against the X100 scheduler profile:
   - search gate: require Hit@5/Hit@10/Recall@10/MRR@10 and p50/p95 latency
   - API chat gate: require citation hit, answer accuracy, safety refusal, and
     full-suite p95
   - Web UI gate: require indexed item visibility, answer/citation visibility,
     scheduler status rendering, and visible latency
