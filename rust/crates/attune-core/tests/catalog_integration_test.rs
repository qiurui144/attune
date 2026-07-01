//! 集成 E2E:mock company-mirror 服 catalog → S8 download_with_failover → parse → resolve。
//!
//! 覆盖 spec §9 「集成」行:mock company-mirror 服 catalog → 拉取 → 校验 → resolve。
//! 用 std::net::TcpListener 起一个一次性 HTTP server(零新依赖),按 HF-resolve URL
//! (`{endpoint}/{repo}/resolve/main/{file}`)返回 catalog 字节。
//!
//! 注:不在本机跑任何模型(§1.6);这里只测 catalog YAML 字节的下载→解析→选型管道,
//! 不加载任何 ONNX / LLM。

use attune_core::infer::catalog::{Catalog, Role};
use attune_core::infer::model_source::ModelSource;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

/// 起一个一次性 HTTP server,对**任意** GET 返回 `body`。返回 `http://127.0.0.1:<port>`。
/// 处理固定数量请求后退出(避免线程泄漏)。
fn spawn_mock_server(body: &'static str, max_requests: usize) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        tx.send(()).ok();
        for _ in 0..max_requests {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf); // drain request line/headers (best-effort)
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });
    rx.recv().ok(); // wait until server thread is up
    format!("http://{addr}")
}

const VALID_CATALOG: &str = "schema_version: 1\n\
tiers:\n\
\x20\x20amd-win:\n\
\x20\x20\x20\x20ocr: {engine: rapidocr, ep: directml, verdict: pass, source: 'test:1'}\n\
\x20\x20intel-win:\n\
\x20\x20\x20\x20ocr: {engine: rapidocr, ep: openvino, verdict: pass, source: 'test:2'}\n\
\x20\x20cpu-fallback:\n\
\x20\x20\x20\x20embedding: {repo: 'Xenova/bge-m3', file: 'onnx/model_quantized.onnx', ep: cpu, verdict: pass, source: 'test:3'}\n";

#[test]
fn mock_mirror_serves_catalog_download_parse_resolve() {
    // 起 mock mirror,直接走 S8 download_with_failover 拉 catalog 字节 → parse → resolve。
    let endpoint = spawn_mock_server(VALID_CATALOG, 4);
    let src = ModelSource {
        id: "mock-mirror".into(),
        endpoint: endpoint.clone(),
        priority: 100,
        region_hint: None,
        coverage: attune_core::infer::model_source::SourceCoverage::Full,
    };

    let tmp = tempfile::tempdir().unwrap();
    let dst = tmp.path().join("model-catalog.yaml");
    let used = attune_core::infer::model_source::download_with_failover(
        std::slice::from_ref(&src),
        attune_core::infer::catalog::CATALOG_REPO,
        attune_core::infer::catalog::CATALOG_FILE,
        &dst,
    )
    .expect("download served catalog");
    assert_eq!(used, "mock-mirror");

    let body = std::fs::read_to_string(&dst).unwrap();
    let cat = Catalog::parse(&body).expect("served catalog parses");
    // resolve 走完整选型 — Intel OCR 必须 OpenVINO,AMD OCR DirectML(OCR EP 规则的数据侧)。
    assert_eq!(cat.resolve("intel-win", Role::Ocr).unwrap().ep, "openvino");
    assert_eq!(cat.resolve("amd-win", Role::Ocr).unwrap().ep, "directml");
    // 缺 tier 回退 cpu-fallback。
    assert_eq!(
        cat.resolve("nope", Role::Embedding).unwrap().repo,
        "Xenova/bge-m3"
    );
}

#[test]
fn fetch_remote_unsigned_falls_back_to_builtin() {
    // R4 安全默认:远程 catalog 无有效签名(当前无信任锚)→ fetch_remote_or_builtin 回退内置 baseline。
    // mock 既服 catalog 又服 .sig(都返回 VALID_CATALOG 字节;签名锚为空 → 必拒)。
    let endpoint = spawn_mock_server(VALID_CATALOG, 4);
    let src = ModelSource {
        id: "mock-mirror".into(),
        endpoint,
        priority: 100,
        region_hint: None,
        coverage: attune_core::infer::model_source::SourceCoverage::Full,
    };
    let tmp = tempfile::tempdir().unwrap();
    let dst = tmp.path().join("model-catalog.yaml");
    let sig = tmp.path().join("model-catalog.yaml.sig");

    let cat = attune_core::infer::catalog::fetch_remote_or_builtin(
        std::slice::from_ref(&src),
        &dst,
        &sig,
    );
    // 回退内置 baseline → intel-win OCR openvino(来自 default.yaml,非 mock)。
    let ocr = cat.resolve("intel-win", Role::Ocr).unwrap();
    assert_eq!(ocr.ep, "openvino");
    // 内置 baseline 的 source 引 bench reports(非 mock 的 'test:2')。
    assert!(
        ocr.source.contains("33-34"),
        "fell back to built-in baseline, not mock catalog"
    );
}

#[test]
fn fetch_remote_all_sources_dead_falls_back_to_builtin() {
    // 所有源不可达(端口无服务)→ fetch_remote_or_builtin 回退内置 baseline,不 panic。
    let src = ModelSource {
        id: "dead".into(),
        endpoint: "http://127.0.0.1:1".into(), // 不可达
        priority: 100,
        region_hint: None,
        coverage: attune_core::infer::model_source::SourceCoverage::Full,
    };
    let tmp = tempfile::tempdir().unwrap();
    let cat = attune_core::infer::catalog::fetch_remote_or_builtin(
        std::slice::from_ref(&src),
        &tmp.path().join("c.yaml"),
        &tmp.path().join("c.yaml.sig"),
    );
    assert_eq!(cat.resolve("amd-win", Role::Ocr).unwrap().ep, "directml");
}
