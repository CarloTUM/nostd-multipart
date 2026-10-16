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

    pub(super) struct AsyncChunky<'a>(pub(super) Chunky<'a>);

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

/// Four parts for the part-level API: a text field, a file with two
/// `Content-Disposition` headers (the first wins), a part without any
/// disposition and an empty body, and a part with a malformed disposition.
const PARTS_BODY: &[u8] = b"--b\r\n\
Content-Disposition: form-data; name=\"version\"\r\n\
\r\n\
1.2.3\r\n\
--b\r\n\
Content-Type: text/plain\r\n\
Content-Disposition: form-data; name=\"firmware\"; filename=\"a.bin\"\r\n\
Content-Disposition: form-data; name=\"dup\"\r\n\
\r\n\
hello\r\nworld\r\n\
--b\r\n\
X-Other: 1\r\n\
\r\n\
\r\n\
--b\r\n\
Content-Disposition: ;;; bad\r\n\
\r\n\
x\r\n\
--b--";

fn parts_parser() -> Parser<'static, 96, 4> {
    Parser::new(b"b").unwrap()
}

#[cfg(feature = "embedded-io")]
mod blocking_parts {
    use super::*;
    use embedded_io::{Error as _, ErrorKind, Read, ReadExactError};
    use nostd_multipart::{MultipartReader, PartInfo};

    fn reader<const BUF: usize>(
        data: &[u8],
        max: usize,
        reads_left: Option<usize>,
    ) -> MultipartReader<'static, Chunky<'_>, BUF, 96, 4> {
        MultipartReader::new(
            Chunky {
                data,
                max,
                reads_left,
            },
            parts_parser(),
        )
    }

    /// Walks all four parts, checking the headers and collecting the bodies with
    /// `next_chunk` (part 1) and `Read::read` with a 3-byte buffer (part 2).
    fn walk<const BUF: usize>(max: usize) -> Result<(), ReadError<Boom>> {
        let mut r = reader::<BUF>(PARTS_BODY, max, None);

        let p = r.next_part()?.unwrap();
        assert_eq!(
            p.disposition_raw(),
            Some(&b"form-data; name=\"version\""[..])
        );
        assert_eq!(p.name().unwrap(), "version");
        assert!(p.filename().is_none());
        assert!(p.disposition().unwrap().is_form_data());
        let mut body = Vec::new();
        while let Some(chunk) = r.next_chunk()? {
            assert!(!chunk.is_empty());
            body.extend_from_slice(chunk);
        }
        assert_eq!(body, b"1.2.3");
        assert_eq!(r.next_chunk()?, None, "stays None after the body");
        assert_eq!(r.read(&mut [0; 4])?, 0, "read outside a body");
        assert_eq!(
            r.read_body(&mut [0; 4])?,
            Some(0),
            "read_body outside a body"
        );

        let p = r.next_part()?.unwrap();
        assert_eq!(p.name().unwrap(), "firmware");
        assert_eq!(p.filename().unwrap(), "a.bin");
        let mut body = Vec::new();
        let mut buf = [0u8; 3];
        loop {
            let n = r.read(&mut buf)?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&buf[..n]);
        }
        assert_eq!(body, b"hello\r\nworld");
        assert_eq!(r.read(&mut buf)?, 0);

        let p = r.next_part()?.unwrap();
        assert_eq!(p.disposition_raw(), None);
        assert!(p.disposition().is_none());
        assert!(p.name().is_none() && p.filename().is_none());
        assert_eq!(format!("{p:?}"), "PartInfo { disposition: None }");
        assert_eq!(r.next_chunk()?, None, "empty body");

