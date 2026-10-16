//! Adapters that pull bytes from an `embedded-io` / `embedded-io-async` reader
//! into a fixed buffer and drive the [`Parser`], and that push [`Encoder`] output
//! into a writer. Enabled by the features of the same names.

use core::fmt;

use crate::parser::Ev;
use crate::{
    content_disposition, ContentDisposition, Encoder, Error, Event, ParamValue, Parser, Part,
};

// `embedded_io_async` re-exports `Error`, `ErrorType` and `ErrorKind` from
// `embedded_io`, so one impl serves both features.
#[cfg(feature = "embedded-io")]
use embedded_io as eio;
#[cfg(all(feature = "embedded-io-async", not(feature = "embedded-io")))]
use embedded_io_async as eio;

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

impl<E: eio::Error> eio::Error for ReadError<E> {
    fn kind(&self) -> eio::ErrorKind {
        match self {
            ReadError::Io(e) => e.kind(),
            ReadError::Parse(_) => eio::ErrorKind::InvalidData,
            ReadError::BufferTooSmall => eio::ErrorKind::OutOfMemory,
        }
    }
}

/// What the reader adapters remember about a part (DESIGN.md §14): a copy of
/// its `Content-Disposition` value, which is at most `LINE` bytes, the parser's
/// line size. Returned by `next_part`. Other headers are not kept; read them
/// with `next_event` if you need them.
#[derive(Clone, PartialEq, Eq)]
pub struct PartInfo<const LINE: usize> {
    disposition: [u8; LINE],
    len: Option<usize>,
}

impl<const LINE: usize> PartInfo<LINE> {
    const fn new() -> Self {
        PartInfo {
            disposition: [0; LINE],
            len: None,
        }
    }

    /// Keeps the first `Content-Disposition` value of the part.
    fn set_disposition(&mut self, value: &[u8]) {
        if self.len.is_some() {
            return;
        }
        if let Some(dst) = self.disposition.get_mut(..value.len()) {
            dst.copy_from_slice(value);
            self.len = Some(value.len());
        }
    }

    /// The raw `Content-Disposition` value, if the part had that header.
    #[must_use]
    pub fn disposition_raw(&self) -> Option<&[u8]> {
        self.len.and_then(|n| self.disposition.get(..n))
    }

    /// The parsed `Content-Disposition` (see [`content_disposition`]); `None` if
    /// the header is missing or malformed.
    #[must_use]
    pub fn disposition(&self) -> Option<ContentDisposition<'_>> {
        content_disposition(self.disposition_raw()?).ok()
    }

    /// The field name (`name` parameter), if any.
    #[must_use]
    pub fn name(&self) -> Option<ParamValue<'_>> {
        self.disposition()?.name()
    }

    /// The file name (`filename` parameter), if any.
    #[must_use]
    pub fn filename(&self) -> Option<ParamValue<'_>> {
        self.disposition()?.filename()
    }
}

impl<const LINE: usize> fmt::Debug for PartInfo<LINE> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PartInfo")
            .field("disposition", &self.disposition_raw())
            .finish()
    }
}

/// Error of the encoder's writer methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteError<E> {
    /// The underlying writer failed. The body written so far is incomplete;
    /// start a new request with a new [`Encoder`].
    Io(E),
    /// The part cannot be encoded (see [`Encoder::write_part_header`]). Nothing
    /// has been written.
    Encode(Error),
}

impl<E: fmt::Debug> fmt::Display for WriteError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::Io(e) => write!(f, "write error: {e:?}"),
            WriteError::Encode(e) => write!(f, "multipart error: {e}"),
        }
    }
}

impl<E: fmt::Debug> core::error::Error for WriteError<E> {}

/// Stack chunk the writer methods encode into before each `write_all`.
const WRITE_CHUNK: usize = 64;

