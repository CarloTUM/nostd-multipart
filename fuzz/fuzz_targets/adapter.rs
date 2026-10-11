//! Fuzz: the embedded-io adapter with arbitrary read sizes and buffer sizes
//! produces the same normalised event stream as the oracle, or
//! `BufferTooSmall` only when the buffer is smaller than `min_buffer()`.
#![no_main]

#[path = "../../tests/oracle.rs"]
#[allow(dead_code)]
mod oracle;

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use nostd_multipart::{Error, Event, MultipartReader, Parser, ReadError};
use oracle::{OError, OEvent};

#[derive(Arbitrary, Debug)]
struct Input<'a> {
    boundary: &'a [u8],
    reads: Vec<u8>,
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

fn run<const BUF: usize>(boundary: &[u8], input: &Input<'_>) -> Result<(Vec<OEvent>, Option<OError>), ()> {
    let parser = Parser::<32, 3>::new(boundary).unwrap();
    let min = parser.min_buffer();
    let mut r = MultipartReader::<_, BUF, 32, 3>::new(R { data: input.body, reads: input.reads.clone().into_iter() }, parser);
    let mut events = Vec::new();
    let mut pending: Vec<u8> = Vec::new();
    let flush = |pending: &mut Vec<u8>, events: &mut Vec<OEvent>| {
        if !pending.is_empty() {
            events.push(OEvent::Data(std::mem::take(pending)));
        }
    };
    loop {
        match r.next_event() {
            Ok(Event::Data(d)) => pending.extend_from_slice(d),
            Ok(ev) => {
                flush(&mut pending, &mut events);
                match ev {
                    Event::PartStart => events.push(OEvent::PartStart),
                    Event::Header { name, value } => events.push(OEvent::Header(name.to_vec(), value.to_vec())),
                    Event::HeadersEnd => events.push(OEvent::HeadersEnd),
                    Event::PartEnd => events.push(OEvent::PartEnd),
                    Event::Finished => {
                        events.push(OEvent::Finished);
                        return Ok((events, None));
                    }
                    Event::Data(_) => unreachable!(),
                }
            }
            Err(ReadError::BufferTooSmall) => {
                assert!(BUF < min, "BufferTooSmall with BUF {BUF} >= min_buffer {min}");
                return Err(());
            }
            Err(ReadError::Parse(e)) => {
                flush(&mut pending, &mut events);
                let oe = match e {
                    Error::Boundary => OError::Boundary,
                    Error::HeaderTooLong => OError::HeaderTooLong,
                    Error::TooManyHeaders => OError::TooManyHeaders,
                    Error::Header => OError::Header,
                    Error::UnexpectedEof => OError::UnexpectedEof,
                    _ => panic!("unexpected {e:?}"),
                };
                return Ok((events, Some(oe)));
            }
            Err(ReadError::Io(never)) => match never {},
        }
    }
}

fuzz_target!(|input: Input<'_>| {
    if input.boundary.is_empty() || input.boundary.len() > 16 || input.body.len() > 2048 {
        return;
    }
    if Parser::<32, 3>::new(input.boundary).is_err() {
        return;
    }
    let expected = oracle::parse(input.boundary, input.body, 32, 3);
    let got = match input.buf_sel % 4 {
        0 => run::<8>(input.boundary, &input),
        1 => run::<16>(input.boundary, &input),
        2 => run::<31>(input.boundary, &input),
        _ => run::<64>(input.boundary, &input),
    };
    if let Ok(got) = got {
        assert_eq!(got, expected);
    }
});
