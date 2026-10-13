//! embedded-io / embedded-io-async adapters (feature-gated).
#![cfg(any(feature = "embedded-io", feature = "embedded-io-async"))]

use nostd_multipart::{Encoder, Error, Event, Parser, Part, ReadError, WriteError};

const BODY: &[u8] = b"--b\r\nContent-Disposition: form-data; name=\"f\"\r\n\r\nhello\r\nworld\r\n--b\r\n\r\n\r\n--b--";

/// Reader yielding at most `max` bytes per read, optionally failing after `fail_after` reads.
struct Chunky<'a> {
    data: &'a [u8],
    max: usize,
    reads_left: Option<usize>,
}

impl Chunky<'_> {
    fn take(&mut self, buf: &mut [u8]) -> Result<usize, Boom> {
        if let Some(n) = &mut self.reads_left {
            if *n == 0 {
                return Err(Boom);
            }
            *n -= 1;
        }
        let n = self.max.min(buf.len()).min(self.data.len());
        buf[..n].copy_from_slice(&self.data[..n]);
        self.data = &self.data[n..];
        Ok(n)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Boom;

impl core::fmt::Display for Boom {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("boom")
    }
}
impl std::error::Error for Boom {}

// `embedded_io_async::Error` is a re-export of `embedded_io::Error`: implement it once.
#[cfg(feature = "embedded-io")]
impl embedded_io::Error for Boom {
    fn kind(&self) -> embedded_io::ErrorKind {
        embedded_io::ErrorKind::Other
    }
}
#[cfg(all(feature = "embedded-io-async", not(feature = "embedded-io")))]
impl embedded_io_async::Error for Boom {
    fn kind(&self) -> embedded_io_async::ErrorKind {
        embedded_io_async::ErrorKind::Other
    }
}

#[cfg(feature = "embedded-io")]
impl embedded_io::ErrorType for Chunky<'_> {
    type Error = Boom;
}

#[cfg(feature = "embedded-io")]
impl embedded_io::Read for Chunky<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Boom> {
        self.take(buf)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Seen {
    headers: Vec<(Vec<u8>, Vec<u8>)>,
    data: Vec<Vec<u8>>,
    starts: usize,
    ends: usize,
}

fn record(seen: &mut Seen, ev: Event<'_, '_>) -> bool {
    match ev {
        Event::PartStart => seen.starts += 1,
        Event::Header { name, value } => seen.headers.push((name.to_vec(), value.to_vec())),
        Event::HeadersEnd => seen.data.push(Vec::new()),
        Event::Data(d) => seen.data.last_mut().unwrap().extend_from_slice(d),
        Event::PartEnd => seen.ends += 1,
        Event::Finished => return true,
    }
    false
}

fn expected() -> Seen {
    Seen {
        headers: vec![(
            b"Content-Disposition".to_vec(),
            b"form-data; name=\"f\"".to_vec(),
        )],
        data: vec![b"hello\r\nworld".to_vec(), vec![]],
        starts: 2,
        ends: 2,
    }
}

#[cfg(feature = "embedded-io")]
mod blocking {
    use super::*;
    use nostd_multipart::MultipartReader;

    fn run<const BUF: usize>(
        max: usize,
        reads_left: Option<usize>,
    ) -> Result<Seen, ReadError<Boom>> {
        let parser = Parser::<64, 4>::new(b"b").unwrap();
        let mut r = MultipartReader::<_, BUF, 64, 4>::new(
            Chunky {
                data: BODY,
                max,
                reads_left,
            },
            parser,
        );
        let mut seen = Seen::default();
        loop {
            let ev = r.next_event()?;
            if record(&mut seen, ev) {
                break;
            }
        }
        // Absorbing Finished.
        assert_eq!(r.next_event(), Ok(Event::Finished));
        assert!(r.parser().is_finished());
        let _rest = r.into_inner();
        Ok(seen)
    }

    #[test]
    fn all_read_sizes_and_buffers() {
        for max in 1..=BODY.len() {
            assert_eq!(run::<16>(max, None).unwrap(), expected(), "max {max}");
            assert_eq!(run::<64>(max, None).unwrap(), expected(), "max {max}");
            assert_eq!(run::<1024>(max, None).unwrap(), expected(), "max {max}");
        }
        // Exactly min_buffer (3 + 14 = 15 bytes).
        assert_eq!(Parser::<64, 4>::new(b"b").unwrap().min_buffer(), 15);
        assert_eq!(run::<15>(7, None).unwrap(), expected());
    }

