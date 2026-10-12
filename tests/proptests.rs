//! Property tests.
//! * `chunking_invariance_random`: random bodies built from delimiter-like tokens
//!   (valid and invalid) are parsed under random chunkings (incl. 1-byte) and must
//!   match the oracle, errors included.
//! * `differential_multer`: valid bodies must parse to the same headers and data
//!   as the `multer` crate.

mod common;
use bytes::Bytes;
use common::*;
use proptest::prelude::*;

const B: &[u8] = b"xY-9";

fn token_alphabet() -> Vec<Vec<u8>> {
    let d = [b"\r\n--", B].concat();
    vec![
        b"\r".to_vec(),
        b"\n".to_vec(),
        b"\r\n".to_vec(),
        b"-".to_vec(),
        b"--".to_vec(),
        b" ".to_vec(),
        b"\t".to_vec(),
        b":".to_vec(),
        b"x".to_vec(),
        b"\xc3\xa9".to_vec(),
        B.to_vec(),
        [b"--", B].concat(),
        [b"--", B, b"\r\n"].concat(),
        [b"--", B, b"--"].concat(),
        d.clone(),
        [&d[..], b"\r\n"].concat(),
        [&d[..], b"--"].concat(),
        [&d[..], b" \t\r\n"].concat(),
        [&d[..], b"         \r\n"].concat(),
        [&d[..], b"x"].concat(),
        [&d[..], b"-x"].concat(),
        d[..d.len() - 1].to_vec(),
        d[..d.len() - 2].to_vec(),
        b"A: v\r\n".to_vec(),
        b"Content-Disposition: form-data; name=\"a\"\r\n".to_vec(),
        b"\r\n\r\n".to_vec(),
        b"bad header\r\n".to_vec(),
        b" fold\r\n".to_vec(),
        b"A: 1234567890123456789012345678901234567890\r\n".to_vec(),
    ]
}

fn gen_body() -> impl Strategy<Value = Vec<u8>> {
    let alpha = token_alphabet();
    prop::collection::vec(0..alpha.len(), 0..40)
        .prop_map(move |idx| idx.iter().flat_map(|&i| alpha[i].clone()).collect())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, .. ProptestConfig::default() })]

    #[test]
    fn chunking_invariance_random(body in gen_body(), seed in any::<u64>()) {
        check_against_oracle::<32, 3>(B, &body, standard_chunkings(body.len(), 6, seed));
    }

    #[test]
    fn chunking_invariance_random_bytes(body in prop::collection::vec(prop_oneof![Just(b'\r'), Just(b'\n'), Just(b'-'), Just(b'x'), Just(b'Y'), Just(b'9'), Just(b' '), Just(b':')], 0..64), seed in any::<u64>()) {
        check_against_oracle::<32, 3>(B, &body, standard_chunkings(body.len(), 6, seed));
    }
}

// ----------------------------------------------------------------- differential

#[derive(Debug, Clone)]
struct GenPart {
    name: String,
    filename: Option<String>,
    content_type: Option<String>,
    extra: Vec<(String, String)>,
    data: Vec<u8>,
}

fn token() -> impl Strategy<Value = String> {
    "[A-Za-z0-9!#$%&'*+.^_`|~-]{1,12}"
}

fn gen_part() -> impl Strategy<Value = GenPart> {
    (
        "[A-Za-z0-9 _.%-]{0,16}",
        prop::option::of("[A-Za-z0-9 _.%-]{1,16}(\\.[a-z]{1,4})?"),
        prop::option::of(prop_oneof![
            Just("text/plain".to_string()),
            Just("application/octet-stream".to_string()),
            Just("image/png".to_string())
        ]),
        prop::collection::vec((token(), "[A-Za-z0-9 ,;=/.-]{0,20}"), 0..3),
        prop::collection::vec(any::<u8>(), 0..200),
    )
        .prop_map(|(name, filename, content_type, extra, data)| GenPart {
            name,
            filename,
            content_type,
            extra,
            data,
        })
        .prop_filter("extra header names unique and not content-*", |p| {
            // multer stores headers in a HeaderMap via insert(), dropping duplicates.
            let names: Vec<String> = p
                .extra
                .iter()
                .map(|(n, _)| n.to_ascii_lowercase())
                .collect();
            names
                .iter()
                .all(|n| n != "content-disposition" && n != "content-type")
                && names
                    .iter()
                    .enumerate()
                    .all(|(i, n)| !names[..i].contains(n))
        })
}

