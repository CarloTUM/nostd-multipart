//! Kani proof harnesses (`cargo kani`). Compiled only under `cfg(kani)`.
//!
//! Proven for every boundary of 1–4 bytes and every input of 0–16 bytes:
//! * `parse` never panics (Kani checks all arithmetic, indexing, unwrap, ...);
//! * `consumed <= input.len()`;
//! * with `eof == true` (or `len >= min_buffer()`) every call makes progress;
//! * a second call after an arbitrary first call keeps the same properties;
//! * `content_disposition`, `from_content_type`, the encoder and
//!   `boundary_from_random` never panic.

use crate::{content_disposition, Encoder, Event, Parser, Part};

fn any_slice<const N: usize>(buf: &[u8; N]) -> &[u8] {
    let len: usize = kani::any();
    kani::assume(len <= N);
    &buf[..len]
}

fn any_boundary(buf: &[u8; 4]) -> &[u8] {
    let len: usize = kani::any();
    kani::assume(len >= 1 && len <= 4);
    &buf[..len]
}

fn check_call(p: &mut Parser<'_, 8, 2>, input: &[u8], eof: bool) {
    let min_buffer = p.min_buffer();
    let failed = match p.parse(input, eof) {
        Ok((consumed, ev)) => {
            assert!(consumed <= input.len());
            if eof || input.len() >= min_buffer {
                assert!(consumed > 0 || ev.is_some(), "progress guarantee");
            }
            if let Some(Event::Data(d)) = ev {
                assert!(!d.is_empty());
                assert!(d.len() <= consumed);
            }
            None
        }
        Err(e) => Some(e),
    };
    if let Some(e) = failed {
        // Sticky.
        assert_eq!(p.parse(input, eof), Err(e));
    }
}

#[kani::proof]
#[kani::unwind(20)]
fn parse_one_call() {
    let bbuf: [u8; 4] = kani::any();
    let ibuf: [u8; 16] = kani::any();
    let eof: bool = kani::any();
    let Ok(mut p) = Parser::<8, 2>::new(any_boundary(&bbuf)) else {
        return;
    };
    check_call(&mut p, any_slice(&ibuf), eof);
}

#[kani::proof]
#[kani::unwind(20)]
fn parse_two_calls() {
    let bbuf: [u8; 4] = kani::any();
    let ibuf: [u8; 16] = kani::any();
    let Ok(mut p) = Parser::<8, 2>::new(any_boundary(&bbuf)) else {
        return;
    };
    let input = any_slice(&ibuf);
    let split: usize = kani::any();
    kani::assume(split <= input.len());
    let eof1: bool = kani::any();
    let (first, second) = input.split_at(split);
    check_call(&mut p, first, eof1);
    check_call(&mut p, second, true);
}

#[kani::proof]
#[kani::unwind(20)]
fn parse_headers_and_body_states() {
    // Reach the Headers state deterministically, then throw arbitrary bytes at it.
    let ibuf: [u8; 16] = kani::any();
    let mut p = Parser::<8, 2>::new(b"ab").unwrap();
    assert_eq!(p.parse(b"--ab\r\n", false), Ok((6, Some(Event::PartStart))));
    let eof: bool = kani::any();
    check_call(&mut p, any_slice(&ibuf), eof);
    check_call(&mut p, b"", true);
}

#[kani::proof]
#[kani::unwind(20)]
fn content_disposition_no_panic() {
    let buf: [u8; 16] = kani::any();
    if let Ok(cd) = content_disposition(any_slice(&buf)) {
        let mut out = [0u8; 16];
        if let Some(n) = cd.name() {
            assert!(n.decode_into(&mut out).is_some());
            assert!(n.decoded_len() <= n.raw().len());
        }
        if let Some(f) = cd.filename() {
            assert!(f.decoded_len() <= f.raw().len());
        }
    }
}

#[kani::proof]
#[kani::unwind(20)]
fn from_content_type_no_panic() {
    let buf: [u8; 16] = kani::any();
    if let Ok(p) = Parser::<8, 2>::from_content_type(any_slice(&buf)) {
        assert!(!p.boundary().is_empty());
    }
}

#[kani::proof]
#[kani::unwind(12)]
fn encoder_no_panic() {
    // Concrete boundary, symbolic 0–2 byte name (every byte class: plain, `"`,
    // CR, LF, `\`), symbolic staging-buffer size 1–8: every resumption path.
    let nbuf: [u8; 2] = kani::any();
    let out_len: usize = kani::any();
    kani::assume(out_len >= 1 && out_len <= 8);
    let mut enc = Encoder::new(b"b").unwrap();
    let part = Part {
        name: any_slice(&nbuf),
        filename: None,
        content_type: None,
    };
    let mut out = [0u8; 8];
    let mut total = 0u64;
    loop {
        let (n, done) = enc.write_part_header(&part, &mut out[..out_len]).unwrap();
        assert!(n <= out_len);
        total += n as u64;
        if done {
            break;
        }
        assert!(n > 0);
    }
    assert!(enc.content_length(&[(part, 0)]) >= total);
    let (n, _done) = enc.write_close(&mut out[..out_len]);
    assert!(n <= out_len);
}

#[kani::proof]
fn boundary_from_random_valid() {
    let r: [u8; 4] = kani::any();
    let b = crate::boundary_from_random(&r).unwrap();
    assert!(Parser::<8, 2>::new(&b).is_ok());
}