    #[test]
    fn buffer_too_small() {
        let body = b"--b\r\n\r\nx\r\n--b        \r\n\r\n\r\n--b--"; // 8 pads: delimiter needs 15 bytes of lookahead
        let parser = Parser::<64, 4>::new(b"b").unwrap();
        let mut r = MultipartReader::<_, 14, 64, 4>::new(
            Chunky {
                data: body,
                max: 100,
                reads_left: None,
            },
            parser,
        );
        let mut err = None;
        for _ in 0..20 {
            match r.next_event() {
                Ok(Event::Finished) => break,
                Ok(_) => {}
                Err(e) => {
                    err = Some(e);
                    break;
                }
            }
        }
        assert_eq!(err, Some(ReadError::BufferTooSmall));
    }

    #[test]
    fn io_error_and_parse_error_propagate() {
        assert_eq!(run::<64>(5, Some(3)).err(), Some(ReadError::Io(Boom)));
        let parser = Parser::<64, 4>::new(b"b").unwrap();
        let mut r = MultipartReader::<_, 64, 64, 4>::new(
            Chunky {
                data: b"--b\r\nbad\r\n",
                max: 3,
                reads_left: None,
            },
            parser,
        );
        assert_eq!(r.next_event(), Ok(Event::PartStart));
        assert_eq!(r.next_event(), Err(ReadError::Parse(Error::Header)));
        assert_eq!(r.next_event(), Err(ReadError::Parse(Error::Header)));
        // Truncated body: EOF from the reader.
        let parser = Parser::<64, 4>::new(b"b").unwrap();
        let mut r = MultipartReader::<_, 64, 64, 4>::new(
            Chunky {
                data: b"--b\r\n\r\nxyz",
                max: 4,
                reads_left: None,
            },
            parser,
        );
        assert_eq!(r.next_event(), Ok(Event::PartStart));
        assert_eq!(r.next_event(), Ok(Event::HeadersEnd));
        let mut data = Vec::new();
        let err = loop {
            match r.next_event() {
                Ok(Event::Data(d)) => data.extend_from_slice(d),
                other => break other,
            }
        };
        assert_eq!(data, b"xyz");
        assert_eq!(err, Err(ReadError::Parse(Error::UnexpectedEof)));
    }

    #[test]
    fn read_error_traits() {
        use std::error::Error as _;
        let e: ReadError<Boom> = ReadError::Io(Boom);
        assert!(format!("{e}").contains("read error"));
        assert!(format!("{}", ReadError::<Boom>::Parse(Error::Header)).contains("multipart error"));
        assert!(format!("{}", ReadError::<Boom>::BufferTooSmall).contains("min_buffer"));
        assert!(e.source().is_none());
        assert_eq!(
            ReadError::<Boom>::from(Error::Boundary),
            ReadError::Parse(Error::Boundary)
        );
        let _c = e;
    }
}

#[cfg(feature = "embedded-io-async")]
mod asynch {
    use super::*;
    use nostd_multipart::MultipartReaderAsync;

    struct AsyncChunky<'a>(Chunky<'a>);

