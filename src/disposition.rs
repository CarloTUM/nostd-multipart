//! `Content-Disposition` helper (DESIGN.md rule 8). Pure functions, no state.

use crate::chars::{eq_ignore_case, trim_ows};
use crate::parser::{is_token, quoted_string_end};
use crate::Error;

/// A parsed `Content-Disposition` header value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentDisposition<'a> {
    disposition_type: &'a [u8],
    name: Option<ParamValue<'a>>,
    filename: Option<ParamValue<'a>>,
    filename_star: Option<&'a [u8]>,
}

impl<'a> ContentDisposition<'a> {
    /// The disposition type token, e.g. `form-data`, as sent (case preserved).
    #[must_use]
    pub const fn disposition_type(&self) -> &'a [u8] {
        self.disposition_type
    }

    /// `true` if the disposition type is `form-data` (case-insensitive).
    #[must_use]
    pub fn is_form_data(&self) -> bool {
        eq_ignore_case(self.disposition_type, b"form-data")
    }

    /// The `name` parameter, if present (first occurrence).
    #[must_use]
    pub const fn name(&self) -> Option<ParamValue<'a>> {
        self.name
    }

    /// The `filename` parameter, if present (first occurrence).
    #[must_use]
    pub const fn filename(&self) -> Option<ParamValue<'a>> {
        self.filename
    }

    /// The raw RFC 5987 `filename*` ext-value (e.g. `utf-8''%E2%82%AC.txt`), not
    /// decoded. Browsers do not send it for form uploads.
    #[must_use]
    pub const fn filename_star(&self) -> Option<&'a [u8]> {
        self.filename_star
    }
}

/// A parameter value: the raw bytes plus lazy decoding of RFC 822 quoted-pairs
/// (`\x` → `x`) and the WHATWG escapes `%22`, `%0D`, `%0A` (→ `"`, CR, LF). Other
/// `%` sequences are left as they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamValue<'a> {
    raw: &'a [u8],
}

impl<'a> ParamValue<'a> {
    /// The undecoded bytes (between the quotes for a quoted-string).
    #[must_use]
    pub const fn raw(&self) -> &'a [u8] {
        self.raw
    }

    /// Iterator over the decoded bytes.
    #[must_use]
    pub const fn bytes(&self) -> Decoded<'a> {
        Decoded { rest: self.raw }
    }

    /// Number of decoded bytes.
    #[must_use]
    pub fn decoded_len(&self) -> usize {
        self.bytes().count()
    }

    /// Writes the decoded bytes into `out`; `None` if `out` is too small.
    pub fn decode_into<'o>(&self, out: &'o mut [u8]) -> Option<&'o [u8]> {
        let mut n = 0usize;
        let mut slots = out.iter_mut();
        for b in self.bytes() {
            *slots.next()? = b;
            n = n.saturating_add(1);
        }
        out.get(..n)
    }

    /// `true` if the decoded bytes equal `other`.
    #[must_use]
    pub fn eq_bytes(&self, other: &[u8]) -> bool {
        self.bytes().eq(other.iter().copied())
    }
}

impl PartialEq<[u8]> for ParamValue<'_> {
    fn eq(&self, other: &[u8]) -> bool {
        ParamValue::eq_bytes(self, other)
    }
}

impl PartialEq<&[u8]> for ParamValue<'_> {
    fn eq(&self, other: &&[u8]) -> bool {
        ParamValue::eq_bytes(self, other)
    }
}

impl PartialEq<str> for ParamValue<'_> {
    fn eq(&self, other: &str) -> bool {
        ParamValue::eq_bytes(self, other.as_bytes())
    }
}

impl PartialEq<&str> for ParamValue<'_> {
    fn eq(&self, other: &&str) -> bool {
        ParamValue::eq_bytes(self, other.as_bytes())
    }
}

/// Iterator over the decoded bytes of a [`ParamValue`].
#[derive(Debug, Clone)]
pub struct Decoded<'a> {
    rest: &'a [u8],
}

impl Iterator for Decoded<'_> {
    type Item = u8;

    fn next(&mut self) -> Option<u8> {
        let (&b, rest) = self.rest.split_first()?;
        if b == b'\\' {
            if let Some((&escaped, after)) = rest.split_first() {
                self.rest = after;
                return Some(escaped);
            }
        } else if b == b'%' {
            let decoded = match rest.get(..2) {
                Some(b"22") => Some(b'"'),
                Some(b"0D" | b"0d") => Some(b'\r'),
                Some(b"0A" | b"0a") => Some(b'\n'),
                _ => None,
            };
            if let Some(d) = decoded {
                self.rest = rest.get(2..).unwrap_or(&[]);
                return Some(d);
            }
        }
        self.rest = rest;
        Some(b)
    }
}

/// Parses a `Content-Disposition` header value (rule 8).
///
/// Lenient where browsers are: whitespace around `;` and `=` is ignored, an
/// unquoted value extends to the next `;`, a trailing `;` is tolerated, unknown
/// parameters are skipped.
///
/// # Errors
/// [`Error::Disposition`] for an empty or non-token disposition type, a parameter
/// without `=`, an empty or non-token attribute name, an unterminated
/// quoted-string, or bytes other than `;` after a quoted-string.
pub fn content_disposition(value: &[u8]) -> Result<ContentDisposition<'_>, Error> {
    let value = trim_ows(value);
    let end = value.iter().position(|&b| b == b';').unwrap_or(value.len());
    let (dtype, mut rest) = value.split_at_checked(end).ok_or(Error::Disposition)?;
    let dtype = trim_ows(dtype);
    if !is_token(dtype) {
        return Err(Error::Disposition);
    }
    let mut cd = ContentDisposition {
        disposition_type: dtype,
        name: None,
        filename: None,
        filename_star: None,
    };
    while let Some(after_semi) = rest.strip_prefix(b";") {
        let s = trim_ows(after_semi);
        if s.is_empty() {
            break;
        }
        let eq = s.iter().position(|&b| b == b'=').ok_or(Error::Disposition)?;
        let (attr, after_eq) = s.split_at_checked(eq).ok_or(Error::Disposition)?;
        let attr = trim_ows(attr);
        if !is_token(attr) {
            return Err(Error::Disposition);
        }
        let v = trim_ows(after_eq.get(1..).unwrap_or(&[]));
        let (raw, tail) = if let Some(inner) = v.strip_prefix(b"\"") {
            let close = quoted_string_end(inner).ok_or(Error::Disposition)?;
            let (raw, tail) = inner.split_at_checked(close).ok_or(Error::Disposition)?;
            let tail = trim_ows(tail.get(1..).unwrap_or(&[]));
            if !tail.is_empty() && !tail.starts_with(b";") {
                return Err(Error::Disposition);
            }
            (raw, tail)
        } else {
            let end = v.iter().position(|&b| b == b';').unwrap_or(v.len());
            let (raw, tail) = v.split_at_checked(end).ok_or(Error::Disposition)?;
            (trim_ows(raw), tail)
        };
        if eq_ignore_case(attr, b"name") {
            cd.name.get_or_insert(ParamValue { raw });
        } else if eq_ignore_case(attr, b"filename") {
            cd.filename.get_or_insert(ParamValue { raw });
        } else if eq_ignore_case(attr, b"filename*") {
            cd.filename_star.get_or_insert(raw);
        }
        rest = tail;
    }
    Ok(cd)
}
