//! Fuzz: the embedded-io adapter with arbitrary read sizes and buffer sizes
//! produces the same normalised event stream as the oracle, or
//! `BufferTooSmall` only when the buffer is smaller than `min_buffer()`.
#![no_main]

#[path = "../../tests/common/mod.rs"]
#[allow(dead_code)]
mod common;

use arbitrary::Arbitrary;
use common::{oerr, oracle, Collector, OError, OEvent};
use libfuzzer_sys::fuzz_target;
use nostd_multipart::{Event, MultipartReader, Parser, ReadError};

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

fn run<const BUF: usize>(
    boundary: &[u8],
    input: &Input<'_>,
) -> Result<(Vec<OEvent>, Option<OError>), ()> {
    let parser = Parser::<32, 3>::new(boundary).unwrap();
    let min = parser.min_buffer();
    let mut r = MultipartReader::<_, BUF, 32, 3>::new(
        R {
            data: input.body,
            reads: input.reads.clone().into_iter(),
        },
        parser,
    );
    let mut col = Collector::default();
    loop {
        match r.next_event() {
            Ok(Event::Finished) => {
                col.push(Event::Finished);
                return Ok((col.events, None));
            }
            Ok(ev) => col.push(ev),
            Err(ReadError::BufferTooSmall) => {
                assert!(
                    BUF < min,
                    "BufferTooSmall with BUF {BUF} >= min_buffer {min}"
                );
                return Err(());
            }
            Err(ReadError::Parse(e)) => {
                col.finish();
                return Ok((col.events, Some(oerr(e))));
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
