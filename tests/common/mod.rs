//! Shared test harness: drives the streaming parser with arbitrary chunking,
//! checks the API contract on every call, and normalises the event stream so it
//! can be compared with the oracle.
#![allow(dead_code)]

#[path = "../oracle.rs"]
pub mod oracle;

use nostd_multipart::{Error, Event, Parser};
pub use oracle::{OError, OEvent};

/// Map crate errors onto oracle errors by name.
pub fn oerr(e: Error) -> OError {
    match e {
        Error::Boundary => OError::Boundary,
        Error::HeaderTooLong => OError::HeaderTooLong,
        Error::TooManyHeaders => OError::TooManyHeaders,
        Error::Header => OError::Header,
        Error::UnexpectedEof => OError::UnexpectedEof,
        other => panic!("parser returned {other:?}, which the oracle never produces"),
    }
}

/// Collects normalised events: consecutive `Data` slices of one part are merged
/// into one `OEvent::Data` (never emitted when empty).
#[derive(Default)]
pub struct Collector {
    pub events: Vec<OEvent>,
    pending: Vec<u8>,
    in_body: bool,
}

impl Collector {
    pub fn push(&mut self, ev: Event<'_, '_>) {
        match ev {
            Event::Data(d) => {
                assert!(!d.is_empty(), "Data event must carry >= 1 byte");
                assert!(self.in_body, "Data outside of a body");
                self.pending.extend_from_slice(d);
            }
            other => {
                if !self.pending.is_empty() {
                    self.events.push(OEvent::Data(std::mem::take(&mut self.pending)));
                }
                match other {
                    Event::PartStart => self.events.push(OEvent::PartStart),
                    Event::Header { name, value } => self.events.push(OEvent::Header(name.to_vec(), value.to_vec())),
                    Event::HeadersEnd => {
                        self.in_body = true;
                        self.events.push(OEvent::HeadersEnd);
                    }
                    Event::PartEnd => {
                        self.in_body = false;
                        self.events.push(OEvent::PartEnd);
                    }
                    Event::Finished => self.events.push(OEvent::Finished),
                    Event::Data(_) => unreachable!(),
                }
            }
        }
    }
}

/// Feeds `body` to a fresh `Parser<LINE, MAX_HEADERS>` in the given `chunks`
/// (sizes must sum to `body.len()`), re-presenting unconsumed bytes exactly as a
/// real caller would. Panics on any API-contract violation.
pub fn run_chunked<const LINE: usize, const MAX_HEADERS: usize>(
    boundary: &[u8],
    body: &[u8],
    chunks: &[usize],
) -> (Vec<OEvent>, Option<OError>) {
    assert_eq!(chunks.iter().sum::<usize>(), body.len(), "chunks must cover body");
    let mut parser = match Parser::<LINE, MAX_HEADERS>::new(boundary) {
        Ok(p) => p,
        Err(e) => return (Vec::new(), Some(oerr(e))),
    };
    let mut col = Collector::default();
    let mut buf: Vec<u8> = Vec::new();
    let mut pos = 0usize;
    let mut chunk_idx = 0usize;
    let mut steps = 0usize;
    loop {
        let eof = chunk_idx >= chunks.len();
        steps += 1;
        assert!(steps < 10_000_000, "runaway parser loop");
        let range = buf.as_ptr_range();
        let min_buffer = parser.min_buffer();
        // (consumed, had_event, finished)
        let step: Result<(usize, bool, bool), Error> = match parser.parse(&buf, eof) {
            Err(e) => Err(e),
            Ok((consumed, ev)) => {
                assert!(consumed <= buf.len(), "consumed {} > len {}", consumed, buf.len());
                if buf.len() >= min_buffer || eof {
                    assert!(
                        consumed > 0 || ev.is_some(),
                        "progress guarantee violated: len={} eof={} min_buffer={}",
                        buf.len(),
                        eof,
                        min_buffer
                    );
                }
                if let Some(Event::Data(d)) = ev {
                    let p = d.as_ptr_range();
                    assert!(range.start <= p.start && p.end <= range.end, "Data is not a slice of input");
                    assert!(d.len() <= consumed, "Data longer than consumed");
                }
                let finished = matches!(ev, Some(Event::Finished));
                if let Some(e) = ev {
                    col.push(e);
                }
                Ok((consumed, ev.is_some(), finished))
            }
        };
        match step {
            Err(e) => {
                // Sticky error.
                assert_eq!(parser.parse(&buf, eof), Err(e));
                assert_eq!(parser.parse(b"", true), Err(e));
                return (col.events, Some(oerr(e)));
            }
            Ok((consumed, had_event, finished)) => {
                buf.drain(..consumed);
                if finished {
                    assert!(parser.is_finished());
                    // Absorbing: Finished again, consuming everything.
                    let r = parser.parse(b"epilogue", true).unwrap();
                    assert_eq!(r, (8, Some(Event::Finished)));
                    return (col.events, None);
                }
                if consumed == 0 && !had_event {
                    // Needs more input.
                    if eof {
                        panic!("(0, None) at eof");
                    }
                    let n = chunks[chunk_idx];
                    buf.extend_from_slice(&body[pos..pos + n]);
                    pos += n;
                    chunk_idx += 1;
                }
            }
        }
    }
}

