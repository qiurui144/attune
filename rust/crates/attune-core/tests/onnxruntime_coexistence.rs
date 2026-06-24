//! 🔴 make-or-break: two libonnxruntime in ONE process.
//!
//! attune-core links onnxruntime **twice** at runtime:
//!   1. the `ort` crate (`download-binaries` → libonnxruntime.so/.dll) — embedding / rerank / OCR.
//!   2. sherpa-rs (`download-binaries` → its OWN libonnxruntime + libsherpa-onnx-c-api/-cxx-api)
//!      — SenseVoice ASR.
//!
//! The adversarial review (C-review I1) flagged that two dynamic libonnxruntime in one
//! address space could clash at `dlopen` (symbol interposition / version skew / global
//! `Ort*` singletons). If they DO clash → ASR-or-everything-else crashes at runtime, and
//! the whole "in-process SenseVoice" design is dead (must move ASR to a subprocess or make
//! sherpa reuse ort's onnxruntime).
//!
//! This test PROVES coexistence by, **in the same process**:
//!   (a) building + running a real `ort::Session` (ort's onnxruntime) on a tiny ONNX graph, then
//!   (b) constructing + running the sherpa SenseVoice recognizer (sherpa's onnxruntime) on real
//!       zh.wav,
//! and asserting BOTH succeed with no crash / abort / symbol error. It runs both orders
//! (ort-first and sherpa-first) to catch load-order-dependent breakage.
//!
//! If this test aborts (SIGABRT / SIGSEGV) instead of failing an assertion, that itself is
//! the BLOCKED signal — the process dies and the harness records a non-zero exit.

#![cfg(feature = "asr-sensevoice")]

use std::path::PathBuf;

/// A minimal but VALID ONNX model: `output = input + input` (single Add node, 1-D f32
/// tensor of dynamic length). Hand-built protobuf so the test needs no model download and
/// runs identically on CI (linux + windows). This is a *real* onnxruntime inference — it
/// links + initializes ort's libonnxruntime exactly like the embedding/OCR path does.
fn tiny_add_onnx() -> Vec<u8> {
    // ── protobuf wire-format helpers (onnx.proto3) ──
    fn tag(field: u32, wire: u32) -> u8 {
        ((field << 3) | wire) as u8
    }
    fn varint(mut v: u64, out: &mut Vec<u8>) {
        loop {
            let mut b = (v & 0x7f) as u8;
            v >>= 7;
            if v != 0 {
                b |= 0x80;
            }
            out.push(b);
            if v == 0 {
                break;
            }
        }
    }
    fn len_delim(field: u32, payload: &[u8], out: &mut Vec<u8>) {
        out.push(tag(field, 2));
        varint(payload.len() as u64, out);
        out.extend_from_slice(payload);
    }
    fn string_field(field: u32, s: &str, out: &mut Vec<u8>) {
        len_delim(field, s.as_bytes(), out);
    }
    fn varint_field(field: u32, v: u64, out: &mut Vec<u8>) {
        out.push(tag(field, 0));
        varint(v, out);
    }

    // ValueInfoProto for a 1-D float tensor with a single dynamic dim "N".
    fn value_info(name: &str) -> Vec<u8> {
        // TypeProto.Tensor (Type.tensor_type = field 1)
        // Tensor { elem_type=1 (FLOAT) field1; shape field2 }
        let mut dim = Vec::new(); // TensorShapeProto.Dimension { dim_param "N" = field 2 }
        string_field(2, "N", &mut dim);
        let mut shape = Vec::new(); // TensorShapeProto { dim (field1, repeated) }
        len_delim(1, &dim, &mut shape);
        let mut tensor = Vec::new(); // Tensor { elem_type field1; shape field2 }
        varint_field(1, 1, &mut tensor); // FLOAT
        len_delim(2, &shape, &mut tensor);
        let mut type_proto = Vec::new(); // TypeProto { tensor_type field1 }
        len_delim(1, &tensor, &mut type_proto);
        let mut vi = Vec::new(); // ValueInfoProto { name field1; type field2 }
        string_field(1, name, &mut vi);
        len_delim(2, &type_proto, &mut vi);
        vi
    }

    // NodeProto: Add(input, input) -> output
    let mut node = Vec::new(); // NodeProto { input(f1, rep); output(f2, rep); op_type(f4) }
    string_field(1, "input", &mut node);
    string_field(1, "input", &mut node);
    string_field(2, "output", &mut node);
    string_field(4, "Add", &mut node);

    // GraphProto { node(f1, rep); name(f2); input(f11, rep ValueInfo); output(f12, rep) }
    let mut graph = Vec::new();
    len_delim(1, &node, &mut graph);
    string_field(2, "coexist", &mut graph);
    len_delim(11, &value_info("input"), &mut graph);
    len_delim(12, &value_info("output"), &mut graph);

    // OpsetIdProto { domain(f1 ""); version(f2 13) }
    let mut opset = Vec::new();
    varint_field(2, 13, &mut opset);

    // ModelProto { ir_version(f1); opset_import(f8, rep); producer(f2); graph(f7) }
    let mut model = Vec::new();
    varint_field(1, 7, &mut model); // ir_version 7
    string_field(2, "attune-coexist-test", &mut model); // producer_name
    len_delim(7, &graph, &mut model); // graph
    len_delim(8, &opset, &mut model); // opset_import
    model
}

