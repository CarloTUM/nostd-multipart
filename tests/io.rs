//! embedded-io / embedded-io-async adapters (feature-gated).
#![cfg(any(feature = "embedded-io", feature = "embedded-io-async"))]

use nostd_multipart::{Error, Event, Parser, ReadError};

const BODY: &[u8] = b"--b\r\nContent-Disposition: form-data; name=\"f\"\r\n\r\nhello\r\nworld\r\n--b\r\n\r\n\r\n--b--";

/// Reader yielding at most `max` bytes per read, optionally failing after `fail_after` reads.
struct Chunky<'a> {
    data: &'a [u8],
    max: usize,
    reads_left: Option<usize>,
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
        // A candidate delimiter prefix that never resolves inside 14 bytes...
        // "--b\r\n\r\n" then data "x\r\n--b" + pad... Build a body where the
        // parser must hold back more than the buffer holds.
        let r = run::<14>(100, None);
        // With BODY it may or may not hit; craft a dedicated one:
        let _ = r;
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
            let c = &mut self.0;
            if let Some(n) = &mut c.reads_left {
                if *n == 0 {
                    return Err(Boom);
                }
                *n -= 1;
            }
            let n = c.max.min(buf.len()).min(c.data.len());
            buf[..n].copy_from_slice(&c.data[..n]);
            c.data = &c.data[n..];
            Ok(n)
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