    impl embedded_io_async::ErrorType for AsyncChunky<'_> {
        type Error = Boom;
    }
    impl embedded_io_async::Read for AsyncChunky<'_> {
        async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Boom> {
            futures::future::ready(()).await; // yield once to exercise the state machine
            self.0.take(buf)
        }
    }

    async fn run<const BUF: usize>(
        max: usize,
        reads_left: Option<usize>,
    ) -> Result<Seen, ReadError<Boom>> {
        let parser = Parser::<64, 4>::new(b"b").unwrap();
        let mut r = MultipartReaderAsync::<_, BUF, 64, 4>::new(
            AsyncChunky(Chunky {
                data: BODY,
                max,
                reads_left,
            }),
            parser,
        );
        let mut seen = Seen::default();
        loop {
            let ev = r.next_event().await?;
            if record(&mut seen, ev) {
                break;
            }
        }
        assert_eq!(r.next_event().await, Ok(Event::Finished));
        assert!(r.parser().is_finished());
        let _rest = r.into_inner();
        Ok(seen)
    }

    #[test]
    fn all_read_sizes() {
        futures::executor::block_on(async {
            for max in 1..=BODY.len() {
                assert_eq!(run::<16>(max, None).await.unwrap(), expected(), "max {max}");
                assert_eq!(
                    run::<256>(max, None).await.unwrap(),
                    expected(),
                    "max {max}"
                );
            }
            assert_eq!(run::<64>(5, Some(2)).await.err(), Some(ReadError::Io(Boom)));
        });
    }

    #[test]
    fn errors() {
        futures::executor::block_on(async {
            let parser = Parser::<64, 4>::new(b"b").unwrap();
            let mut r = MultipartReaderAsync::<_, 64, 64, 4>::new(
                AsyncChunky(Chunky {
                    data: b"--b\r\nbad\r\n",
                    max: 3,
                    reads_left: None,
                }),
                parser,
            );
            assert_eq!(r.next_event().await, Ok(Event::PartStart));
            assert_eq!(r.next_event().await, Err(ReadError::Parse(Error::Header)));
            let body = b"--b\r\n\r\nx\r\n--b        \r\n\r\n\r\n--b--";
            let parser = Parser::<64, 4>::new(b"b").unwrap();
            let mut r = MultipartReaderAsync::<_, 14, 64, 4>::new(
                AsyncChunky(Chunky {
                    data: body,
                    max: 100,
                    reads_left: None,
                }),
                parser,
            );
            let mut err = None;
            for _ in 0..20 {
                match r.next_event().await {
                    Ok(Event::Finished) => break,
                    Ok(_) => {}
                    Err(e) => {
                        err = Some(e);
                        break;
                    }
                }
            }
            assert_eq!(err, Some(ReadError::BufferTooSmall));
        });
    }
}

/// Writer accepting at most `max` bytes per write, failing once `writes_left` is 0.
struct Sink {
    out: Vec<u8>,
    max: usize,
    writes_left: Option<usize>,
    flushed: bool,
}

impl Sink {
    fn new(max: usize, writes_left: Option<usize>) -> Self {
        Sink {
            out: Vec::new(),
            max,
            writes_left,
            flushed: false,
        }
    }

    fn put(&mut self, buf: &[u8]) -> Result<usize, Boom> {
        if let Some(n) = &mut self.writes_left {
            if *n == 0 {
                return Err(Boom);
            }
            *n -= 1;
        }
        let n = self.max.min(buf.len());
        self.out.extend_from_slice(&buf[..n]);
        self.flushed = false;
        Ok(n)
    }
}

/// Two parts, the second with a header longer than the writers' 64-byte chunk.
const PARTS: [Part<'static>; 2] = [
    Part {
        name: b"version",
        filename: None,
        content_type: None,
    },
    Part {
        name: b"firmware",
        filename: Some(b"a-file-name-that-is-long-enough-to-need-two-chunks.bin"),
        content_type: Some(b"application/octet-stream"),
    },
];
const DATA: [&[u8]; 2] = [b"1.2.3", b"\x7fELF\r\n--bnd\r\n"];

/// The same body produced by the slice-based API in one go.
fn reference_body() -> Vec<u8> {
    let mut enc = Encoder::new(b"bnd").unwrap();
    let mut body = Vec::new();
    let mut out = [0u8; 512];
    for (part, data) in PARTS.iter().zip(DATA) {
        let (n, done) = enc.write_part_header(part, &mut out).unwrap();
        assert!(done);
        body.extend_from_slice(&out[..n]);
        body.extend_from_slice(data);
    }
    let (n, done) = enc.write_close(&mut out);
    assert!(done);
    body.extend_from_slice(&out[..n]);
    body
}

const BAD_PART: Part<'static> = Part {
    name: b"f",
    filename: None,
    content_type: Some(b"text/plain\r\nX-Injected: 1"),
};

#[test]
fn write_error_traits() {
    use std::error::Error as _;
    let e: WriteError<Boom> = WriteError::Io(Boom);
    assert!(format!("{e}").contains("write error"));
    assert!(format!("{}", WriteError::<Boom>::Encode(Error::Header)).contains("multipart error"));
    assert!(e.source().is_none());
    let _c = e;
}

#[cfg(feature = "embedded-io")]
mod blocking_write {
    use super::*;

