//! Reference oracle: a deliberately simple *whole-buffer* multipart parser written
//! straight from DESIGN.md. It shares no code with the crate (it does not even
//! `use nostd_multipart`). Verification tests compare the streaming parser's
//! event stream against this one.
//!
//! This file is a test target of its own (so `cargo test` compiles it) and is also
//! `#[path]`-included by the other test binaries and by the fuzz targets.
#![allow(dead_code)]

/// Mirror of the crate's error variants (same names, independent type).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OError {
    Boundary,
    HeaderTooLong,
    TooManyHeaders,
    Header,
    UnexpectedEof,
}

/// Normalised event: `Data` holds the *whole* body of a part and is only emitted
/// when the body is non-empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OEvent {
    PartStart,
    Header(Vec<u8>, Vec<u8>),
    HeadersEnd,
    Data(Vec<u8>),
    PartEnd,
    Finished,
}

pub const MAX_PADDING: usize = 8; // DESIGN.md rule 3

fn is_bchar(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"'()+_,-./:=? ".contains(&b)
}

fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

/// Rule 1.
pub fn valid_boundary(b: &[u8]) -> bool {
    (1..=70).contains(&b.len()) && b.iter().all(|&c| is_bchar(c)) && b[b.len() - 1] != b' '
}

#[derive(Debug, PartialEq, Eq)]
enum Class {
    Part(usize),
    Close(usize),
    No,
}

/// Rules 2 and 3, applied to the whole remaining buffer `s` (so "ran out of bytes"
/// is simply "not a delimiter").
fn classify(s: &[u8], lead: &[u8], boundary: &[u8]) -> Class {
    let mut pattern = lead.to_vec();
    pattern.extend_from_slice(boundary);
    if !s.starts_with(&pattern) {
        return Class::No;
    }
    let tail = &s[pattern.len()..];
    if tail.starts_with(b"--") {
        return Class::Close(pattern.len() + 2);
    }
    let pad = tail
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .count();
    if pad > MAX_PADDING {
        return Class::No;
    }
    if tail[pad..].starts_with(b"\r\n") {
        return Class::Part(pattern.len() + pad + 2);
    }
    Class::No
}

/// Finds the first resolved delimiter at or after `from`. Returns `(at, len, is_close)`.
fn find_delimiter(
    body: &[u8],
    from: usize,
    boundary: &[u8],
    allow_d0: bool,
) -> Option<(usize, usize, bool)> {
    if allow_d0 && from == 0 {
        match classify(body, b"--", boundary) {
            Class::Part(n) => return Some((0, n, false)),
            Class::Close(n) => return Some((0, n, true)),
            Class::No => {}
        }
    }
    let mut c = from;
    while c < body.len() {
        if body[c] == b'\r' {
            match classify(&body[c..], b"\r\n--", boundary) {
                Class::Part(n) => return Some((c, n, false)),
                Class::Close(n) => return Some((c, n, true)),
                Class::No => {}
            }
        }
        c += 1;
    }
    None
}

/// Parses `body` completely. Returns the events produced before the end or the
/// error, exactly as the streaming parser must (DESIGN.md §10).
pub fn parse(
    boundary: &[u8],
    body: &[u8],
    line_max: usize,
    max_headers: usize,
) -> (Vec<OEvent>, Option<OError>) {
    let mut ev = Vec::new();
    if !valid_boundary(boundary) {
        return (ev, Some(OError::Boundary));
    }
    // Preamble (rule 4).
    let Some((at, len, close)) = find_delimiter(body, 0, boundary, true) else {
        return (ev, Some(OError::UnexpectedEof));
    };
    let mut pos = at + len;
    if close {
        ev.push(OEvent::Finished);
        return (ev, None);
    }
    ev.push(OEvent::PartStart);
    loop {
        // Headers (rule 5).
        let mut count = 0usize;
        loop {
            let Some(lf) = body[pos..].iter().position(|&b| b == b'\n') else {
                // No line end: the parser accumulates everything that is left.
                let e = if body.len() - pos > line_max {
                    OError::HeaderTooLong
                } else {
                    OError::UnexpectedEof
                };
                return (ev, Some(e));
            };
            let line = &body[pos..pos + lf + 1];
            if line.len() > line_max {
                return (ev, Some(OError::HeaderTooLong)); // 5.1
            }
            let Some(content) = line.strip_suffix(b"\r\n") else {
                return (ev, Some(OError::Header)); // 5.2 bare LF
            };
            pos += lf + 1;
            if content.is_empty() {
                ev.push(OEvent::HeadersEnd); // 5.3
                break;
            }
            count += 1;
            if count > max_headers {
                return (ev, Some(OError::TooManyHeaders)); // 5.4
            }
            let Some(colon) = content.iter().position(|&b| b == b':') else {
                return (ev, Some(OError::Header)); // 5.5
            };
            let name = &content[..colon];
            if name.is_empty() || !name.iter().all(|&b| is_tchar(b)) {
                return (ev, Some(OError::Header)); // 5.5
            }
            if content.contains(&b'\r') {
                return (ev, Some(OError::Header)); // 5.6
            }
            let mut value = &content[colon + 1..];
            while let [b' ' | b'\t', rest @ ..] = value {
                value = rest;
            }
            while let [rest @ .., b' ' | b'\t'] = value {
                value = rest;
            }
            ev.push(OEvent::Header(name.to_vec(), value.to_vec())); // 5.7
        }
        // Body (rule 6).
        let Some((at, len, close)) = find_delimiter(body, pos, boundary, false) else {
            // Rule 7 / §10 Body: everything left is data, then the input ends.
            if pos < body.len() {
                ev.push(OEvent::Data(body[pos..].to_vec()));
            }
            return (ev, Some(OError::UnexpectedEof));
        };
        if at > pos {
            ev.push(OEvent::Data(body[pos..at].to_vec()));
        }
        ev.push(OEvent::PartEnd);
        pos = at + len;
        if close {
            ev.push(OEvent::Finished);
            return (ev, None);
        }
        ev.push(OEvent::PartStart);
    }
}

#[test]
fn oracle_smoke() {
    let (ev, err) = parse(b"b", b"--b\r\nA: 1\r\n\r\nxy\r\n--b--", 64, 4);
    assert_eq!(err, None);
    assert_eq!(
        ev,
        vec![
            OEvent::PartStart,
            OEvent::Header(b"A".to_vec(), b"1".to_vec()),
            OEvent::HeadersEnd,
            OEvent::Data(b"xy".to_vec()),
            OEvent::PartEnd,
            OEvent::Finished
        ]
    );
}
