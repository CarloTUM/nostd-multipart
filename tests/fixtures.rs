//! Real-client fixtures captured by `tests/fixtures/capture/capture.sh` from
//! curl -F, Python `requests` and Node's `FormData` (undici) through a local HTTP
//! server. Each fixture is parsed from its captured Content-Type and the exact
//! event stream is asserted. Bodies are also checked for chunking invariance
//! against the oracle.

mod common;
use common::*;
use nostd_multipart::Parser;
use OEvent::*;

const BIN: &[u8] = b"hello\r\nworld\r\n--\r\n--not-a-boundary\r\n\x00\x01\xff binary";
const TXT: &[u8] = b"line1\nline2\n";

fn h(n: &str, v: &str) -> OEvent {
    Header(n.as_bytes().to_vec(), v.as_bytes().to_vec())
}
fn d(x: &[u8]) -> OEvent {
    Data(x.to_vec())
}

fn fixture(name: &str) -> (Vec<u8>, Vec<u8>) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");
    (
        std::fs::read(format!("{dir}{name}.ct")).unwrap(),
        std::fs::read(format!("{dir}{name}.body")).unwrap(),
    )
}

fn check(name: &str, expected: Vec<OEvent>) {
    let (ct, body) = fixture(name);
    let p = Parser::<512, 16>::from_content_type(&ct).unwrap();
    let boundary = p.boundary().to_vec();
    let got = check_against_oracle::<512, 16>(&boundary, &body, &standard_chunkings(body.len(), 64, 7));
    assert_eq!(got, (expected, None), "fixture {name}");
    // 1 KiB-ish buffer with realistic reads of 1..=37 bytes, driven through
    // from_content_type on the raw header value.
    let got2 = run_chunked::<512, 16>(&boundary, &body, &standard_chunkings(body.len(), 1, 99)[1]);
    assert_eq!(got2, got);
}

#[test]
fn curl() {
    let (_, body) = fixture("curl");
    assert!(body.starts_with(b"--------------------------ksWXkr5pRWPWD14b4N9oTM\r\n"));
    check(
        "curl",
        vec![
            PartStart,
            h("Content-Disposition", "form-data; name=\"field\""),
            HeadersEnd,
            d(b"plain text"),
            PartEnd,
            PartStart,
            h("Content-Disposition", "form-data; name=\"firmware\"; filename=\"upload.bin\""),
            h("Content-Type", "application/octet-stream"),
            HeadersEnd,
            d(BIN),
            PartEnd,
            PartStart,
            h("Content-Disposition", "form-data; name=\"odd\"; filename=\"f\u{ef}le %22q%22.bin\""),
            h("Content-Type", "application/octet-stream"),
            HeadersEnd,
            d(BIN),
            PartEnd,
            Finished,
        ],
    );
}

#[test]
fn curl_edge() {
    check(
        "curl_edge",
        vec![
            PartStart,
            h("Content-Disposition", "form-data; name=\"empty\""),
            HeadersEnd,
            PartEnd,
            PartStart,
            h("Content-Disposition", "form-data; name=\"multi\""),
            HeadersEnd,
            d(b"a\r\nb"),
            PartEnd,
            PartStart,
            h("Content-Disposition", "form-data; name=\"\u{fc}n\u{ef}code\""),
            HeadersEnd,
            d("ü".as_bytes()),
            PartEnd,
            Finished,
        ],
    );
}

#[test]
fn python_requests() {
    check(
        "requests",
        vec![
            PartStart,
            h("Content-Disposition", "form-data; name=\"field\""),
            HeadersEnd,
            d(b"plain text"),
            PartEnd,
            PartStart,
            h("Content-Disposition", "form-data; name=\"empty\""),
            HeadersEnd,
            PartEnd,
            PartStart,
            h("Content-Disposition", "form-data; name=\"firmware\"; filename=\"upload.bin\""),
            h("Content-Type", "application/octet-stream"),
            HeadersEnd,
            d(BIN),
            PartEnd,
            PartStart,
            h("Content-Disposition", "form-data; name=\"notes\"; filename=\"n\u{f6}tes %22q%22.txt\""),
            h("Content-Type", "text/plain"),
            HeadersEnd,
            d(TXT),
            PartEnd,
            Finished,
        ],
    );
}

#[test]
fn node_formdata() {
    check(
        "node",
        vec![
            PartStart,
            h("Content-Disposition", "form-data; name=\"field\""),
            HeadersEnd,
            d(b"plain text"),
            PartEnd,
            PartStart,
            h("Content-Disposition", "form-data; name=\"empty\""),
            HeadersEnd,
            PartEnd,
            PartStart,
            h("Content-Disposition", "form-data; name=\"firmware\"; filename=\"upload.bin\""),
            h("Content-Type", "application/octet-stream"),
            HeadersEnd,
            d(BIN),
            PartEnd,
            PartStart,
            h("Content-Disposition", "form-data; name=\"notes\"; filename=\"n\u{f6}tes %22q%22.txt\""),
            h("Content-Type", "text/plain"),
            HeadersEnd,
            d(TXT),
            PartEnd,
            PartStart,
            h("Content-Disposition", "form-data; name=\"multi\""),
            HeadersEnd,
            d(b"a\r\nb"),
            PartEnd,
            Finished,
        ],
    );
}

#[test]
fn node_fields_only() {
    check(
        "node_fields",
        vec![
            PartStart,
            h("Content-Disposition", "form-data; name=\"a\""),
            HeadersEnd,
            d(b"1"),
            PartEnd,
            PartStart,
            h("Content-Disposition", "form-data; name=\"b\""),
            HeadersEnd,
            d(b"2"),
            PartEnd,
            Finished,
        ],
    );
}
