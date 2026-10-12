//! Real-client fixtures captured by `tests/fixtures/capture/capture.sh` from
//! curl -F, Python `requests` and Node's `FormData` (undici) through a local HTTP
//! server. Each fixture is parsed from its captured Content-Type and the exact
//! event stream is asserted. Bodies are also checked for chunking invariance
//! against the oracle.

mod common;
use common::*;
use nostd_multipart::{content_disposition, Parser};
use OEvent::*;

const BIN: &[u8] = b"hello\r\nworld\r\n--\r\n--not-a-boundary\r\n\x00\x01\xff binary";
const TXT: &[u8] = b"line1\nline2\n";

fn fixture(name: &str) -> (Vec<u8>, Vec<u8>) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");
    (
        std::fs::read(format!("{dir}{name}.ct")).unwrap(),
        std::fs::read(format!("{dir}{name}.body")).unwrap(),
    )
}

/// Decoded (name, filename) of every Content-Disposition header in the stream.
fn dispositions(events: &[OEvent]) -> Vec<(Vec<u8>, Option<Vec<u8>>)> {
    events
        .iter()
        .filter_map(|e| match e {
            Header(n, v) if n.eq_ignore_ascii_case(b"content-disposition") => {
                let cd = content_disposition(v).unwrap();
                assert!(cd.is_form_data());
                Some((
                    cd.name().unwrap().bytes().collect(),
                    cd.filename().map(|f| f.bytes().collect()),
                ))
            }
            _ => None,
        })
        .collect()
}

/// Events of one part with `Content-Disposition: form-data; {params}`.
fn part(params: &str, content_type: Option<&str>, data: &[u8]) -> Vec<OEvent> {
    let mut v = vec![
        PartStart,
        h("Content-Disposition", &format!("form-data; {params}")),
    ];
    v.extend(content_type.map(|ct| h("Content-Type", ct)));
    v.push(HeadersEnd);
    if !data.is_empty() {
        v.push(d(data));
    }
    v.push(PartEnd);
    v
}

fn field() -> Vec<OEvent> {
    part("name=\"field\"", None, b"plain text")
}
fn empty() -> Vec<OEvent> {
    part("name=\"empty\"", None, b"")
}
fn multi() -> Vec<OEvent> {
    part("name=\"multi\"", None, b"a\r\nb")
}
fn firmware() -> Vec<OEvent> {
    let ct = Some("application/octet-stream");
    part("name=\"firmware\"; filename=\"upload.bin\"", ct, BIN)
}
fn notes() -> Vec<OEvent> {
    part(
        "name=\"notes\"; filename=\"n\u{f6}tes %22q%22.txt\"",
        Some("text/plain"),
        TXT,
    )
}

/// Parses fixture `name` and asserts the events are `parts` then `Finished`.
fn check(name: &str, parts: &[Vec<OEvent>]) {
    let mut expected = parts.concat();
    expected.push(Finished);
    let (ct, body) = fixture(name);
    let p = Parser::<512, 16>::from_content_type(&ct).unwrap();
    let boundary = p.boundary().to_vec();
    let got =
        check_against_oracle::<512, 16>(&boundary, &body, standard_chunkings(body.len(), 64, 7));
    assert_eq!(got, (expected, None), "fixture {name}");
}

#[test]
fn decoded_dispositions() {
    let n = |s: &str| s.as_bytes().to_vec();
    let got = |f: &str| {
        let (ct, body) = fixture(f);
        let p = Parser::<512, 16>::from_content_type(&ct).unwrap();
        dispositions(&run_whole::<512, 16>(p.boundary(), &body).0)
    };
    assert_eq!(
        got("curl"),
        vec![
            (n("field"), None),
            (n("firmware"), Some(n("upload.bin"))),
            (n("odd"), Some(n("fïle \"q\".bin")))
        ]
    );
    assert_eq!(
        got("curl_edge"),
        vec![(n("empty"), None), (n("multi"), None), (n("ünïcode"), None)]
    );
    assert_eq!(
        got("requests"),
        vec![
            (n("field"), None),
            (n("empty"), None),
            (n("firmware"), Some(n("upload.bin"))),
            (n("notes"), Some(n("nötes \"q\".txt")))
        ]
    );
    assert_eq!(
        got("node"),
        vec![
            (n("field"), None),
            (n("empty"), None),
            (n("firmware"), Some(n("upload.bin"))),
            (n("notes"), Some(n("nötes \"q\".txt"))),
            (n("multi"), None)
        ]
    );
    assert_eq!(got("node_fields"), vec![(n("a"), None), (n("b"), None)]);
}

#[test]
fn curl() {
    let (_, body) = fixture("curl");
    assert!(body.starts_with(b"--------------------------ksWXkr5pRWPWD14b4N9oTM\r\n"));
    let odd = "name=\"odd\"; filename=\"f\u{ef}le %22q%22.bin\"";
    check(
        "curl",
        &[
            field(),
            firmware(),
            part(odd, Some("application/octet-stream"), BIN),
        ],
    );
}

#[test]
fn curl_edge() {
    let unicode = part("name=\"\u{fc}n\u{ef}code\"", None, "ü".as_bytes());
    check("curl_edge", &[empty(), multi(), unicode]);
}

#[test]
fn python_requests() {
    check("requests", &[field(), empty(), firmware(), notes()]);
}

#[test]
fn node_formdata() {
    check("node", &[field(), empty(), firmware(), notes(), multi()]);
}

#[test]
fn node_fields_only() {
    let (a, b) = (
        part("name=\"a\"", None, b"1"),
        part("name=\"b\"", None, b"2"),
    );
    check("node_fields", &[a, b]);
}