/// Defines `Encoder` methods that write to `$write` (blocking or async).
macro_rules! writer {
    ($header:ident, $close:ident, $write:path $(, $async:ident $await:ident)?) => {
        impl Encoder<'_> {
            /// Writes the complete prefix of `part` to `w` (see
            /// [`Self::write_part_header`]). Write the part's data to `w`
            /// yourself afterwards. Uses a 64-byte stack buffer.
            ///
            /// # Errors
            /// See [`WriteError`].
            pub $($async)? fn $header<W: $write>(
                &mut self,
                part: &Part<'_>,
                w: &mut W,
            ) -> Result<(), WriteError<W::Error>> {
                let mut chunk = [0u8; WRITE_CHUNK];
                loop {
                    let (n, done) = self.write_part_header(part, &mut chunk).map_err(WriteError::Encode)?;
                    w.write_all(chunk.get(..n).unwrap_or(&[]))$(.$await)?.map_err(WriteError::Io)?;
                    if done {
                        return Ok(());
                    }
                }
            }

            /// Writes the complete close delimiter to `w` (see [`Self::write_close`])
            /// and flushes it.
            ///
            /// # Errors
            /// The writer's error.
            pub $($async)? fn $close<W: $write>(&mut self, w: &mut W) -> Result<(), W::Error> {
                let mut chunk = [0u8; WRITE_CHUNK];
                loop {
                    let (n, done) = self.write_close(&mut chunk);
                    w.write_all(chunk.get(..n).unwrap_or(&[]))$(.$await)??;
                    if done {
                        return w.flush()$(.$await)?;
                    }
                }
            }
        }
    };
}

#[cfg(feature = "embedded-io")]
writer!(write_part_header_to, write_close_to, embedded_io::Write);

#[cfg(feature = "embedded-io-async")]
writer!(
    write_part_header_to_async,
    write_close_to_async,
    embedded_io_async::Write,
    async await
);

