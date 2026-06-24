# SenseVoice ONNX ASR Provider（替代 whisper-cli 二进制）

> 2026-06-24。触发：Intel Windows 真机 E2E 发现 `bin\whisper-cli` 是 Linux ELF 装进 Win 包 → ASR 坏。
> 用户拍板"确定质量可行条件下开干"；feasibility spike PASS（CER 7.69%，详见下）。

## 1. 目标定位
桌面 ASR 从 whisper-cli **平台相关二进制**（打包易错：Linux ELF 误入 Win 包）切到 **SenseVoice ONNX（sherpa-onnx in-process）**，从构造上消除"按平台 bundle 二进制"这一 bug 类。对齐 catalog 已有的 `engine: sensevoice` 路由（之前无 provider 接它，故实际仍跑 whisper）。

## 2. 范围边界
- **做**：新增 in-process `SenseVoiceAsrProvider`（sherpa-rs 0.6.8）；ASR 引擎抽象（whisper | sensevoice 派发）；catalog 引擎选型驱动构造；sensevoice 模型进 S8 ModelStack runtime-fetch；ai_stack engine 字段动态化。
- **不做（保留）**：whisper 作 **CPU-tier 兜底**（benchmark：sensevoice 纯 CPU 参考 FAIL CER 23%，但 Win ONNX PASS 7.69%）+ diarization（whisperx/pyannote）暂不动。不删 whisper 路径。
- **后续**：sherpa-rs → 官方 k2-fsa Rust 绑定迁移；company-mirror 托管 sensevoice 模型。

## 3. 架构数据流
`audio → detect_asr_engine(catalog,hardware) → {SenseVoice: sherpa_rs::SenseVoiceRecognizer.transcribe(rate,&samples) | Whisper: whisper-cli subprocess} → text`
- 模型：`sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17`（`model.int8.onnx` 239MB + `tokens.txt`），runtime-fetch（HF `csukuangfj/...` + 后续 company-mirror），落 `models_dir()/asr/sensevoice/`。
- sherpa-onnx libs：build-time vendored。**实现偏离（2026-06-24，详见 impl report Deviations + 对抗 review I1）**：本节原拟 sherpa-rs `static`，实测 `static` 把 sherpa 自带 onnxruntime `.a` 与 ort crate 的 onnxruntime 同名符号撞（rust-lld duplicate symbol）。故 sherpa 与 ort **均用 `download-binaries`（动态）**，各自动态链接自己的 libonnxruntime；两个动态 libonnxruntime 同进程共存已由 `tests/onnxruntime_coexistence.rs` 实测验证安全（ort 真推理 + sherpa 真转写、双加载序、交错重复，全过无 crash/符号冲突）。代价 = 3 个 shared lib 须随包（见 §9 rc.5 打包硬门）。

## 4. 模块边界
- `attune-core/src/asr.rs`：引擎抽象（`enum AsrEngine { Whisper(AsrBackend), SenseVoice(SenseVoiceBackend) }` 或 trait）；`detect_asr_backend`→`detect_asr_engine`（catalog 驱动）。
- `attune-core/src/asr_sensevoice.rs`（新）：sherpa-rs provider。
- `attune-core/Cargo.toml`：`sherpa-rs = { version="0.6.8", features=["static"] }`（与 `tts` 互斥，不用 tts）。
- `attune-core/src/infer/`：catalog sensevoice 模型 source + ModelStack fetch。
- `attune-server/routes/ai_stack.rs`：engine 字段取自解析引擎（非硬编码 whisper.cpp）；ensure 拉 sensevoice。
- 调用点 `parser.rs:194/785`：改走引擎派发。

## 5. API 契约
- `detect_asr_engine() -> Option<AsrEngine>`；`transcribe_audio(engine,&Path)->Result<String>`（签名兼容，内部派发）。
- `GET /ai_stack` `asr.engine` ∈ {sensevoice, whisper.cpp}（动态）。

