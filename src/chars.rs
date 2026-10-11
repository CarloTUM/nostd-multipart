//! Byte classes from the RFCs (DESIGN.md rules 1 and 5).

/// RFC 2046 `bchars`.
pub(crate) const fn is_bchar(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'\'' | b'(' | b')' | b'+' | b'_' | b',' | b'-' | b'.' | b'/' | b':' | b'=' | b'?' | b' '
        )
}

/// RFC 7230 `tchar`.
pub(crate) const fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

/// SP or HTAB.
pub(crate) const fn is_ows(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// Rule 1: 1–70 bchars, last byte not SP.
pub(crate) fn is_valid_boundary(b: &[u8]) -> bool {
    !b.is_empty()
        && b.len() <= 70
        && b.iter().all(|&c| is_bchar(c))
        && b.last() != Some(&b' ')
}

/// `s` without leading and trailing SP/HTAB.
pub(crate) fn trim_ows(mut s: &[u8]) -> &[u8] {
    while let Some((&first, rest)) = s.split_first() {
        if is_ows(first) {
            s = rest;
        } else {
            break;
        }
    }
    while let Some((&last, rest)) = s.split_last() {
        if is_ows(last) {
            s = rest;
        } else {
            break;
        }
    }
    s
}

/// `a` and `b` are equal ignoring ASCII case.
pub(crate) fn eq_ignore_case(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

/// `(start, end)` of `s` without leading and trailing SP/HTAB (`s[start..end]`).
pub(crate) fn ows_trimmed_range(s: &[u8]) -> (usize, usize) {
    let start = s.iter().take_while(|&&b| is_ows(b)).count();
    let trailing = s.iter().rev().take_while(|&&b| is_ows(b)).count();
    let end = s.len().saturating_sub(trailing).max(start);
    (start, end)
}