/// Reader-independent part of the adapters.
struct Core<'b, const BUF: usize, const LINE: usize, const MAX_HEADERS: usize> {
    parser: Parser<'b, LINE, MAX_HEADERS>,
    buf: [u8; BUF],
    start: usize,
    end: usize,
    eof: bool,
    /// `(offset of the input slice in buf, event)` of the last successful `advance`.
    pending: Option<(usize, Ev)>,
    /// Between `HeadersEnd` and the following `PartEnd`.
    in_body: bool,
    /// Bytes of a pending `Data` event already handed to the caller.
    chunk_pos: usize,
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
            in_body: false,
            chunk_pos: 0,
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
                match ev {
                    Ev::HeadersEnd => self.in_body = true,
                    Ev::PartEnd | Ev::Finished => self.in_body = false,
                    _ => {}
                }
                self.chunk_pos = 0;
                self.pending = Some((input_at, ev));
                return Ok(true);
            }
            if consumed == 0 {
                return Ok(false);
            }
        }
    }

    /// The event recorded by the last successful `advance`. A `Data` event is
    /// handed out whole, so `read` / `next_chunk` will not deliver it again.
    fn event(&mut self) -> Event<'_, '_> {
        let (at, ev) = self.pending.unwrap_or((0, Ev::Finished));
        if let Ev::Data { end } = ev {
            self.chunk_pos = end;
        }
        self.parser
            .materialize(self.buf.get(at..self.end).unwrap_or(&[]), ev)
    }

    /// The not yet delivered bytes of the pending `Data` event (empty if the
    /// pending event is not `Data`).
    fn chunk_rest(&self) -> &[u8] {
        match self.pending {
            Some((at, Ev::Data { end })) => self
                .buf
                .get(at.saturating_add(self.chunk_pos)..at.saturating_add(end))
                .unwrap_or(&[]),
            _ => &[],
        }
    }

    /// Delivers the rest of the pending `Data` event in one piece.
    fn take_chunk(&mut self) -> Option<&[u8]> {
        let Some((at, Ev::Data { end })) = self.pending else {
            return None;
        };
        let from = at.saturating_add(self.chunk_pos);
        self.chunk_pos = end;
        self.buf
            .get(from..at.saturating_add(end))
            .filter(|s| !s.is_empty())
    }

    /// Copies as much of the pending `Data` event into `out` as fits.
    fn copy_out(&mut self, out: &mut [u8]) -> usize {
        let rest = self.chunk_rest();
        let n = rest.len().min(out.len());
        if let (Some(dst), Some(src)) = (out.get_mut(..n), rest.get(..n)) {
            dst.copy_from_slice(src);
        }
        self.chunk_pos = self.chunk_pos.saturating_add(n);
        n
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

            /// Runs the parser until it has produced an event, reading as needed.
            $($async)? fn fill(&mut self) -> Result<(), ReadError<R::Error>> {
                while !self.core.advance()? {
                    let dst = self.core.free()?;
                    let n = self.reader.read(dst)$(.$await)?.map_err(ReadError::Io)?;
                    self.core.filled(n);
                }
                Ok(())
            }

            /// Returns the next event, reading as needed. After `Finished` it keeps
            /// returning `Finished`; after an error it keeps returning it.
            ///
            /// # Errors
            /// See [`ReadError`].
            pub $($async)? fn next_event(&mut self) -> Result<Event<'_, '_>, ReadError<R::Error>> {
                self.fill()$(.$await)??;
                Ok(self.core.event())
            }

            /// Skips the rest of the current part, if any, and reads the next
            /// part's headers. `None` once the body is complete (and on every
            /// later call).
            ///
            /// Follow it with [`Self::next_chunk`] or the `Read` impl to get the
            /// part's data; or call `next_part` again to skip the part.
            ///
            /// # Errors
            /// See [`ReadError`].
            pub $($async)? fn next_part(&mut self) -> Result<Option<PartInfo<LINE>>, ReadError<R::Error>> {
                let mut info = PartInfo::new();
                loop {
                    match self.next_event()$(.$await)?? {
                        Event::PartStart => info = PartInfo::new(),
                        Event::Header { name, value }
                            if name.eq_ignore_ascii_case(b"content-disposition") =>
                        {
                            info.set_disposition(value);
                        }
                        Event::HeadersEnd => return Ok(Some(info)),
                        Event::Finished => return Ok(None),
                        _ => {}
                    }
                }
            }

            /// Copies the whole body of the current part into `out`: `Some(len)`
            /// if it fit, `None` if the body is longer than `out` (then `out` is
            /// full and the rest of the body is still unread). Meant for small
            /// text fields; for files use [`Self::next_chunk`].
            ///
            /// # Errors
            /// See [`ReadError`].
            pub $($async)? fn read_body(&mut self, out: &mut [u8]) -> Result<Option<usize>, ReadError<R::Error>> {
                let mut filled = 0usize;
                while self.core.in_body {
                    if self.core.chunk_rest().is_empty() {
                        self.fill()$(.$await)??;
                        continue;
                    }
                    let Some(dst) = out.get_mut(filled..).filter(|d| !d.is_empty()) else {
                        return Ok(None);
                    };
                    filled = filled.saturating_add(self.core.copy_out(dst));
                }
                Ok(Some(filled))
            }

            /// The next piece of the current part's body, zero-copy: a non-empty
            /// slice of the internal buffer, valid until the next call. `None`
            /// when the part's body is complete, and outside a part body.
            ///
            /// # Errors
            /// See [`ReadError`].
            pub $($async)? fn next_chunk(&mut self) -> Result<Option<&[u8]>, ReadError<R::Error>> {
                if !self.core.in_body {
                    return Ok(None);
                }
                if self.core.chunk_rest().is_empty() {
                    self.fill()$(.$await)??;
                }
                Ok(self.core.take_chunk())
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

        impl<'b, R: $read, const BUF: usize, const LINE: usize, const MAX_HEADERS: usize> eio::ErrorType
            for $name<'b, R, BUF, LINE, MAX_HEADERS>
        {
            type Error = ReadError<R::Error>;
        }

        /// Reads the body of the current part (the one whose headers
        /// [`Self::next_part`] returned last), copying it into `buf`. Returns 0 at
        /// the end of the part's body and outside a part body, so `read_exact`
        /// and friends work on a part. Mixing with [`Self::next_chunk`] is fine:
        /// no byte is delivered twice.
        impl<'b, R: $read, const BUF: usize, const LINE: usize, const MAX_HEADERS: usize> $read
            for $name<'b, R, BUF, LINE, MAX_HEADERS>
        {
            $($async)? fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
                if buf.is_empty() || !self.core.in_body {
                    return Ok(0);
                }
                if self.core.chunk_rest().is_empty() {
                    self.fill()$(.$await)??;
                }
                Ok(self.core.copy_out(buf))
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