fn build_body(boundary: &[u8], parts: &[GenPart]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in parts {
        out.extend_from_slice(b"--");
        out.extend_from_slice(boundary);
        out.extend_from_slice(b"\r\nContent-Disposition: form-data; name=\"");
        out.extend_from_slice(p.name.as_bytes());
        out.push(b'"');
        if let Some(f) = &p.filename {
            out.extend_from_slice(b"; filename=\"");
            out.extend_from_slice(f.as_bytes());
            out.push(b'"');
        }
        out.extend_from_slice(b"\r\n");
        if let Some(ct) = &p.content_type {
            out.extend_from_slice(b"Content-Type: ");
            out.extend_from_slice(ct.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        for (n, v) in &p.extra {
            out.extend_from_slice(n.as_bytes());
            out.extend_from_slice(b": ");
            out.extend_from_slice(v.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&p.data);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"--");
    out.extend_from_slice(boundary);
    out.extend_from_slice(b"--\r\n");
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SeenPart {
    headers: Vec<(String, Vec<u8>)>, // lower-cased names
    data: Vec<u8>,
}

fn parse_ours(boundary: &[u8], body: &[u8], chunks: &[usize]) -> Vec<SeenPart> {
    let (events, err) = run_chunked::<512, 16>(boundary, body, chunks);
    assert_eq!(err, None);
    let mut parts: Vec<SeenPart> = Vec::new();
    for ev in events {
        match ev {
            OEvent::PartStart => parts.push(SeenPart {
                headers: vec![],
                data: vec![],
            }),
            OEvent::Header(n, v) => parts
                .last_mut()
                .unwrap()
                .headers
                .push((String::from_utf8(n.to_ascii_lowercase()).unwrap(), v)),
            OEvent::Data(d) => parts.last_mut().unwrap().data = d,
            _ => {}
        }
    }
    parts
}

fn parse_multer(boundary: &[u8], body: &[u8], chunks: &[usize]) -> Vec<SeenPart> {
    let mut pos = 0;
    let mut items: Vec<Result<Bytes, std::io::Error>> = Vec::new();
    for &n in chunks {
        items.push(Ok(Bytes::copy_from_slice(&body[pos..pos + n])));
        pos += n;
    }
    let stream = futures::stream::iter(items);
    let mut mp = multer::Multipart::new(stream, String::from_utf8(boundary.to_vec()).unwrap());
    futures::executor::block_on(async {
        let mut parts = Vec::new();
        while let Some(field) = mp.next_field().await.unwrap() {
            let headers = field
                .headers()
                .iter()
                .map(|(k, v)| (k.as_str().to_string(), v.as_bytes().to_vec()))
                .collect();
            let data = field.bytes().await.unwrap().to_vec();
            parts.push(SeenPart { headers, data });
        }
        parts
    })
}

fn delimiter_free(boundary: &[u8], data: &[u8]) -> bool {
    let d = [b"\r\n--", boundary].concat();
    !data.windows(d.len()).any(|w| w == d)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 600, .. ProptestConfig::default() })]

    #[test]
    fn differential_multer(parts in prop::collection::vec(gen_part(), 0..5), seed in any::<u64>()) {
        let boundary = b"----Bnd9x_Q";
        prop_assume!(parts.iter().all(|p| delimiter_free(boundary, &p.data)));
        let body = build_body(boundary, &parts);
        let chunkings = standard_chunkings(body.len(), 2, seed);
        let reference = parse_multer(boundary, &body, &chunkings[0]);
        prop_assert_eq!(reference.len(), parts.len());
        for ch in &chunkings {
            let ours = parse_ours(boundary, &body, ch);
            prop_assert_eq!(&ours, &reference);
            prop_assert_eq!(parse_multer(boundary, &body, ch), reference.clone());
        }
    }
}