/// Run the tiny Add model through ort's onnxruntime. Returns the output (input doubled).
/// Any failure here = ort's libonnxruntime is broken in this process.
fn run_ort_session() -> Vec<f32> {
    use ort::session::Session;
    use ort::value::Tensor;

    let model_bytes = tiny_add_onnx();
    let mut session = Session::builder()
        .expect("ort Session::builder")
        .commit_from_memory(&model_bytes)
        .expect("ort commit_from_memory (tiny Add) — ort's onnxruntime init");

    let input = vec![1.0f32, 2.0, 3.0, 4.0];
    let tensor = Tensor::<f32>::from_array((vec![4usize], input)).expect("input tensor");
    let outputs = session
        .run(ort::inputs! { "input" => tensor })
        .expect("ort run (Add) — ort's onnxruntime execution");
    let (_shape, flat) = outputs["output"]
        .try_extract_tensor::<f32>()
        .expect("extract ort output");
    flat.to_vec()
}

/// Locate the SenseVoice model dir (same resolution order as the quality gate).
fn sensevoice_model_dir() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("ATTUNE_SENSEVOICE_MODEL_DIR") {
        let p = PathBuf::from(d);
        if p.join("model.int8.onnx").exists() {
            return Some(p);
        }
    }
    let runtime = attune_core::asr_sensevoice::sensevoice_model_dir();
    if runtime.join("model.int8.onnx").exists() {
        return Some(runtime);
    }
    let vlm = PathBuf::from(
        "/data/company/project/vlm-llm-benchmark/datasets/asr/models/\
sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17",
    );
    if vlm.join("model.int8.onnx").exists() {
        return Some(vlm);
    }
    None
}

/// Run sherpa SenseVoice on the bundled zh.wav. Returns the transcript.
/// Any failure here (after model present) = sherpa's onnxruntime is broken in this process.
fn run_sherpa_sensevoice() -> String {
    use attune_core::asr_sensevoice::{transcribe_sensevoice, SenseVoiceBackend};
    let dir = sensevoice_model_dir().unwrap_or_else(|| {
        panic!(
            "SenseVoice model assets missing (set ATTUNE_SENSEVOICE_MODEL_DIR). \
             Coexistence test must run sherpa's onnxruntime against the real model."
        )
    });
    let backend =
        SenseVoiceBackend::from_paths(dir.join("model.int8.onnx"), dir.join("tokens.txt"))
            .expect("construct SenseVoice backend");
    let wav = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("assets")
        .join("zh.wav");
    assert!(wav.exists(), "bundled zh.wav missing at {}", wav.display());
    transcribe_sensevoice(&backend, &wav).expect("sherpa SenseVoice transcribe")
}

/// ort first, then sherpa — same process. Both must succeed (no crash / symbol clash).
#[test]
fn ort_then_sherpa_coexist() {
    let doubled = run_ort_session();
    assert_eq!(
        doubled,
        vec![2.0, 4.0, 6.0, 8.0],
        "ort onnxruntime produced wrong result"
    );

    let transcript = run_sherpa_sensevoice();
    assert!(
        transcript.contains("开放时间"),
        "sherpa onnxruntime produced wrong/empty transcript after ort ran: {transcript:?}"
    );
    eprintln!("[coexist ort→sherpa] ort=ok sherpa=\"{transcript}\"");
}

/// sherpa first, then ort — catch load-order-dependent breakage (ort init after sherpa
/// already pulled in its own libonnxruntime).
#[test]
fn sherpa_then_ort_coexist() {
    let transcript = run_sherpa_sensevoice();
    assert!(
        transcript.contains("开放时间"),
        "sherpa onnxruntime produced wrong/empty transcript: {transcript:?}"
    );

    let doubled = run_ort_session();
    assert_eq!(
        doubled,
        vec![2.0, 4.0, 6.0, 8.0],
        "ort onnxruntime broke AFTER sherpa initialized its own onnxruntime"
    );
    eprintln!("[coexist sherpa→ort] sherpa=\"{transcript}\" ort=ok");
}

/// Both engines exercised twice, interleaved, in one process — stress the shared global
/// onnxruntime state (env / arena allocators) for re-entrancy across the two libs.
#[test]
fn interleaved_ort_sherpa_repeat() {
    for i in 0..2 {
        let doubled = run_ort_session();
        assert_eq!(doubled, vec![2.0, 4.0, 6.0, 8.0], "ort run {i}");
        let transcript = run_sherpa_sensevoice();
        assert!(
            transcript.contains("开放时间"),
            "sherpa run {i}: {transcript:?}"
        );
    }
}
