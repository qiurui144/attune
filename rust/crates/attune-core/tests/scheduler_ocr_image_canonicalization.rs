use attune_core::parser::{parse_bytes_with_options, ParseOptions};
use base64::Engine;
use image_wire::GenericImageView;
use serde_json::Value;
use std::io::{Cursor, Read, Write};
use std::net::{TcpListener, TcpStream};

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

#[test]
fn scheduler_path_canonicalizes_real_jpeg_to_one_png_contract() {
    let jpeg = real_jpeg_fixture();
    assert!(jpeg.starts_with(&[0xff, 0xd8, 0xff]));

    let (base, scheduler) = start_scheduler_mock();
    let options = ParseOptions::default()
        .with_scheduler_base(Some(&base))
        .with_scheduler_timeout_ms(5_000);
    let (title, text) = parse_bytes_with_options(&jpeg, "receipt.jpg", &options).unwrap();
    assert_eq!(title, "JPEG canonical OCR text");
    assert_eq!(text, "JPEG canonical OCR text");

    let request = scheduler.join().unwrap();
    let top = request
        .pointer("/file_base64")
        .and_then(Value::as_str)
        .unwrap();
    let input = request
        .pointer("/input/file_base64")
        .and_then(Value::as_str)
        .unwrap();
    let x = request
        .pointer("/x/file_base64")
        .and_then(Value::as_str)
        .unwrap();
    assert_eq!(top, input);
    assert_eq!(top, x);

    for pointer in ["/content_type", "/input/content_type", "/x/content_type"] {
        assert_eq!(
            request.pointer(pointer).and_then(Value::as_str),
            Some("image/png"),
            "pointer={pointer}; request={request}"
        );
    }
    for pointer in ["/filename", "/input/filename", "/x/filename"] {
        assert_eq!(
            request.pointer(pointer).and_then(Value::as_str),
            Some("receipt.jpg"),
            "pointer={pointer}; request={request}"
        );
    }

    let png = base64::engine::general_purpose::STANDARD
        .decode(top)
        .unwrap();
    assert!(png.starts_with(PNG_SIGNATURE));
    assert_eq!(
        image_wire::load_from_memory(&png).unwrap().dimensions(),
        (8, 6)
    );
}

fn real_jpeg_fixture() -> Vec<u8> {
    let image = image_wire::DynamicImage::ImageRgb8(image_wire::RgbImage::from_fn(8, 6, |x, y| {
        image_wire::Rgb([(x * 23) as u8, (y * 31) as u8, ((x + y) * 17) as u8])
    }));
    let mut encoded = Cursor::new(Vec::new());
    image
        .write_to(&mut encoded, image_wire::ImageFormat::Jpeg)
        .unwrap();
    encoded.into_inner()
}

fn start_scheduler_mock() -> (String, std::thread::JoinHandle<Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let (submit_stream, _) = listener.accept().unwrap();
        let (mut submit_stream, submit_line, body) = read_request(submit_stream);
        assert!(
            submit_line.starts_with("POST /kb/tasks/kb.document.ocr_recognize:async "),
            "request={submit_line}"
        );
        respond_json(
            &mut submit_stream,
            202,
            serde_json::json!({
                "schema_version": "kb_task.v1",
                "scheduled_as": "async",
                "job_id": "jpeg-canonical-job",
                "status": "queued",
                "task": "kb.document.ocr_recognize",
                "model": "ocr-rec",
                "outputs": {}
            }),
        );

        let (job_stream, _) = listener.accept().unwrap();
        let (mut job_stream, job_line, _) = read_request(job_stream);
        assert!(
            job_line.starts_with("GET /jobs/jpeg-canonical-job "),
            "request={job_line}"
        );
        respond_json(
            &mut job_stream,
            200,
            serde_json::json!({
                "schema_version": "job_status.v2",
                "job_id": "jpeg-canonical-job",
                "task": "kb.document.ocr_recognize",
                "model": "ocr-rec",
                "scheduled_as": "async",
                "status": "done",
                "phase": "done",
                "outputs": {
                    "schema_version": "ocr_result.v1",
                    "task": "kb.document.ocr_recognize",
                    "status": "ok",
                    "engine": "integration-test-ocr",
                    "degraded": false,
                    "text": "JPEG canonical OCR text",
                    "layout": [],
                    "lines": [],
                    "pages": [{
                        "page_index": 0,
                        "width": 8,
                        "height": 6,
                        "text": "JPEG canonical OCR text",
                        "blocks": [],
                        "layout": [],
                        "confidence": 0.99
                    }]
                }
            }),
        );
        body
    });
    (format!("http://{addr}"), handle)
}

fn read_request(mut stream: TcpStream) -> (TcpStream, String, Value) {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let count = stream.read(&mut chunk).unwrap();
        assert!(count > 0, "connection closed before HTTP headers");
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while bytes.len().saturating_sub(header_end) < content_length {
        let count = stream.read(&mut chunk).unwrap();
        assert!(count > 0, "connection closed before HTTP body");
        bytes.extend_from_slice(&chunk[..count]);
    }
    let request_line = headers.lines().next().unwrap().to_string();
    let body = if content_length == 0 {
        Value::Null
    } else {
        serde_json::from_slice(&bytes[header_end..header_end + content_length]).unwrap()
    };
    (stream, request_line, body)
}

fn respond_json(stream: &mut TcpStream, status: u16, body: Value) {
    let body = body.to_string();
    let reason = if status == 202 { "Accepted" } else { "OK" };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
}
