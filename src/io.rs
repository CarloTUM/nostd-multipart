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

/// Reader-independent part of the adapters.
struct Core<'b, const BUF: usize, const LINE: usize, const MAX_HEADERS: usize> {
    parser: Parser<'b, LINE, MAX_HEADERS>,
    buf: [u8; BUF],
    start: usize,
    end: usize,
    eof: bool,
    /// `(offset of the input slice in buf, event)` of the last successful `advance`.
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

    /// Runs the parser on the buffered bytes; `true` once an event is ready
    /// (fetch it with `event`), `false` if more input must be read.
    fn advance(&mut self) -> Result<bool, Error> {
        loop {
            let input_at = self.start;
            let input = self.buf.get(input_at..self.end).unwrap_or(&[]);
            let (consumed, ev) = self.parser.step(input, self.eof)?;
            self.start = self.start.saturating_add(consumed);
            if let Some(ev) = ev {
                self.pending = Some((input_at, ev));
                return Ok(true);
            }
            if consumed == 0 {
                return Ok(false);
            }
        }
    }

    /// The event recorded by the last successful `advance`.
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
        // the parser always progresses, so no read is requested then.)
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

/// Defines a reader adapter over `$read` (blocking or async).
macro_rules! reader {
    ($(#[$doc:meta])* $name:ident, $read:path $(, $async:ident $await:ident)?) => {
        $(#[$doc])*
        pub struct $name<'b, R, const BUF: usize, const LINE: usize = 512, const MAX_HEADERS: usize = 16> {
            reader: R,
            core: Core<'b, BUF, LINE, MAX_HEADERS>,
        }

        impl<'b, R: $read, const BUF: usize, const LINE: usize, const MAX_HEADERS: usize>
            $name<'b, R, BUF, LINE, MAX_HEADERS>
        {
            /// Wraps `reader`.
            pub fn new(reader: R, parser: Parser<'b, LINE, MAX_HEADERS>) -> Self {
                $name { reader, core: Core::new(parser) }
            }

            /// Returns the next event, reading as needed. After `Finished` it keeps
            /// returning `Finished`; after an error it keeps returning it.
            ///
            /// # Errors
            /// See [`ReadError`].
            pub $($async)? fn next_event(&mut self) -> Result<Event<'_, '_>, ReadError<R::Error>> {
                while !self.core.advance()? {
                    let dst = self.core.free()?;
                    let n = self.reader.read(dst)$(.$await)?.map_err(ReadError::Io)?;
                    self.core.filled(n);
                }
                Ok(self.core.event())
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
    };
}

#[cfg(feature = "embedded-io")]
reader!(
    /// Drives a [`Parser`] from a blocking [`embedded_io::Read`] with a `BUF`-byte
    /// buffer (`BUF >= parser.min_buffer()`).
    MultipartReader,
    embedded_io::Read
);

#[cfg(feature = "embedded-io-async")]
reader!(
    /// Drives a [`Parser`] from an [`embedded_io_async::Read`] with a `BUF`-byte
    /// buffer (`BUF >= parser.min_buffer()`).
    MultipartReaderAsync,
    embedded_io_async::Read,
    async await
);