## 6. 扩展点
引擎抽象支持未来 rk-asr / 官方 k2-fsa 绑定，加变体即可。

## 7. 错误 + 边界
- sherpa init / 转写失败 → 若 whisper 可用则**transcribe-time 降级 whisper**（已实现于 `asr::transcribe_with_engine`，非仅注释承诺），否则 surface 原 SenseVoice error（graceful，不 panic、不 swallow）。
- **非-WAV 音频（mp3/m4a/flac/非 16k WAV）**：sherpa `read_audio_file` 仅 16k i16 WAV，对这些格式 `Err` → 经上面 transcribe-time fallback 走 whisper-cli（whisper 原生支持容器格式）。实测见 `tests/sensevoice_mp3_fallback.rs`（mp3 经 SenseVoice 派发 → whisper fallback 出 "开放时间…"）。
- 模型缺 → 触发 fetch；offline 且缺 → 明确 error-code `asr-model-offline-missing`。
- 空/超短音频 → 空串非 panic。

## 8. 成本契约
零 token（本地 ONNX）；CPU int8 ~7x realtime（spike 实测 800ms/5.6s）。⚡本地算力层。

## 9. 测试矩阵
- 单元：引擎解析（catalog tier→engine）、provider 构造、降级路径。
- **真音频集成（质量门）**：转写 `test_wavs/zh.wav` → CER ≤ 15%（benchmark 7.69%）。fixture 用 benchmark 的 zh.wav（小 wav 入 test assets 或 env 指路）。
- 边界：空/超短/非 16k 重采样。异常：缺模型/offline。
- 跨平台构建：Linux 本地真跑；**Windows = rc.5 CI 1-WAV smoke（PENDING-VERIFY，gate rc.5）**。
- **两-onnxruntime 同进程共存（make-or-break）**：`tests/onnxruntime_coexistence.rs` — ort 真 Session 推理 + sherpa 真转写同进程跑（双加载序 + 交错重复），断言全过无 crash/符号冲突。**本机实测 PASS（exit 0）**。
- 回归：whisper CPU-tier 路径 + diarization 不破。

### rc.5 打包硬门（对抗 review I2 — 必须，不是"以后"）
in-process 把"误装错-OS 二进制"bug 类消除，但**引入 3 个 native shared lib**（libonnxruntime / libsherpa-onnx-c-api / libsherpa-onnx-cxx-api，~20MB/平台）须随产物打包。本 worktree **不做 tauri 打包**，仅记清楚 rc.5 硬门要求：
1. `tauri.conf.json` resources / deb layout / Windows `.dll` set 收齐这 3 个 lib（per-target 正确架构，build.rs 已 fetch 进 `target/`）。
2. 设 rpath `$ORIGIN`（Linux）/ 同目录加载（Windows），使运行时 dlopen 找得到。
3. **真打包产物 post-install ASR-load smoke（per §7.3 本机部署验证）**：装 deb/MSI/AppImage → 起服 → 真转写一条音频 → 断言出字。lib 缺 → `SenseVoiceRecognizer::new` 须 Err 干净降级（不是裸 crash）。
4. 缺任一 = rc.5 不放行。RELEASE.md Known Limitations 现在就标"SenseVoice ASR 依赖 3 个随包 native lib，打包 gate 在 rc.5"。

## 10. 向后兼容
whisper AsrBackend + ggml 路径保留（CPU tier + diarization）；调用点签名兼容；老 settings ASR 配置不破。

## 11. 风险登记
- sherpa-rs README 称将弃用转官方 k2-fsa Rust 绑定 → 0.6.8 现可用，记迁移项（同引擎，CER 不变）。
- **Windows 构建 PENDING-VERIFY**（dist.json 有 msvc 预编译库，但未在真 runner 验）→ rc.5 CI smoke 硬门。
- 二进制体积 +sherpa-onnx libs（用 static 折叠）；build-time GitHub releases 网络依赖。
- 质量门已过（spike CER 7.69%）。
