//! Encoder (DESIGN.md §11): exact output, resumability with tiny buffers,
//! `content_length`, `boundary_from_random`, and the encoder → parser round-trip
//! property test over adversarial names, filenames and data.

mod common;
use common::*;
use nostd_multipart::{boundary_from_random, content_disposition, Encoder, Error, Event, Parser, Part};
use proptest::prelude::*;

/// Encodes a whole body using `out` as the (possibly tiny) staging buffer.
fn encode(boundary: &[u8], parts: &[(Part<'_>, &[u8])], out_len: usize) -> Vec<u8> {
    let mut enc = Encoder::new(boundary).unwrap();
    let mut body = Vec::new();
    let mut out = vec![0u8; out_len];
    for (part, data) in parts {
        loop {
            let (n, done) = enc.write_part_header(part, &mut out).unwrap();
            body.extend_from_slice(&out[..n]);
            if done {
                break;
            }
            assert!(n > 0 || out_len == 0, "no progress with non-empty buffer");
        }
        body.extend_from_slice(data);
    }
    loop {
        let (n, done) = enc.write_close(&mut out);
        body.extend_from_slice(&out[..n]);
        if done {
            break;
        }
    }
    body
}

#[test]
fn exact_output() {
    let parts = [
        (Part { name: b"field", filename: None, content_type: None }, &b"plain text"[..]),
        (
            Part { name: b"firmware", filename: Some(b"fw.bin"), content_type: Some(b"application/octet-stream") },
            &b"\x00\x01\xff"[..],
        ),
        (Part { name: b"q\"\r\n\\", filename: Some(b"\"a\"\r\n.txt"), content_type: None }, &b""[..]),
    ];
    let expected = b"--xyz\r\nContent-Disposition: form-data; name=\"field\"\r\n\r\nplain text\r\n--xyz\r\nContent-Disposition: form-data; name=\"firmware\"; filename=\"fw.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n\x00\x01\xff\r\n--xyz\r\nContent-Disposition: form-data; name=\"q%22%0D%0A\\\\\"; filename=\"%22a%22%0D%0A.txt\"\r\n\r\n\r\n--xyz--\r\n";
    for out_len in [1usize, 2, 3, 7, 64, 4096] {
        assert_eq!(encode(b"xyz", &parts, out_len), &expected[..], "out_len {out_len}");
    }
    let enc = Encoder::new(b"xyz").unwrap();
    let lens: Vec<(Part<'_>, u64)> = parts.iter().map(|(p, d)| (*p, d.len() as u64)).collect();
    assert_eq!(enc.content_length(&lens), expected.len() as u64);
    assert_eq!(enc.content_length(&[]), b"\r\n--xyz--\r\n".len() as u64);
    assert_eq!(enc.boundary(), b"xyz");
    assert!(format!("{enc:?}").contains("Encoder"));
    let _ = enc.clone();
}

#[test]
fn zero_parts() {
    assert_eq!(encode(b"b", &[], 8), b"\r\n--b--\r\n");
    let (ev, err) = run_whole::<64, 4>(b"b", &encode(b"b", &[], 8));
    assert_eq!((ev, err), (vec![OEvent::Finished], None));
}

#[test]
fn zero_length_buffer_makes_no_progress_but_no_error() {
    let mut enc = Encoder::new(b"b").unwrap();
    let mut out = [0u8; 0];
    assert_eq!(enc.write_part_header(&Part { name: b"n", ..Part::default() }, &mut out), Ok((0, false)));
    let mut out = [0u8; 100];
    let (n, done) = enc.write_part_header(&Part { name: b"n", ..Part::default() }, &mut out).unwrap();
    assert!(done);
    assert_eq!(&out[..n], b"--b\r\nContent-Disposition: form-data; name=\"n\"\r\n\r\n");
}

#[test]
fn errors() {
    assert_eq!(Encoder::new(b"").err(), Some(Error::Boundary));
    assert_eq!(Encoder::new(b"a ").err(), Some(Error::Boundary));
    let mut enc = Encoder::new(b"b").unwrap();
    let mut out = [0u8; 100];
    assert_eq!(
        enc.write_part_header(&Part { name: b"n", filename: None, content_type: Some(b"text/plain\r\nX: y") }, &mut out),
        Err(Error::Header)
    );
    assert_eq!(enc.write_part_header(&Part { name: b"n", filename: None, content_type: Some(b"a\nb") }, &mut out), Err(Error::Header));
    // The encoder is still usable afterwards.
    assert!(enc.write_part_header(&Part { name: b"n", ..Part::default() }, &mut out).unwrap().1);
}

#[test]
fn boundary_from_random_maps_to_bchars() {
    let r: [u8; 32] = core::array::from_fn(|i| (i as u8).wrapping_mul(37).wrapping_add(11));
    let b = boundary_from_random(&r).unwrap();
    assert!(Parser::<64, 4>::new(&b).is_ok());
    assert!(b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-' || *c == b'_'));
    let all: Vec<u8> = (0u8..=255).collect();
    let all: [u8; 70] = all[..70].try_into().unwrap();
    assert!(Parser::<64, 4>::new(&boundary_from_random(&all).unwrap()).is_ok());
    let hi: [u8; 70] = core::array::from_fn(|i| 255 - i as u8);
    assert!(Parser::<64, 4>::new(&boundary_from_random(&hi).unwrap()).is_ok());
    assert_eq!(boundary_from_random(&[0u8; 0]).err(), Some(Error::Boundary));
    assert_eq!(boundary_from_random(&[0u8; 71]).err(), Some(Error::Boundary));
    assert_eq!(boundary_from_random(&[0u8; 1]).unwrap(), *b"A");
    assert_eq!(boundary_from_random(&[63u8; 2]).unwrap(), *b"__");
    assert_eq!(boundary_from_random(&[62u8 + 64; 1]).unwrap(), *b"-");
}

#[test]
fn whatwg_escaping_is_lossy_by_design() {
    // WHATWG does not escape '%', so a literal "%22" in a name decodes as '"'.
    // Browsers have the same ambiguity; documented in DESIGN.md rule 8.
    let body = encode(b"b", &[(Part { name: b"100%22", ..Part::default() }, b"")], 64);
    let (ev, _) = run_whole::<64, 4>(b"b", &body);
    let OEvent::Header(_, v) = &ev[1] else { panic!() };
    assert_eq!(v, b"form-data; name=\"100%22\"");
    let cd = content_disposition(v).unwrap();
    assert_eq!(cd.name().unwrap().raw(), b"100%22");
    assert_eq!(cd.name().unwrap().bytes().collect::<Vec<u8>>(), b"100\"");
}

// --------------------------------------------------------------- round-trip

#[derive(Debug, Clone)]
struct RtPart {
    name: Vec<u8>,
    filename: Option<Vec<u8>>,
    content_type: Option<Vec<u8>>,
    data: Vec<u8>,
}

const RT_BOUNDARY: &[u8] = b"rt-Bn_d";

fn text() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(
        prop_oneof![
            8 => any::<char>().prop_filter("no CR/LF/quote here", |c| !matches!(c, '\r' | '\n' | '"')).prop_map(|c| c.to_string().into_bytes()),
            1 => Just(b"\"".to_vec()),
            1 => Just(b"\r".to_vec()),
            1 => Just(b"\n".to_vec()),
            1 => Just(b"\r\n".to_vec()),
            1 => Just(b"%".to_vec()),
            1 => Just(b"\\".to_vec()),
            1 => Just(b";".to_vec()),
        ],
        0..12,
    )
    .prop_map(|v| v.concat())
}

fn data() -> impl Strategy<Value = Vec<u8>> {
    let d = [b"\r\n--", RT_BOUNDARY].concat();
    let mut toks: Vec<Vec<u8>> = vec![
        b"\r".to_vec(),
        b"\n".to_vec(),
        b"\r\n".to_vec(),
        b"-".to_vec(),
        b"--".to_vec(),
        RT_BOUNDARY.to_vec(),
        [&d[..], b"x"].concat(),
        [&d[..], b"-x"].concat(),
        [&d[..], b" x"].concat(),
        [&d[..], b"\rx"].concat(),
        b"\x00".to_vec(),
        b"\xff".to_vec(),
        b"data".to_vec(),
    ];
    for i in 1..d.len() {
        toks.push(d[..i].to_vec());
    }
    let n = toks.len();
    prop::collection::vec(0..n, 0..16).prop_map(move |idx| idx.iter().flat_map(|&i| toks[i].clone()).collect())
}

fn rt_part() -> impl Strategy<Value = RtPart> {
    (
        text(),
        prop::option::of(text()),
        prop::option::of(prop_oneof![Just(b"text/plain".to_vec()), Just(b"application/octet-stream".to_vec()), Just(b"x/y; q=\"a\"".to_vec())]),
        data(),
    )
        .prop_map(|(name, filename, content_type, data)| RtPart { name, filename, content_type, data: make_safe(&data) })
}

/// RFC 2046 obligation on the caller: the data must not contain the delimiter
/// `D` followed by `--`, padding or CRLF, and must not end in `D` (the encoder's
/// CRLF would complete it). `D` followed by any other byte is allowed (rule 3).
fn data_is_safe(data: &[u8]) -> bool {
    let d = [b"\r\n--", RT_BOUNDARY].concat();
    data.windows(d.len()).enumerate().all(|(i, w)| w != d || !matches!(data.get(i + d.len()), None | Some(b'-' | b' ' | b'\t' | b'\r')))
}

/// Repairs generated data by inserting an `x` after every unsafe `D` occurrence,
/// so the generator keeps every adversarial pattern and nothing is rejected.
fn make_safe(data: &[u8]) -> Vec<u8> {
    let d = [b"\r\n--", RT_BOUNDARY].concat();
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
    assert!(data_is_safe(&out));
    out
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1500, .. ProptestConfig::default() })]

    #[test]
    fn round_trip(parts in prop::collection::vec(rt_part(), 0..4), out_len in 1usize..40, seed in any::<u64>()) {
        // WHATWG escaping is lossy: a literal "%22"/"%0D"/"%0A" cannot round-trip
        // (see `whatwg_escaping_is_lossy_by_design`).
        fn plain(v: &[u8]) -> bool {
            !v.windows(3).any(|w| matches!(w, b"%22" | b"%0D" | b"%0A" | b"%0d" | b"%0a"))
        }
        prop_assume!(parts.iter().all(|p| plain(&p.name) && p.filename.as_deref().map_or(true, plain)));
        let borrowed: Vec<(Part<'_>, &[u8])> = parts
            .iter()
            .map(|p| (Part { name: &p.name, filename: p.filename.as_deref(), content_type: p.content_type.as_deref() }, &p.data[..]))
            .collect();
        let body = encode(RT_BOUNDARY, &borrowed, out_len);
        let enc = Encoder::new(RT_BOUNDARY).unwrap();
        let lens: Vec<(Part<'_>, u64)> = borrowed.iter().map(|(p, d)| (*p, d.len() as u64)).collect();
        prop_assert_eq!(enc.content_length(&lens), body.len() as u64);

        for chunks in standard_chunkings(body.len(), 3, seed) {
            let mut parser = Parser::<512, 16>::new(RT_BOUNDARY).unwrap();
            let mut got: Vec<RtPart> = Vec::new();
            let mut buf = Vec::new();
            let mut pos = 0;
            let mut ci = 0;
            loop {
                let eof = ci >= chunks.len();
                let (used, ev) = parser.parse(&buf, eof).unwrap();
                let had = ev.is_some();
                let mut done = false;
                match ev {
                    Some(Event::PartStart) => got.push(RtPart { name: vec![], filename: None, content_type: None, data: vec![] }),
                    Some(Event::Header { name, value }) => {
                        let cur = got.last_mut().unwrap();
                        if name.eq_ignore_ascii_case(b"content-disposition") {
                            let cd = content_disposition(value).unwrap();
                            prop_assert!(cd.is_form_data());
                            cur.name = cd.name().unwrap().bytes().collect();
                            cur.filename = cd.filename().map(|f| f.bytes().collect());
                        } else if name.eq_ignore_ascii_case(b"content-type") {
                            cur.content_type = Some(value.to_vec());
                        } else {
                            prop_assert!(false, "unexpected header {:?}", name);
                        }
                    }
                    Some(Event::Data(d)) => got.last_mut().unwrap().data.extend_from_slice(d),
                    Some(Event::Finished) => done = true,
                    Some(Event::HeadersEnd | Event::PartEnd) | None => {}
                }
                buf.drain(..used);
                if done { break; }
                if used == 0 && !had {
                    prop_assert!(!eof);
                    let n = chunks[ci];
                    buf.extend_from_slice(&body[pos..pos + n]);
                    pos += n;
                    ci += 1;
                }
            }
            prop_assert_eq!(got.len(), parts.len());
            for (g, p) in got.iter().zip(&parts) {
                prop_assert_eq!(&g.name, &p.name);
                prop_assert_eq!(&g.filename, &p.filename);
                prop_assert_eq!(&g.content_type, &p.content_type);
                prop_assert_eq!(&g.data, &p.data);
            }
        }
    }
}