        let p = r.next_part()?.unwrap();
        assert_eq!(p.disposition_raw(), Some(&b";;; bad"[..]));
        assert!(p.disposition().is_none(), "malformed value parses to None");
        assert_eq!(p.clone(), p);
        // Skipping the body by asking for the next part.
        assert!(r.next_part()?.is_none());
        assert!(r.next_part()?.is_none(), "stays None");
        assert_eq!(r.next_chunk()?, None);
        assert_eq!(r.read(&mut buf)?, 0);
        assert!(r.parser().is_finished());
        Ok(())
    }

    #[test]
    fn all_read_sizes_and_buffers() {
        for max in 1..=PARTS_BODY.len() {
            walk::<16>(max).unwrap_or_else(|e| panic!("BUF 16, max {max}: {e}"));
            walk::<96>(max).unwrap_or_else(|e| panic!("BUF 96, max {max}: {e}"));
            walk::<1024>(max).unwrap_or_else(|e| panic!("BUF 1024, max {max}: {e}"));
        }
    }

    #[test]
    fn mixing_read_next_chunk_and_next_event() {
        // Everything arrives in one read, so a part body is one `Data` event.
        let mut r = reader::<1024>(PARTS_BODY, usize::MAX, None);
        r.next_part().unwrap().unwrap();
        // A `Data` event handed out by `next_event` is not delivered again.
        assert_eq!(r.next_event(), Ok(Event::Data(b"1.2.3")));
        assert_eq!(r.read(&mut [0; 8]), Ok(0));
        assert_eq!(r.next_chunk(), Ok(None));

        r.next_part().unwrap().unwrap();
        assert_eq!(r.read(&mut []), Ok(0), "empty buffer reads nothing");
        let mut two = [0u8; 2];
        assert_eq!(r.read(&mut two), Ok(2));
        assert_eq!(&two, b"he");
        // `next_chunk` continues where `read` stopped ...
        assert_eq!(r.next_chunk(), Ok(Some(&b"llo\r\nworld"[..])));
        assert_eq!(r.next_chunk(), Ok(None));
        // ... and `next_part` after the body end still finds the next part.
        let p = r.next_part().unwrap().unwrap();
        assert_eq!(p.disposition_raw(), None);
    }

    #[test]
    fn next_part_from_any_position() {
        // Mid-body: the rest of part 1 is skipped.
        let mut r = reader::<1024>(PARTS_BODY, 4, None);
        r.next_part().unwrap().unwrap();
        assert!(r.next_chunk().unwrap().is_some());
        assert_eq!(r.next_part().unwrap().unwrap().name().unwrap(), "firmware");

        // Mid-headers: headers already consumed with `next_event` are missed.
        let mut r = reader::<1024>(PARTS_BODY, 4, None);
        assert_eq!(r.next_event(), Ok(Event::PartStart));
        assert!(matches!(r.next_event(), Ok(Event::Header { .. })));
        let p = r.next_part().unwrap().unwrap();
        assert_eq!(p.disposition_raw(), None);
        // After `PartStart` alone nothing is missed.
        let mut r = reader::<1024>(PARTS_BODY, 4, None);
        assert_eq!(r.next_event(), Ok(Event::PartStart));
        assert_eq!(r.next_part().unwrap().unwrap().name().unwrap(), "version");
        // Before any part, `next_chunk` and `read` have nothing to give.
        let mut r = reader::<1024>(PARTS_BODY, 4, None);
        assert_eq!(r.next_chunk(), Ok(None));
        assert_eq!(r.read(&mut [0; 4]), Ok(0));
    }

    /// `read_body` for every read size: exact fit, too small (nothing lost),
    /// and after a partial `read`.
    #[test]
    fn read_body_all_read_sizes() {
        for max in 1..=PARTS_BODY.len() {
            let mut r = reader::<32>(PARTS_BODY, max, None);
            r.next_part().unwrap().unwrap();
            let mut exact = [0u8; 5];
            assert_eq!(r.read_body(&mut exact), Ok(Some(5)), "max {max}");
            assert_eq!(&exact, b"1.2.3");
            assert_eq!(r.read_body(&mut exact), Ok(Some(0)), "body is over");

            r.next_part().unwrap().unwrap();
            let mut small = [0u8; 3];
            assert_eq!(r.read_body(&mut small), Ok(None), "max {max}");
            assert_eq!(&small, b"hel");
            let mut rest = Vec::new();
            while let Some(c) = r.next_chunk().unwrap() {
                rest.extend_from_slice(c);
            }
            assert_eq!(rest, b"lo\r\nworld", "max {max}");

            let p = r.next_part().unwrap().unwrap();
            assert!(p.name().is_none());
            let mut big = [0u8; 8];
            assert_eq!(r.read_body(&mut big), Ok(Some(0)), "empty body");

            r.next_part().unwrap().unwrap();
            assert_eq!(r.read(&mut small[..1]), Ok(1));
            assert_eq!(r.read_body(&mut big), Ok(Some(0)), "x was read already");
            assert_eq!(r.read_body(&mut []), Ok(Some(0)));
            assert!(r.next_part().unwrap().is_none());
        }
        // Zero-length `out` with a non-empty body: nothing fits. (8 reads of 7
        // bytes end exactly with the first part's headers.)
        let mut r = reader::<32>(PARTS_BODY, 7, Some(8));
        r.next_part().unwrap().unwrap();
        assert_eq!(r.read_body(&mut []), Ok(None));
        // I/O error inside read_body.
        assert_eq!(r.read_body(&mut [0; 16]), Err(ReadError::Io(Boom)));
    }

    #[test]
    fn sizes_are_documented() {
        use core::mem::size_of;
        assert_eq!(size_of::<PartInfo<256>>(), 256 + 2 * size_of::<usize>());
        assert_eq!(size_of::<PartInfo<64>>(), 64 + 2 * size_of::<usize>());
        // README "How much memory?", measured with a zero-sized reader.
        struct Zst;
        impl embedded_io::ErrorType for Zst {
            type Error = Boom;
        }
        impl Read for Zst {
            fn read(&mut self, _: &mut [u8]) -> Result<usize, Boom> {
                Ok(0)
            }
        }
        let big = size_of::<MultipartReader<'static, Zst, 1024, 512, 16>>();
        let small = size_of::<MultipartReader<'static, Zst, 256, 128, 8>>();
        println!("MultipartReader 1024/512: {big} B, 256/128: {small} B");
        if size_of::<usize>() == 8 {
            assert_eq!(big, 1648);
            assert_eq!(small, 496);
        }
    }

    #[test]
    fn read_exact_on_a_part() {
        let mut r = reader::<32>(PARTS_BODY, 5, None);
        r.next_part().unwrap().unwrap();
        let mut v = [0u8; 5];
        r.read_exact(&mut v).unwrap();
        assert_eq!(&v, b"1.2.3");
        assert_eq!(
            r.read_exact(&mut [0u8; 1]),
            Err(ReadExactError::UnexpectedEof)
        );
    }

    #[test]
    fn errors_propagate() {
        // I/O error while looking for the first part / while reading a body.
        let mut r = reader::<32>(PARTS_BODY, 5, Some(0));
        assert_eq!(r.next_part(), Err(ReadError::Io(Boom)));
        let mut r = reader::<32>(PARTS_BODY, 5, Some(11));
        r.next_part().unwrap().unwrap();
        let err = loop {
            match r.next_chunk() {
                Ok(Some(_)) => {}
                other => break other,
            }
        };
        assert_eq!(err, Err(ReadError::Io(Boom)));
        let mut r = reader::<32>(PARTS_BODY, 5, Some(11));
        r.next_part().unwrap().unwrap();
        let err = loop {
            match r.read(&mut [0; 2]) {
                Ok(n) if n > 0 => {}
                other => break other,
            }
        };
        assert_eq!(err, Err(ReadError::Io(Boom)));
        // Parse error.
        let mut r = reader::<32>(b"--b\r\nbad\r\n", 3, None);
        assert_eq!(r.next_part(), Err(ReadError::Parse(Error::Header)));
        // Truncated body.
        let mut r = reader::<32>(b"--b\r\n\r\nxyz", 3, None);
        r.next_part().unwrap().unwrap();
        let mut body = Vec::new();
        let err = loop {
            match r.next_chunk() {
                Ok(Some(d)) => body.extend_from_slice(d),
                other => break other,
            }
        };
        assert_eq!(body, b"xyz");
        assert_eq!(err, Err(ReadError::Parse(Error::UnexpectedEof)));
    }

    #[test]
    fn error_kinds() {
        assert_eq!(ReadError::Io(Boom).kind(), ErrorKind::Other);
        assert_eq!(
            ReadError::<Boom>::Parse(Error::Header).kind(),
            ErrorKind::InvalidData
        );
        assert_eq!(
            ReadError::<Boom>::BufferTooSmall.kind(),
            ErrorKind::OutOfMemory
        );
        let _: PartInfo<96>;
    }
}

