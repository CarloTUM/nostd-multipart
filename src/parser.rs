//! The streaming parser (DESIGN.md §2–7, §9–10).

use crate::chars::{eq_ignore_case, is_ows, is_tchar, is_valid_boundary, ows_trimmed_range, trim_ows};
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

    /// `true` once `Finished` has been returned.
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
        let _ = (input, eof);
        Err(Error::UnexpectedEof) // PHASE 2 STUB — replaced in phase 3
    }
}

/// Extracts the `boundary` parameter from a `multipart/*` media type (rule 1).
fn boundary_param(ct: &[u8]) -> Option<&[u8]> {
    let slash = ct.iter().position(|&b| b == b'/')?;
    let (mtype, rest) = ct.split_at_checked(slash)?;
    if !eq_ignore_case(trim_ows(mtype), b"multipart") {
        return None;
    }
    let rest = rest.get(1..)?;
    let semi = rest.iter().position(|&b| b == b';').unwrap_or(rest.len());
    let (subtype, mut params) = rest.split_at_checked(semi)?;
    if !is_token(trim_ows(subtype)) {
        return None;
    }
    while let Some(after_semi) = params.strip_prefix(b";") {
        let s = trim_ows(after_semi);
        let eq = s.iter().position(|&b| b == b'=')?;
        let (attr, after_eq) = s.split_at_checked(eq)?;
        let attr = trim_ows(attr);
        if !is_token(attr) {
            return None;
        }
        let v = trim_ows(after_eq.get(1..)?);
        let (value, rest) = if let Some(inner) = v.strip_prefix(b"\"") {
            let close = quoted_string_end(inner)?;
            let (value, rest) = inner.split_at_checked(close)?;
            (value, trim_ows(rest.get(1..)?))
        } else {
            let end = v.iter().position(|&b| b == b';').unwrap_or(v.len());
            let (value, rest) = v.split_at_checked(end)?;
            let value = trim_ows(value);
            if !is_token(value) {
                return None;
            }
            (value, rest)
        };
        if eq_ignore_case(attr, b"boundary") {
            return Some(value);
        }
        params = rest;
    }
    None
}

/// Index of the closing `"` of a quoted-string whose opening quote was removed,
/// honouring `\x` quoted-pairs.
pub(crate) fn quoted_string_end(s: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    let mut escaped = false;
    for &b in s {
        if escaped {
            escaped = false;
        } else if b == b'\\' {
            escaped = true;
        } else if b == b'"' {
            return Some(i);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Non-empty sequence of RFC 7230 `tchar`.
pub(crate) fn is_token(s: &[u8]) -> bool {
    !s.is_empty() && s.iter().all(|&b| is_tchar(b))
}

#[allow(dead_code)]
fn unused_in_stub() -> (fn(u8) -> bool, fn(&[u8]) -> (usize, usize)) {
    (is_ows, ows_trimmed_range)
}