/// Whole body in one chunk.
pub fn run_whole<const LINE: usize, const MAX_HEADERS: usize>(boundary: &[u8], body: &[u8]) -> (Vec<OEvent>, Option<OError>) {
    run_chunked::<LINE, MAX_HEADERS>(boundary, body, &[body.len()])
}

/// Asserts that every given chunking of `body` yields the oracle's result, and
/// returns it.
pub fn check_against_oracle<const LINE: usize, const MAX_HEADERS: usize>(
    boundary: &[u8],
    body: &[u8],
    chunkings: &[Vec<usize>],
) -> (Vec<OEvent>, Option<OError>) {
    let expected = oracle::parse(boundary, body, LINE, MAX_HEADERS);
    for ch in chunkings {
        let got = run_chunked::<LINE, MAX_HEADERS>(boundary, body, ch);
        assert_eq!(got, expected, "chunking {ch:?} of body {:?} differs from oracle", String::from_utf8_lossy(body));
    }
    expected
}

/// All 2^(n-1) compositions of `n` (chunk-size lists); `[]` for n = 0.
pub fn all_compositions(n: usize) -> Vec<Vec<usize>> {
    if n == 0 {
        return vec![vec![]];
    }
    let mut out = Vec::with_capacity(1 << (n - 1));
    for mask in 0u64..(1u64 << (n - 1)) {
        let mut v = Vec::new();
        let mut run = 1;
        for i in 0..n - 1 {
            if mask & (1 << i) != 0 {
                v.push(run);
                run = 1;
            } else {
                run += 1;
            }
        }
        v.push(run);
        out.push(v);
    }
    out
}

/// A useful standard set of chunkings for bodies of any size: whole, 1-byte,
/// 2-byte, 3-byte, 7-byte, and `count` pseudo-random splits.
pub fn standard_chunkings(len: usize, count: usize, seed: u64) -> Vec<Vec<usize>> {
    let mut out = vec![vec![len]];
    for size in [1usize, 2, 3, 7, 16] {
        if len > 0 {
            let mut v: Vec<usize> = std::iter::repeat(size).take(len / size).collect();
            if len % size != 0 {
                v.push(len % size);
            }
            out.push(v);
        }
    }
    let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..count {
        let mut v = Vec::new();
        let mut rem = len;
        while rem > 0 {
            let n = ((next() % 5) as usize + 1).min(rem);
            v.push(n);
            rem -= n;
        }
        out.push(v);
    }
    out.retain(|v| v.iter().sum::<usize>() == len);
    out
}