#[cfg(feature = "embedded-io-async")]
mod async_parts {
    use super::asynch::AsyncChunky;
    use super::*;
    use embedded_io_async::{Error as _, ErrorKind, Read, ReadExactError};
    use nostd_multipart::MultipartReaderAsync;

    fn reader<const BUF: usize>(
        data: &[u8],
        max: usize,
        reads_left: Option<usize>,
    ) -> MultipartReaderAsync<'static, AsyncChunky<'_>, BUF, 96, 4> {
        MultipartReaderAsync::new(
            AsyncChunky(Chunky {
                data,
                max,
                reads_left,
            }),
            parts_parser(),
        )
    }

    async fn walk<const BUF: usize>(max: usize) -> Result<(), ReadError<Boom>> {
        let mut r = reader::<BUF>(PARTS_BODY, max, None);
        let p = r.next_part().await?.unwrap();
        assert_eq!(p.name().unwrap(), "version");
        let mut small = [0u8; 2];
        assert_eq!(r.read_body(&mut small).await?, None);
        assert_eq!(&small, b"1.");
        let mut body = Vec::new();
        while let Some(chunk) = r.next_chunk().await? {
            body.extend_from_slice(chunk);
        }
        assert_eq!(body, b"2.3");
        assert_eq!(r.next_chunk().await?, None);
        assert_eq!(r.read_body(&mut small).await?, Some(0));

        let p = r.next_part().await?.unwrap();
        assert_eq!(p.filename().unwrap(), "a.bin");
        let mut body = Vec::new();
        let mut buf = [0u8; 3];
        loop {
            let n = r.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&buf[..n]);
        }
        assert_eq!(body, b"hello\r\nworld");