    impl embedded_io::ErrorType for Sink {
        type Error = Boom;
    }
    impl embedded_io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> Result<usize, Boom> {
            self.put(buf)
        }
        fn flush(&mut self) -> Result<(), Boom> {
            self.flushed = true;
            Ok(())
        }
    }

    #[test]
    fn matches_slice_api_for_all_write_sizes() {
        let expected = reference_body();
        for max in 1..=80 {
            let mut enc = Encoder::new(b"bnd").unwrap();
            let mut w = Sink::new(max, None);
            for (part, data) in PARTS.iter().zip(DATA) {
                enc.write_part_header_to(part, &mut w).unwrap();
                embedded_io::Write::write_all(&mut w, data).unwrap();
            }
            enc.write_close_to(&mut w).unwrap();
            assert!(w.flushed);
            assert_eq!(w.out, expected, "max {max}");
        }
    }

    #[test]
    fn errors() {
        let mut enc = Encoder::new(b"bnd").unwrap();
        let mut w = Sink::new(100, None);
        assert_eq!(
            enc.write_part_header_to(&BAD_PART, &mut w),
            Err(WriteError::Encode(Error::Header))
        );
        assert!(w.out.is_empty());
        let mut w = Sink::new(10, Some(1));
        assert_eq!(
            enc.write_part_header_to(&PARTS[0], &mut w),
            Err(WriteError::Io(Boom))
        );
        let mut enc = Encoder::new(b"bnd").unwrap();
        assert_eq!(enc.write_close_to(&mut Sink::new(1, Some(0))), Err(Boom));
    }

    #[test]
    fn close_longer_than_one_chunk() {
        let boundary = [b'x'; 70];
        let mut enc = Encoder::new(&boundary).unwrap();
        let mut w = Sink::new(100, None);
        enc.write_close_to(&mut w).unwrap();
        assert_eq!(w.out, [&b"\r\n--"[..], &boundary, b"--\r\n"].concat());
    }
}

#[cfg(feature = "embedded-io-async")]
mod async_write {
    use super::*;

    struct AsyncSink(Sink);

    impl embedded_io_async::ErrorType for AsyncSink {
        type Error = Boom;
    }
    impl embedded_io_async::Write for AsyncSink {
        async fn write(&mut self, buf: &[u8]) -> Result<usize, Boom> {
            futures::future::ready(()).await;
            self.0.put(buf)
        }
        async fn flush(&mut self) -> Result<(), Boom> {
            self.0.flushed = true;
            Ok(())
        }
    }

    #[test]
    fn matches_slice_api_and_errors() {
        futures::executor::block_on(async {
            let expected = reference_body();
            for max in [1, 7, 64, 500] {
                let mut enc = Encoder::new(b"bnd").unwrap();
                let mut w = AsyncSink(Sink::new(max, None));
                for (part, data) in PARTS.iter().zip(DATA) {
                    enc.write_part_header_to_async(part, &mut w).await.unwrap();
                    embedded_io_async::Write::write_all(&mut w, data)
                        .await
                        .unwrap();
                }
                enc.write_close_to_async(&mut w).await.unwrap();
                assert!(w.0.flushed);
                assert_eq!(w.0.out, expected, "max {max}");
            }
            let mut enc = Encoder::new(b"bnd").unwrap();
            let mut w = AsyncSink(Sink::new(100, None));
            assert_eq!(
                enc.write_part_header_to_async(&BAD_PART, &mut w).await,
                Err(WriteError::Encode(Error::Header))
            );
            let mut w = AsyncSink(Sink::new(10, Some(1)));
            assert_eq!(
                enc.write_part_header_to_async(&PARTS[0], &mut w).await,
                Err(WriteError::Io(Boom))
            );
            let mut w = AsyncSink(Sink::new(1, Some(0)));
            assert_eq!(enc.write_close_to_async(&mut w).await, Err(Boom));
            // A close delimiter longer than one 64-byte chunk.
            let boundary = [b'x'; 70];
            let mut w = AsyncSink(Sink::new(100, None));
            Encoder::new(&boundary)
                .unwrap()
                .write_close_to_async(&mut w)
                .await
                .unwrap();
            assert_eq!(w.0.out, [&b"\r\n--"[..], &boundary, b"--\r\n"].concat());
        });
    }
}
