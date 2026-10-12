//! Byte classes from the RFCs (DESIGN.md rules 1 and 5).

use crate::Error;

// `matches!`, not `b"...".contains(&b)`: the latter is a memchr loop longer
// than the Kani harnesses' unwind bound.

/// RFC 2046 `bchars`.
const fn is_bchar(b: u8) -> bool {
    #[rustfmt::skip]
    let special = matches!(b, b'\'' | b'(' | b')' | b'+' | b'_' | b',' | b'-' | b'.' | b'/' | b':' | b'=' | b'?' | b' ');
    b.is_ascii_alphanumeric() || special
}

/// RFC 7230 `tchar`.
const fn is_tchar(b: u8) -> bool {
    #[rustfmt::skip]
    let special = matches!(b, b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~');
    b.is_ascii_alphanumeric() || special
}

/// SP or HTAB.
pub(crate) const fn is_ows(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// Rule 1: 1–70 bchars, last byte not SP.
pub(crate) fn is_valid_boundary(b: &[u8]) -> bool {
    b.len() <= 70 && b.iter().all(|&c| is_bchar(c)) && !matches!(b.last(), None | Some(b' '))
}

/// Non-empty sequence of RFC 7230 `tchar`.
pub(crate) fn is_token(s: &[u8]) -> bool {
    !s.is_empty() && s.iter().all(|&b| is_tchar(b))
}

/// `(start, end)` of `s` without leading and trailing SP/HTAB (`s[start..end]`).
pub(crate) fn ows_trimmed_range(s: &[u8]) -> (usize, usize) {
    let start = s.iter().take_while(|&&b| is_ows(b)).count();
    let trailing = s.iter().rev().take_while(|&&b| is_ows(b)).count();
    (start, s.len().saturating_sub(trailing).max(start))
}

/// `s` without leading and trailing SP/HTAB. (Plain loops: cheaper for Kani than
/// reusing `ows_trimmed_range`, and `content_disposition` calls this a lot.)
pub(crate) fn trim_ows(mut s: &[u8]) -> &[u8] {
    while let [b' ' | b'\t', rest @ ..] = s {
        s = rest;
    }
    while let [rest @ .., b' ' | b'\t'] = s {
        s = rest;
    }
    s
}

/// `(attr, value, quoted, rest)` of one parameter; `rest` is empty or starts at
/// the next `;`. A quoted value is returned without its quotes, undecoded.
type Param<'a> = (&'a [u8], &'a [u8], bool, &'a [u8]);

/// The parameter at the start of `s` (the text after a `;`); `None` if blank.
pub(crate) fn param(s: &[u8]) -> Result<Option<Param<'_>>, Error> {
    let s = trim_ows(s);
    if s.is_empty() {
        return Ok(None);
    }
    let eq = s
        .iter()
        .position(|&b| b == b'=')
        .ok_or(Error::Disposition)?;
    let (attr, v) = s.split_at_checked(eq).ok_or(Error::Disposition)?;
    let attr = trim_ows(attr);
    if !is_token(attr) {
        return Err(Error::Disposition);
    }
    let v = trim_ows(v.get(1..).unwrap_or(&[]));
    if let Some(inner) = v.strip_prefix(b"\"") {
        let close = quoted_string_end(inner).ok_or(Error::Disposition)?;
        let (value, tail) = inner.split_at_checked(close).ok_or(Error::Disposition)?;
        let tail = trim_ows(tail.get(1..).unwrap_or(&[]));
        if !tail.is_empty() && !tail.starts_with(b";") {
            return Err(Error::Disposition);
        }
        return Ok(Some((attr, value, true, tail)));
    }
    let end = v.iter().position(|&b| b == b';').unwrap_or(v.len());
    let (value, tail) = v.split_at_checked(end).ok_or(Error::Disposition)?;
    Ok(Some((attr, trim_ows(value), false, tail)))
}

/// Index of the closing `"` of a quoted-string whose opening quote was removed,
/// honouring `\x` quoted-pairs.
fn quoted_string_end(s: &[u8]) -> Option<usize> {
    let mut escaped = false;
    s.iter().position(|&b| {
        let close = !escaped && b == b'"';
        escaped = !escaped && b == b'\\';
        close
    })
}