        let p = r.next_part().await?.unwrap();
        assert_eq!(p.disposition_raw(), None);
        assert_eq!(r.next_chunk().await?, None);
        let p = r.next_part().await?.unwrap();
        assert!(p.disposition().is_none());
        assert!(r.next_part().await?.is_none());
        assert!(r.next_part().await?.is_none());
        assert_eq!(r.read(&mut buf).await?, 0);
        Ok(())
    }

    #[test]
    fn all_read_sizes() {
        futures::executor::block_on(async {
            for max in 1..=PARTS_BODY.len() {
                walk::<16>(max)
                    .await
                    .unwrap_or_else(|e| panic!("BUF 16, max {max}: {e}"));
                walk::<256>(max)
                    .await
                    .unwrap_or_else(|e| panic!("BUF 256, max {max}: {e}"));
            }
        });
    }

    #[test]
    fn mixing_and_errors() {
        futures::executor::block_on(async {
            let mut r = reader::<1024>(PARTS_BODY, usize::MAX, None);
            assert_eq!(r.next_chunk().await, Ok(None), "before any part");
            assert_eq!(r.read(&mut [0; 4]).await, Ok(0));
            r.next_part().await.unwrap().unwrap();
            assert_eq!(r.next_event().await, Ok(Event::Data(b"1.2.3")));
            assert_eq!(r.read(&mut [0; 8]).await, Ok(0));
            r.next_part().await.unwrap().unwrap();
            assert_eq!(r.read(&mut []).await, Ok(0));
            let mut two = [0u8; 2];
            assert_eq!(r.read(&mut two).await, Ok(2));
            assert_eq!(&two, b"he");
            assert_eq!(r.next_chunk().await, Ok(Some(&b"llo\r\nworld"[..])));
            assert_eq!(r.next_chunk().await, Ok(None));
            // Skip part 3 from inside its headers.
            assert_eq!(r.next_event().await, Ok(Event::PartStart));
            assert!(matches!(r.next_event().await, Ok(Event::Header { .. })));
            let p = r.next_part().await.unwrap().unwrap();
            assert_eq!(p.disposition_raw(), None);
            let p = r.next_part().await.unwrap().unwrap();
            assert_eq!(p.disposition_raw(), Some(&b";;; bad"[..]));
            let mut one = [0u8; 1];
            r.read_exact(&mut one).await.unwrap();
            assert_eq!(&one, b"x");
            assert_eq!(
                r.read_exact(&mut one).await,
                Err(ReadExactError::UnexpectedEof)
            );

            let mut r = reader::<32>(PARTS_BODY, 5, Some(0));
            assert_eq!(r.next_part().await, Err(ReadError::Io(Boom)));
            let mut r = reader::<32>(PARTS_BODY, 7, Some(8));
            r.next_part().await.unwrap().unwrap();
            assert_eq!(r.read_body(&mut [0; 16]).await, Err(ReadError::Io(Boom)));
            let mut r = reader::<32>(PARTS_BODY, 5, Some(11));
            r.next_part().await.unwrap().unwrap();
            let err = loop {
                match r.read(&mut [0; 2]).await {
                    Ok(n) if n > 0 => {}
                    other => break other,
                }
            };
            assert_eq!(err, Err(ReadError::Io(Boom)));
            let mut r = reader::<32>(PARTS_BODY, 5, Some(11));
            r.next_part().await.unwrap().unwrap();
            let err = loop {
                match r.next_chunk().await {
                    Ok(Some(_)) => {}
                    other => break other,
                }
            };
            assert_eq!(err, Err(ReadError::Io(Boom)));
            let mut r = reader::<32>(b"--b\r\nbad\r\n", 3, None);
            assert_eq!(r.next_part().await, Err(ReadError::Parse(Error::Header)));

            assert_eq!(ReadError::Io(Boom).kind(), ErrorKind::Other);
            assert_eq!(
                ReadError::<Boom>::Parse(Error::Header).kind(),
                ErrorKind::InvalidData
            );
            assert_eq!(
                ReadError::<Boom>::BufferTooSmall.kind(),
                ErrorKind::OutOfMemory
            );
        });
    }
}
