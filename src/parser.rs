//! The streaming parser (DESIGN.md §2–7, §9–10).

use crate::chars::{is_ows, is_token, is_valid_boundary, ows_trimmed_range, param, trim_ows};
use crate::Error;

/// Maximum number of SP/HTAB transport-padding bytes accepted between the boundary
/// and the CRLF of a part delimiter (rule 3). More padding means "not a delimiter".
pub const MAX_PADDING: usize = 8;

/// `min_buffer()` minus the boundary length: `"\r\n--"` + padding + `"\r\n"`.
const LOOKAHEAD_EXTRA: usize = 4 + MAX_PADDING + 2;

/// One parse event. `'i` is the lifetime of the `input` slice passed to
/// [`Parser::parse`], `'p` the borrow of the parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event<'i, 'p> {
    /// A part delimiter was consumed; headers follow.
    PartStart,
    /// One header line of the current part. Both slices borrow the parser's line
    /// buffer and are valid until the next call.
    Header {
        /// The field name, an RFC 7230 token (case preserved).
        name: &'p [u8],
        /// The field value with surrounding SP/HTAB removed; may contain bytes
        /// ≥ 0x80.
        value: &'p [u8],
    },
    /// The empty line after the headers; body data follows.
    HeadersEnd,
    /// A non-empty sub-slice of `input` belonging to the current part body.
    Data(&'i [u8]),
    /// The body of the current part is complete.
    PartEnd,
    /// The close delimiter was consumed. Every later call returns this again.
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Preamble { at_start: bool },
    Headers,
    Body,
    NewPart,
    Finished,
    Failed(Error),
}

/// Streaming, zero-copy `multipart/form-data` parser.
///
/// `LINE` is the size of the only buffer the parser owns: a part header line
/// (including its CRLF) must fit into it. `MAX_HEADERS` bounds the number of
/// header lines per part and costs no memory.
///
/// `size_of::<Parser<LINE, _>>() == LINE + 5 * size_of::<usize>()` (asserted by a
/// test): the boundary slice, the line length, the header counter and the state.
#[derive(Debug, Clone)]
pub struct Parser<'b, const LINE: usize = 512, const MAX_HEADERS: usize = 16> {
    boundary: &'b [u8],
    state: State,
    line: [u8; LINE],
    line_len: usize,
    header_count: usize,
}

impl<'b, const LINE: usize, const MAX_HEADERS: usize> Parser<'b, LINE, MAX_HEADERS> {
    /// Creates a parser for `boundary` (rule 1), which is borrowed, not copied.
    ///
    /// # Errors
    /// [`Error::Boundary`] if `boundary` is not 1–70 RFC 2046 `bchars` ending in a
    /// non-space byte.
    pub fn new(boundary: &'b [u8]) -> Result<Self, Error> {
        if !is_valid_boundary(boundary) {
            return Err(Error::Boundary);
        }
        Ok(Self {
            boundary,
            state: State::Preamble { at_start: true },
            line: [0; LINE],
            line_len: 0,
            header_count: 0,
        })
    }

    /// Creates a parser from a `Content-Type` header value such as
    /// `multipart/form-data; boundary=----x` (rule 1). The boundary is borrowed
    /// from `content_type`, quoted or unquoted.
    ///
    /// # Errors
    /// [`Error::Boundary`] if the value is not a `multipart/*` media type with a
    /// valid `boundary` parameter.
    pub fn from_content_type(content_type: &'b [u8]) -> Result<Self, Error> {
        match boundary_param(content_type) {
            Some(b) => Self::new(b),
            None => Err(Error::Boundary),
        }
    }

