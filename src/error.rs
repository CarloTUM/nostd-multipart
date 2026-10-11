use core::fmt;

/// Every failure the crate can report. See `DESIGN.md` §12.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Error {
    /// The boundary is not 1–70 RFC 2046 `bchars` ending in a non-space byte, the
    /// `Content-Type` carries no usable `boundary` parameter, or a
    /// `boundary_from_random` array has an invalid size (rule 1).
    Boundary,
    /// A part header line (including its CRLF) is longer than `LINE` (rule 5.1).
    HeaderTooLong,
    /// A part has more than `MAX_HEADERS` header lines (rule 5.4).
    TooManyHeaders,
    /// A header line is malformed: bare LF, missing colon, empty or non-token name,
    /// obs-fold, CR inside the line (rule 5); or an encoder content type containing
    /// CR/LF (§11).
    Header,
    /// The input ended before the close delimiter (rule 7).
    UnexpectedEof,
    /// A `Content-Disposition` value does not follow RFC 2183 syntax (rule 8).
    Disposition,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Error::Boundary => "invalid multipart boundary",
            Error::HeaderTooLong => "part header line exceeds LINE",
            Error::TooManyHeaders => "part has more than MAX_HEADERS headers",
            Error::Header => "malformed part header line",
            Error::UnexpectedEof => "input ended before the close delimiter",
            Error::Disposition => "malformed Content-Disposition value",
        })
    }
}

impl core::error::Error for Error {}
