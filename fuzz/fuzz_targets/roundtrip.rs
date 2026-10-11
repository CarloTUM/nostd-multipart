//! Fuzz: encoder -> parser round-trip with arbitrary parts, staging-buffer size
//! and parse chunking. Names/filenames may contain anything; data is repaired to
//! respect the RFC 2046 caller obligation (no delimiter followed by `--`, padding
//! or CRLF). `content_length` must match the produced body.
#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use nostd_multipart::{content_disposition, Encoder, Event, Parser, Part};

#[derive(Arbitrary, Debug)]
struct FPart<'a> {
    name: &'a [u8],
    filename: Option<&'a [u8]>,
    content_type: Option<&'a [u8]>,
    data: &'a [u8],
}

#[derive(Arbitrary, Debug)]
struct Input<'a> {
    boundary: &'a [u8],
    out_len: u8,
    chunk: u8,
    parts: Vec<FPart<'a>>,
}

fn lossy(v: &[u8]) -> bool {
    v.windows(3)
        .any(|w| matches!(w, b"%22" | b"%0D" | b"%0A" | b"%0d" | b"%0a"))
}

fn make_safe(boundary: &[u8], data: &[u8]) -> Vec<u8> {
    let d = [b"\r\n--", boundary].concat();
    let mut out = Vec::with_capacity(data.len() + 8);
    let mut i = 0;
    while i < data.len() {
        if data[i..].starts_with(&d) {
            out.extend_from_slice(&d);
            i += d.len();
            if matches!(data.get(i), None | Some(b'-' | b' ' | b'\t' | b'\r')) {
                out.push(b'x');
            }
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    out
}

fuzz_target!(|input: Input<'_>| {
    let Ok(mut enc) = Encoder::new(input.boundary) else {
        return;
    };
    if input.parts.len() > 8 {
        return;
    }
    if input
        .parts
        .iter()
        .any(|p| lossy(p.name) || p.filename.is_some_and(lossy))
    {
        return;
    }
    // Keep every header line within the parser's LINE = 2048 (escaping triples
    // bytes at most: 38 + 300 + 14 + 300 + 3 < 2048): longer names are
    // legitimately `HeaderTooLong`.
    if input.parts.iter().any(|p| {
        p.name.len() > 100
            || p.filename.map_or(0, <[u8]>::len) > 100
            || p.content_type.map_or(0, <[u8]>::len) > 100
    }) {
        return;
    }
    let out_len = (input.out_len as usize).max(1);
    let mut out = vec![0u8; out_len];
    let mut body = Vec::new();
    let mut lens = Vec::new();
    let mut kept: Vec<(Part<'_>, Vec<u8>)> = Vec::new();
    for p in &input.parts {
        let part = Part {
            name: p.name,
            filename: p.filename,
            content_type: p.content_type,
        };
        let data = make_safe(input.boundary, p.data);
        loop {
            match enc.write_part_header(&part, &mut out) {
                Ok((n, done)) => {
                    body.extend_from_slice(&out[..n]);
                    if done {
                        break;
                    }
                    assert!(n > 0);
                }
                Err(e) => {
                    assert_eq!(e, nostd_multipart::Error::Header);
                    assert!(p
                        .content_type
                        .unwrap()
                        .iter()
                        .any(|&b| b == b'\r' || b == b'\n'));
                    return;
                }
            }
        }
        body.extend_from_slice(&data);
        lens.push((part, data.len() as u64));
        kept.push((part, data));
    }
    loop {
        let (n, done) = enc.write_close(&mut out);
        body.extend_from_slice(&out[..n]);
        if done {
            break;
        }
    }
    assert_eq!(
        Encoder::new(input.boundary).unwrap().content_length(&lens),
        body.len() as u64
    );

    // Parse back in chunks of `chunk` bytes (0 => whole body).
    let chunk = if input.chunk == 0 {
        body.len().max(1)
    } else {
        input.chunk as usize
    };
    let mut parser = Parser::<2048, 16>::new(input.boundary).unwrap();
    let mut got: Vec<(Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>, Vec<u8>)> = Vec::new();
    let mut buf = Vec::new();
    let mut pos = 0;
    loop {
        let eof = pos >= body.len();
        let (used, ev) = parser.parse(&buf, eof).unwrap();
        let had = ev.is_some();
        let mut done = false;
        match ev {
            Some(Event::PartStart) => got.push((vec![], None, None, vec![])),
            Some(Event::Header { name, value }) => {
                let cur = got.last_mut().unwrap();
                if name.eq_ignore_ascii_case(b"content-disposition") {
                    let cd = content_disposition(value).unwrap();
                    cur.0 = cd.name().unwrap().bytes().collect();
                    cur.1 = cd.filename().map(|f| f.bytes().collect());
                } else {
                    assert!(name.eq_ignore_ascii_case(b"content-type"));
                    cur.2 = Some(value.to_vec());
                }
            }
            Some(Event::Data(d)) => got.last_mut().unwrap().3.extend_from_slice(d),
            Some(Event::Finished) => done = true,
            _ => {}
        }
        buf.drain(..used);
        if done {
            break;
        }
        if used == 0 && !had {
            assert!(!eof);
            let n = chunk.min(body.len() - pos);
            buf.extend_from_slice(&body[pos..pos + n]);
            pos += n;
        }
    }
    assert_eq!(got.len(), kept.len());
    for (g, (p, data)) in got.iter().zip(&kept) {
        assert_eq!(g.0, p.name);
        assert_eq!(g.1.as_deref(), p.filename);
        // Content-Type values are trimmed of OWS by the parser.
        assert_eq!(g.2.as_deref(), p.content_type.map(|ct| trim(ct)));
        assert_eq!(&g.3, data);
    }
});

fn trim(mut s: &[u8]) -> &[u8] {
    while let [b' ' | b'\t', rest @ ..] = s {
        s = rest;
    }
    while let [rest @ .., b' ' | b'\t'] = s {
        s = rest;
    }
    s
}
