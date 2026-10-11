//! Adapters that pull bytes from an `embedded-io` / `embedded-io-async` reader
//! into a fixed buffer and drive the [`Parser`]. Enabled by the features of the
//! same names.

use core::fmt;

use crate::parser::Ev;
use crate::{Error, Event, Parser};

/// Error of the reader adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadError<E> {
    /// The underlying reader failed.
    Io(E),
    /// The body is malformed.
    Parse(Error),
    /// The buffer is full but the parser cannot make progress: `BUF` is smaller
    /// than [`Parser::min_buffer`].
    BufferTooSmall,
}

impl<E> From<Error> for ReadError<E> {
    fn from(e: Error) -> Self {
        ReadError::Parse(e)
    }
}

impl<E: fmt::Debug> fmt::Display for ReadError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadError::Io(e) => write!(f, "read error: {e:?}"),
            ReadError::Parse(e) => write!(f, "multipart error: {e}"),
            ReadError::BufferTooSmall => f.write_str("buffer smaller than Parser::min_buffer()"),
        }
    }
}

impl<E: fmt::Debug> core::error::Error for ReadError<E> {}

/// What the shared core wants the adapter to do next.
enum Advance {
    /// An event is ready; fetch it with `Core::event`.
    Ready,
    /// Bytes were consumed without an event; call `advance` again.
    Again,
    /// Read more input into `Core::free`.
    NeedRead,
}

/// Reader-independent part of the adapters.
struct Core<'b, const BUF: usize, const LINE: usize, const MAX_HEADERS: usize> {
    parser: Parser<'b, LINE, MAX_HEADERS>,
    buf: [u8; BUF],
    start: usize,
    end: usize,
    eof: bool,
    /// `(offset of the input slice in buf, event)` of the last `Ready`.
    pending: Option<(usize, Ev)>,
}

impl<'b, const BUF: usize, const LINE: usize, const MAX_HEADERS: usize>
    Core<'b, BUF, LINE, MAX_HEADERS>
{
    fn new(parser: Parser<'b, LINE, MAX_HEADERS>) -> Self {
        Core {
            parser,
            buf: [0; BUF],
            start: 0,
            end: 0,
            eof: false,
            pending: None,
        }
    }

    fn advance(&mut self) -> Result<Advance, Error> {
        let input_at = self.start;
        let input = self.buf.get(input_at..self.end).unwrap_or(&[]);
        let (consumed, ev) = self.parser.step(input, self.eof)?;
        self.start = self.start.saturating_add(consumed);
        Ok(match ev {
            Some(ev) => {
                self.pending = Some((input_at, ev));
                Advance::Ready
            }
            None if consumed > 0 => Advance::Again,
            None => Advance::NeedRead,
        })
    }

    /// The event recorded by the last `Ready`.
    fn event(&self) -> Event<'_, '_> {
        let (at, ev) = self.pending.unwrap_or((0, Ev::Finished));
        self.parser
            .materialize(self.buf.get(at..self.end).unwrap_or(&[]), ev)
    }

    /// Compacts the buffer and returns the free space to read into.
    fn free<E>(&mut self) -> Result<&mut [u8], ReadError<E>> {
        if self.start > 0 {
            self.buf.copy_within(self.start..self.end, 0);
            self.end = self.end.saturating_sub(self.start);
            self.start = 0;
        }
        // A full buffer without progress means BUF < min_buffer(). (With `eof`
        // the parser always progresses, so `NeedRead` never happens then.)
        self.buf
            .get_mut(self.end..)
            .filter(|s| !s.is_empty())
            .ok_or(ReadError::BufferTooSmall)
    }

    fn filled(&mut self, n: usize) {
        if n == 0 {
            self.eof = true;
        } else {
            self.end = self.end.saturating_add(n).min(BUF);
        }
    }
}

#[cfg(feature = "embedded-io")]
pub mod blocking {
    use super::{Advance, Core, ReadError};
    use crate::{Event, Parser};

