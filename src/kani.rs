//! Kani proof harnesses (`cargo kani --all-features`). Compiled only under `cfg(kani)`.
//!
//! Each harness fixes the input length and the boundary and leaves every input
//! byte (and `eof`) symbolic: CBMC does not finish on this code with a symbolic
//! slice length, but it proves concrete-length harnesses in minutes. Properties:
//! no panic (Kani checks every arithmetic, index, unwrap and unreachable), and
//! `consumed <= input.len()`, `Data` non-empty and within `consumed`, progress
//! with `eof`.
//!
//! * default set (`cargo kani --all-features`, fits a 16 GB machine):
//!   `parse_len4`, `parse_len6`, `parse_two_calls_len4`, `headers_len4`,
//!   `cd_len4`, `boundary_from_random_valid`.
//! * `RUSTFLAGS="--cfg kani_full"` adds the 8/10/12-byte rungs, the 6/8-byte
//!   header and Content-Disposition rungs and the encoder harness; these need
//!   tens of GB and hours (see README for what has been completed).

use crate::{content_disposition, Event, Parser};

fn check_call(p: &mut Parser<'_, 8, 2>, input: &[u8], eof: bool) {
    // (Error stickiness is checked by the test harness on every error path; a
    // second symbolic `parse` call here multiplies the CBMC formula size.)
    let min_buffer = p.min_buffer();
    if let Ok((consumed, ev)) = p.parse(input, eof) {
        assert!(consumed <= input.len());
        if eof || input.len() >= min_buffer {
            assert!(consumed > 0 || ev.is_some(), "progress guarantee");
        }
        if let Some(Event::Data(d)) = ev {
            assert!(!d.is_empty());
            assert!(d.len() <= consumed);
        }
    }
}

macro_rules! parse_harness {
    ($name:ident, $len:literal, $unwind:literal $(, $cfg:meta)?) => {
        $(#[cfg($cfg)])?
        #[kani::proof]
        #[kani::unwind($unwind)]
        fn $name() {
            let input: [u8; $len] = kani::any();
            let eof: bool = kani::any();
            let mut p = Parser::<8, 2>::new(b"ab").unwrap();
            check_call(&mut p, &input, eof);
        }
    };
}
parse_harness!(parse_len4, 4, 8);
parse_harness!(parse_len6, 6, 10);
parse_harness!(parse_len8, 8, 12, kani_full);
parse_harness!(parse_len10, 10, 14, kani_full);
parse_harness!(parse_len12, 12, 16, kani_full);

/// Two calls: any split of 4 symbolic bytes, then EOF.
#[kani::proof]
#[kani::unwind(8)]
fn parse_two_calls_len4() {
    let input: [u8; 4] = kani::any();
    let split: usize = kani::any();
    kani::assume(split <= 4);
    let eof1: bool = kani::any();
    let (first, second) = input.split_at(split);
    let mut p = Parser::<8, 2>::new(b"ab").unwrap();
    check_call(&mut p, first, eof1);
    check_call(&mut p, second, true);
}

macro_rules! headers_harness {
    ($name:ident, $len:literal, $unwind:literal $(, $cfg:meta)?) => {
        $(#[cfg($cfg)])?
        #[kani::proof]
        #[kani::unwind($unwind)]
        fn $name() {
            // Reach the Headers state deterministically, then arbitrary bytes.
            let input: [u8; $len] = kani::any();
            let eof: bool = kani::any();
            let mut p = Parser::<8, 2>::new(b"ab").unwrap();
            assert_eq!(p.parse(b"--ab\r\n", false), Ok((6, Some(Event::PartStart))));
            check_call(&mut p, &input, eof);
        }
    };
}
headers_harness!(headers_len4, 4, 8);
headers_harness!(headers_len6, 6, 10, kani_full);
headers_harness!(headers_len8, 8, 12, kani_full);

macro_rules! cd_harness {
    ($name:ident, $len:literal, $unwind:literal $(, $cfg:meta)?) => {
        $(#[cfg($cfg)])?
        #[kani::proof]
        #[kani::unwind($unwind)]
        fn $name() {
            let input: [u8; $len] = kani::any();
            if let Ok(cd) = content_disposition(&input) {
                let mut out = [0u8; $len];
                if let Some(n) = cd.name() {
                    assert!(n.decode_into(&mut out).is_some());
                    assert!(n.decoded_len() <= n.raw().len());
                }
                if let Some(f) = cd.filename() {
                    assert!(f.decoded_len() <= f.raw().len());
                }
            }
        }
    };
}
cd_harness!(cd_len4, 4, 8);
cd_harness!(cd_len6, 6, 10, kani_full);
cd_harness!(cd_len8, 8, 12, kani_full);

#[cfg(kani_full)]
#[kani::proof]
#[kani::unwind(60)]
fn encoder_no_panic() {
    use crate::{Encoder, Part};
    let nbuf: [u8; 1] = kani::any();
    let out_len: usize = kani::any();
    kani::assume(out_len >= 1 && out_len <= 4);
    let mut enc = Encoder::new(b"b").unwrap();
    let part = Part {
        name: &nbuf,
        filename: None,
        content_type: None,
    };
    let mut out = [0u8; 4];
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