    /// The boundary this parser was created with.
    #[must_use]
    pub const fn boundary(&self) -> &'b [u8] {
        self.boundary
    }

    /// The smallest `input.len()` that guarantees progress on every call
    /// (`boundary.len() + MAX_PADDING + 6`, see DESIGN.md §9).
    #[must_use]
    pub const fn min_buffer(&self) -> usize {
        self.boundary.len().saturating_add(LOOKAHEAD_EXTRA)
    }

    /// `true` once the close delimiter has been consumed. That is already the
    /// case when the last part's `PartEnd` is returned, one call before the
    /// trailing `Finished` event.
    #[must_use]
    pub const fn is_finished(&self) -> bool {
        matches!(self.state, State::Finished)
    }

    /// Consumes a prefix of `input` and yields at most one event.
    ///
    /// The parser may consume fewer bytes than given (a possible delimiter prefix
    /// at the end is held back): drop the first `consumed` bytes, append new ones
    /// and call again. `eof = true` means no more input will follow.
    ///
    /// Progress guarantee: if `input.len() >= self.min_buffer()` or `eof`, every
    /// call consumes ≥ 1 byte, returns an event, or returns an error. After an
    /// error every call returns the same error; after `Finished` every call
    /// returns `(input.len(), Some(Finished))`.
    ///
    /// # Errors
    /// See [`Error`]; the parser is unusable afterwards.
    pub fn parse<'i>(
        &mut self,
        input: &'i [u8],
        eof: bool,
    ) -> Result<(usize, Option<Event<'i, '_>>), Error> {
        let (consumed, ev) = self.step(input, eof)?;
        Ok((consumed, ev.map(|ev| self.materialize(input, ev))))
    }

    /// `parse` without the borrowed event: the event description `Ev` holds only
    /// indices, which lets adapters store it and build the `Event` later.
    pub(crate) fn step(&mut self, input: &[u8], eof: bool) -> Result<(usize, Option<Ev>), Error> {
        let step = match self.state {
            State::NewPart => {
                self.state = State::Headers;
                return Ok((0, Some(Ev::PartStart)));
            }
            State::Finished => return Ok((input.len(), Some(Ev::Finished))),
            State::Failed(e) => Err(e),
            _ if input.is_empty() && eof => Err(Error::UnexpectedEof),
            _ if input.is_empty() => Ok((0, None)),
            State::Preamble { at_start } => Ok(self.preamble(input, eof, at_start)),
            State::Headers => self.headers(input),
            State::Body => Ok(self.body(input, eof)),
        };
        step.inspect_err(|&e| self.state = State::Failed(e))
    }

    /// Turns an event description into the borrowed [`Event`]. `input` must be
    /// the slice that was passed to the `step` that produced `ev`.
    pub(crate) fn materialize<'i>(&self, input: &'i [u8], ev: Ev) -> Event<'i, '_> {
        match ev {
            Ev::PartStart => Event::PartStart,
            Ev::HeadersEnd => Event::HeadersEnd,
            Ev::PartEnd => Event::PartEnd,
            Ev::Finished => Event::Finished,
            Ev::Data { end } => Event::Data(input.get(..end).unwrap_or(input)),
            Ev::Header {
                name_end,
                value_start,
                value_end,
            } => Event::Header {
                name: self.line.get(..name_end).unwrap_or(&[]),
                value: self.line.get(value_start..value_end).unwrap_or(&[]),
            },
        }
    }

    /// §10 Preamble. `input` is non-empty.
    fn preamble(&mut self, input: &[u8], eof: bool, at_start: bool) -> (usize, Option<Ev>) {
        let (consumed, ev) = match scan(input, eof, at_start, self.boundary) {
            Found::Delim { at, len, close } => {
                let (state, ev) = if close {
                    (State::Finished, Ev::Finished)
                } else {
                    (State::Headers, Ev::PartStart)
                };
                self.state = state;
                return (at.saturating_add(len), Some(ev));
            }
            Found::HeldBack { at } => (at, None),
            Found::Nothing => (input.len(), None),
        };
        if consumed > 0 {
            self.state = State::Preamble { at_start: false };
        }
        (consumed, ev)
    }

    /// §10 Headers. `input` is non-empty; `line_len` and `header_count` are 0 at
    /// the start of every part.
    fn headers(&mut self, input: &[u8]) -> Result<(usize, Option<Ev>), Error> {
        let lf = input.iter().position(|&b| b == b'\n');
        let take = lf.map_or(input.len(), |i| i.saturating_add(1));
        let chunk = input.get(..take).unwrap_or(input);
        let new_len = self.line_len.saturating_add(chunk.len());
        if new_len > LINE {
            return Err(Error::HeaderTooLong); // 5.1
        }
        for (dst, &src) in self.line.iter_mut().skip(self.line_len).zip(chunk) {
            *dst = src;
        }
        self.line_len = new_len;
        if lf.is_none() {
            return Ok((take, None));
        }
        // A complete line is in `self.line[..new_len]`; it is consumed either way.
        self.line_len = 0;
        let line = self.line.get(..new_len).unwrap_or(&[]);
        let Some(content) = line.strip_suffix(b"\r\n") else {
            return Err(Error::Header); // 5.2 bare LF
        };
        if content.is_empty() {
            self.state = State::Body; // 5.3
            self.header_count = 0;
            return Ok((take, Some(Ev::HeadersEnd)));
        }
        self.header_count = self.header_count.saturating_add(1);
        if self.header_count > MAX_HEADERS {
            return Err(Error::TooManyHeaders); // 5.4
        }
        let Some(colon) = content.iter().position(|&b| b == b':') else {
            return Err(Error::Header); // 5.5
        };
        if !is_token(content.get(..colon).unwrap_or(&[])) {
            return Err(Error::Header); // 5.5
        }
        if content.contains(&b'\r') {
            return Err(Error::Header); // 5.6
        }
        let value_from = colon.saturating_add(1);
        let (start, end) = ows_trimmed_range(content.get(value_from..).unwrap_or(&[]));
        Ok((
            take,
            Some(Ev::Header {
                name_end: colon,
                value_start: value_from.saturating_add(start),
                value_end: value_from.saturating_add(end),
            }),
        ))
    }

    /// §10 Body. `input` is non-empty.
    fn body(&mut self, input: &[u8], eof: bool) -> (usize, Option<Ev>) {
        let end = match scan(input, eof, false, self.boundary) {
            Found::Delim { at: 0, len, close } => {
                self.state = if close {
                    State::Finished
                } else {
                    State::NewPart
                };
                return (len, Some(Ev::PartEnd));
            }
            Found::Delim { at, .. } | Found::HeldBack { at } => at,
            Found::Nothing => input.len(),
        };
        (end, (end > 0).then_some(Ev::Data { end }))
    }
}