    /// Drives a [`Parser`] from a blocking [`embedded_io::Read`] with a `BUF`-byte
    /// buffer (`BUF >= parser.min_buffer()`).
    pub struct MultipartReader<
        'b,
        R,
        const BUF: usize,
        const LINE: usize = 512,
        const MAX_HEADERS: usize = 16,
    > {
        reader: R,
        core: Core<'b, BUF, LINE, MAX_HEADERS>,
    }

    impl<
            'b,
            R: embedded_io::Read,
            const BUF: usize,
            const LINE: usize,
            const MAX_HEADERS: usize,
        > MultipartReader<'b, R, BUF, LINE, MAX_HEADERS>
    {
        /// Wraps `reader`.
        pub fn new(reader: R, parser: Parser<'b, LINE, MAX_HEADERS>) -> Self {
            MultipartReader {
                reader,
                core: Core::new(parser),
            }
        }

        /// Returns the next event, reading as needed. After `Finished` it keeps
        /// returning `Finished`; after an error it keeps returning it.
        ///
        /// # Errors
        /// See [`ReadError`].
        pub fn next_event(&mut self) -> Result<Event<'_, '_>, ReadError<R::Error>> {
            let mut need_read = false;
            loop {
                if need_read {
                    let dst = self.core.free()?;
                    let n = self.reader.read(dst).map_err(ReadError::Io)?;
                    self.core.filled(n);
                    need_read = false;
                }
                match self.core.advance()? {
                    Advance::Ready => return Ok(self.core.event()),
                    Advance::Again => {}
                    Advance::NeedRead => need_read = true,
                }
            }
        }

        /// The wrapped parser.
        pub fn parser(&self) -> &Parser<'b, LINE, MAX_HEADERS> {
            &self.core.parser
        }

        /// Returns the reader.
        pub fn into_inner(self) -> R {
            self.reader
        }
    }
}

#[cfg(feature = "embedded-io-async")]
pub mod asynch {
    use super::{Advance, Core, ReadError};
    use crate::{Event, Parser};

    /// Drives a [`Parser`] from an [`embedded_io_async::Read`] with a `BUF`-byte
    /// buffer (`BUF >= parser.min_buffer()`).
    pub struct MultipartReaderAsync<
        'b,
        R,
        const BUF: usize,
        const LINE: usize = 512,
        const MAX_HEADERS: usize = 16,
    > {
        reader: R,
        core: Core<'b, BUF, LINE, MAX_HEADERS>,
    }

    impl<
            'b,
            R: embedded_io_async::Read,
            const BUF: usize,
            const LINE: usize,
            const MAX_HEADERS: usize,
        > MultipartReaderAsync<'b, R, BUF, LINE, MAX_HEADERS>
    {
        /// Wraps `reader`.
        pub fn new(reader: R, parser: Parser<'b, LINE, MAX_HEADERS>) -> Self {
            MultipartReaderAsync {
                reader,
                core: Core::new(parser),
            }
        }

        /// Returns the next event, reading as needed. After `Finished` it keeps
        /// returning `Finished`; after an error it keeps returning it.
        ///
        /// # Errors
        /// See [`ReadError`].
        pub async fn next_event(&mut self) -> Result<Event<'_, '_>, ReadError<R::Error>> {
            let mut need_read = false;
            loop {
                if need_read {
                    let dst = self.core.free()?;
                    let n = self.reader.read(dst).await.map_err(ReadError::Io)?;
                    self.core.filled(n);
                    need_read = false;
                }
                match self.core.advance()? {
                    Advance::Ready => return Ok(self.core.event()),
                    Advance::Again => {}
                    Advance::NeedRead => need_read = true,
                }
            }
        }

        /// The wrapped parser.
        pub fn parser(&self) -> &Parser<'b, LINE, MAX_HEADERS> {
            &self.core.parser
        }

        /// Returns the reader.
        pub fn into_inner(self) -> R {
            self.reader
        }
    }
}
