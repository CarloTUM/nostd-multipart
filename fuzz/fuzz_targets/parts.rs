//! Fuzz: the part-level adapter API (`next_part`, `next_chunk`, `read`,
//! `read_body`) with arbitrary read sizes, buffer sizes and an arbitrary mix of
//! the three body-reading methods yields, per part, the first
//! `Content-Disposition` value and exactly the body that the oracle sees
//! (DESIGN.md §14: no byte delivered twice, none lost), and the same error.
#![no_main]

#[path = "../../tests/common/mod.rs"]
#[allow(dead_code)]
mod common;

use arbitrary::Arbitrary;
use common::{oerr, oracle, OError, OEvent};
use embedded_io::Read;
use libfuzzer_sys::fuzz_target;
use nostd_multipart::{MultipartReader, Parser, ReadError};

#[derive(Arbitrary, Debug)]
struct Input<'a> {
    boundary: &'a [u8],
    reads: Vec<u8>,
    ops: Vec<u8>,
    buf_sel: u8,
    body: &'a [u8],
}

struct R<'a> {
    data: &'a [u8],
    reads: std::vec::IntoIter<u8>,
}
impl embedded_io::ErrorType for R<'_> {
    type Error = core::convert::Infallible;
}
impl embedded_io::Read for R<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        let want = (self.reads.next().unwrap_or(7) as usize).max(1);
        let n = want.min(buf.len()).min(self.data.len());
        buf[..n].copy_from_slice(&self.data[..n]);
        self.data = &self.data[n..];
        Ok(n)
    }
}

/// One part as the oracle saw it.
#[derive(Debug, Default, PartialEq, Eq)]
struct Expected {
    disposition: Option<Vec<u8>>,
    body: Vec<u8>,
    headers_done: bool,
    complete: bool,
}

fn expected_parts(events: &[OEvent]) -> (Vec<Expected>, bool) {
    let mut parts: Vec<Expected> = Vec::new();
    let mut finished = false;
    for ev in events {
        match ev {
            OEvent::PartStart => parts.push(Expected::default()),
            OEvent::Header(name, value) => {
                let p = parts.last_mut().unwrap();
                if name.eq_ignore_ascii_case(b"content-disposition") && p.disposition.is_none() {
                    p.disposition = Some(value.clone());
                }
            }
            OEvent::HeadersEnd => parts.last_mut().unwrap().headers_done = true,
            OEvent::Data(d) => parts.last_mut().unwrap().body.extend_from_slice(d),
            OEvent::PartEnd => parts.last_mut().unwrap().complete = true,
            OEvent::Finished => finished = true,
        }
    }
    (parts, finished)
}

fn run<const BUF: usize>(
    input: &Input<'_>,
    expected: &[Expected],
    finished: bool,
    oracle_err: Option<OError>,
) {
    let parser = Parser::<32, 3>::new(input.boundary).unwrap();
    let min = parser.min_buffer();
    let mut r = MultipartReader::<_, BUF, 32, 3>::new(
        R {
            data: input.body,
            reads: input.reads.clone().into_iter(),
        },
        parser,
    );
    let mut ops = input.ops.iter().copied().cycle();
    let mut idx = 0usize;
    let check_err = |e: ReadError<core::convert::Infallible>| match e {
        ReadError::BufferTooSmall => {
            assert!(BUF < min, "BufferTooSmall with BUF {BUF} >= min {min}")
        }
        ReadError::Parse(e) => assert_eq!(Some(oerr(e)), oracle_err),
        ReadError::Io(never) => match never {},
    };
    loop {
        let info = match r.next_part() {
            Ok(Some(info)) => info,
            Ok(None) => {
                assert!(finished, "adapter finished, oracle did not");
                assert_eq!(idx, expected.len(), "adapter saw fewer parts");
                return;
            }
            Err(e) => return check_err(e),
        };
        let exp = &expected[idx];
        assert!(
            exp.headers_done,
            "part {idx}: oracle never finished these headers"
        );
        assert_eq!(
            info.disposition_raw().map(<[u8]>::to_vec),
            exp.disposition,
            "part {idx}"
        );
        let mut got: Vec<u8> = Vec::new();
        let mut scratch = [0u8; 40];
        // Empty buffers read nothing and change nothing.
        assert_eq!(r.read(&mut []), Ok(0));
        let outcome: Result<(), ReadError<_>> = loop {
            let op = ops.next().unwrap_or(0);
            // 1..=40 bytes: a zero-sized buffer could never finish the body.
            let size = ((op as usize >> 2) % scratch.len()) + 1;
            match op & 3 {
                0 => match r.next_chunk() {
                    Ok(Some(c)) => {
                        assert!(!c.is_empty());
                        got.extend_from_slice(c);
                    }
                    Ok(None) => break Ok(()),
                    Err(e) => break Err(e),
                },
                1 => match r.read(&mut scratch[..size]) {
                    Ok(0) => break Ok(()),
                    Ok(n) => got.extend_from_slice(&scratch[..n]),
                    Err(e) => break Err(e),
                },
                2 => match r.read_body(&mut scratch[..size]) {
                    Ok(Some(n)) => {
                        got.extend_from_slice(&scratch[..n]);
                        break Ok(());
                    }
                    Ok(None) => {
                        got.extend_from_slice(&scratch[..size]);
                        assert!(
                            got.len() < exp.body.len() || !exp.complete,
                            "part {idx}: None but body fit"
                        );
                    }
                    Err(e) => break Err(e),
                },
                _ => {
                    // Every reading method ends the body with its own "done" signal;
                    // check they agree once one has fired.
                    match r.next_chunk() {
                        Ok(Some(c)) => got.extend_from_slice(c),
                        Ok(None) => {
                            assert_eq!(r.read(&mut scratch), Ok(0));
                            assert_eq!(r.read_body(&mut scratch), Ok(Some(0)));
                            break Ok(());
                        }
                        Err(e) => break Err(e),
                    }
                }
            }
            assert!(exp.body.starts_with(&got), "part {idx}: body diverged");
        };
        match outcome {
            Ok(()) => {
                assert!(
                    exp.complete,
                    "part {idx}: adapter ended a body the oracle did not"
                );
                assert_eq!(got, exp.body, "part {idx}");
            }
            Err(e) => {
                assert!(
                    exp.body.starts_with(&got),
                    "part {idx}: body diverged before error"
                );
                return check_err(e);
            }
        }
        idx += 1;
    }
}

fuzz_target!(|input: Input<'_>| {
    if input.boundary.is_empty() || input.boundary.len() > 16 || input.body.len() > 2048 {
        return;
    }
    if Parser::<32, 3>::new(input.boundary).is_err() {
        return;
    }
    let (events, err) = oracle::parse(input.boundary, input.body, 32, 3);
    let (expected, finished) = expected_parts(&events);
    match input.buf_sel % 4 {
        0 => run::<8>(&input, &expected, finished, err),
        1 => run::<16>(&input, &expected, finished, err),
        2 => run::<31>(&input, &expected, finished, err),
        _ => run::<64>(&input, &expected, finished, err),
    }
});