/// Borrow-free description of an event; turned into an [`Event`] by
/// `Parser::materialize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ev {
    PartStart,
    Header {
        name_end: usize,
        value_start: usize,
        value_end: usize,
    },
    HeadersEnd,
    Data {
        end: usize,
    },
    PartEnd,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Found {
    /// A resolved delimiter starting at `at` with total length `len`.
    Delim { at: usize, len: usize, close: bool },
    /// A candidate at `at` that needs more bytes (never with `eof`).
    HeldBack { at: usize },
    /// No delimiter (`classify`: definitely not one here; `scan`: none anywhere).
    Nothing,
}

/// Rule 3: classifies `s` against `lead ++ boundary` followed by a part or close
/// suffix. `HeldBack` means undecidable with the bytes available.
fn classify(s: &[u8], lead: &[u8], boundary: &[u8]) -> Found {
    let full = lead.len().saturating_add(boundary.len());
    if !s
        .iter()
        .zip(lead.iter().chain(boundary))
        .all(|(a, b)| a == b)
    {
        return Found::Nothing;
    }
    let Some(tail) = s.get(full..) else {
        return Found::HeldBack { at: 0 };
    };
    let pad = tail
        .iter()
        .take(MAX_PADDING + 1)
        .take_while(|&&b| is_ows(b))
        .count();
    let (len, close) = match (
        tail.first(),
        tail.get(1),
        tail.get(pad),
        tail.get(pad.saturating_add(1)),
    ) {
        (Some(b'-'), Some(b'-'), ..) => (2, true),
        (None | Some(b'-'), None, ..) | (_, _, None | Some(b'\r'), None) if pad <= MAX_PADDING => {
            return Found::HeldBack { at: 0 }
        }
        (Some(b'-'), ..) => return Found::Nothing,
        (.., Some(b'\r'), Some(b'\n')) if pad <= MAX_PADDING => (pad.saturating_add(2), false),
        _ => return Found::Nothing,
    };
    Found::Delim {
        at: 0,
        len: full.saturating_add(len),
        close,
    }
}

/// §9: finds and resolves the first candidate delimiter in `input`. With
/// `allow_d0` the dash-boundary form is tried at offset 0 first.
fn scan(input: &[u8], eof: bool, allow_d0: bool, boundary: &[u8]) -> Found {
    let mut from = 0usize;
    let mut try_d0 = allow_d0 && input.first() == Some(&b'-');
    loop {
        let (at, lead): (usize, &[u8]) = if try_d0 {
            (0, b"--")
        } else {
            match input
                .get(from..)
                .and_then(|r| r.iter().position(|&b| b == b'\r'))
            {
                Some(off) => (from.saturating_add(off), b"\r\n--"),
                None => return Found::Nothing,
            }
        };
        try_d0 = false;
        match classify(input.get(at..).unwrap_or(&[]), lead, boundary) {
            Found::Delim { len, close, .. } => return Found::Delim { at, len, close },
            Found::HeldBack { .. } if !eof => return Found::HeldBack { at },
            _ => from = at.saturating_add(1),
        }
    }
}

/// Extracts the `boundary` parameter from a `multipart/*` media type (rule 1).
fn boundary_param(ct: &[u8]) -> Option<&[u8]> {
    let slash = ct.iter().position(|&b| b == b'/')?;
    let (mtype, rest) = ct.split_at_checked(slash)?;
    if !trim_ows(mtype).eq_ignore_ascii_case(b"multipart") {
        return None;
    }
    let rest = rest.get(1..)?;
    let semi = rest.iter().position(|&b| b == b';').unwrap_or(rest.len());
    let (subtype, mut params) = rest.split_at_checked(semi)?;
    if !is_token(trim_ows(subtype)) {
        return None;
    }
    loop {
        let (attr, value, quoted, rest) = param(params.strip_prefix(b";")?).ok()??;
        if !quoted && !is_token(value) {
            return None;
        }
        if attr.eq_ignore_ascii_case(b"boundary") {
            return Some(value);
        }
        params = rest;
    }
}
