//! Encoder (DESIGN.md §11): writes boundary lines and part headers into a caller
//! buffer, resumably; part data is written by the caller.

use crate::chars::is_valid_boundary;
use crate::Error;

/// Description of one part for the [`Encoder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Part<'a> {
    /// The form field name (escaped per WHATWG: `"` → `%22`, CR → `%0D`, LF → `%0A`;
    /// additionally `\` → `\\`, see DESIGN.md §11).
    pub name: &'a [u8],
    /// Optional file name (escaped the same way).
    pub filename: Option<&'a [u8]>,
    /// Optional `Content-Type` value; must not contain CR or LF.
    pub content_type: Option<&'a [u8]>,
}

/// Streaming `multipart/form-data` encoder.
///
/// ```
/// use nostd_multipart::{Encoder, Part};
/// let mut enc = Encoder::new(b"xyz").unwrap();
/// let mut out = [0u8; 128];
/// let part = Part { name: b"file", filename: Some(b"a.bin"), content_type: Some(b"application/octet-stream") };
/// let (n, done) = enc.write_part_header(&part, &mut out).unwrap();
/// assert!(done);
/// assert_eq!(&out[..n], &b"--xyz\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n"[..]);
/// // ... write the file data yourself ...
/// let (n, done) = enc.write_close(&mut out);
/// assert!(done);
/// assert_eq!(&out[..n], b"\r\n--xyz--\r\n");
/// ```
#[derive(Debug, Clone)]
pub struct Encoder<'b> {
    boundary: &'b [u8],
    first: bool,
    pos: usize,
}

impl<'b> Encoder<'b> {
    /// Creates an encoder for `boundary` (rule 1).
    ///
    /// # Errors
    /// [`Error::Boundary`] if the boundary is invalid.
    pub fn new(boundary: &'b [u8]) -> Result<Self, Error> {
        if !is_valid_boundary(boundary) {
            return Err(Error::Boundary);
        }
        Ok(Self {
            boundary,
            first: true,
            pos: 0,
        })
    }

    /// The boundary.
    #[must_use]
    pub const fn boundary(&self) -> &'b [u8] {
        self.boundary
    }

    /// Writes the next bytes of a part's prefix (CRLF for parts after the first,
    /// `--boundary` CRLF, headers, empty line) into `out`. Returns the number of
    /// bytes written and whether the prefix is complete. If not complete, flush
    /// `out[..written]` and call again with the *same* `part`.
    ///
    /// # Errors
    /// [`Error::Header`] if `part.content_type` contains CR or LF.
    pub fn write_part_header(
        &mut self,
        part: &Part<'_>,
        out: &mut [u8],
    ) -> Result<(usize, bool), Error> {
        if part
            .content_type
            .is_some_and(|ct| ct.iter().any(|&b| b == b'\r' || b == b'\n'))
        {
            return Err(Error::Header);
        }
        let (written, done) = copy_stream(
            part_header_bytes(self.boundary(), self.first, part),
            &mut self.pos,
            out,
        );
        if done {
            self.first = false;
        }
        Ok((written, done))
    }

    /// Writes the next bytes of the close delimiter `CRLF --boundary-- CRLF`
    /// into `out`, resumably like [`Self::write_part_header`].
    pub fn write_close(&mut self, out: &mut [u8]) -> (usize, bool) {
        copy_stream(close_bytes(self.boundary()), &mut self.pos, out)
    }

    /// Exact length of the complete body for `parts` (each with its data length),
    /// as this encoder would produce it from the start. Saturates at `u64::MAX`.
    #[must_use]
    pub fn content_length(&self, parts: &[(Part<'_>, u64)]) -> u64 {
        let mut total = 0u64;
        let mut first = true;
        for (part, data_len) in parts {
            let header = count_u64(part_header_bytes(self.boundary(), first, part));
            total = total.saturating_add(header).saturating_add(*data_len);
            first = false;
        }
        total.saturating_add(count_u64(close_bytes(self.boundary())))
    }
}

fn count_u64(it: impl Iterator<Item = u8>) -> u64 {
    u64::try_from(it.count()).unwrap_or(u64::MAX)
}

/// Copies `stream[*pos..]` into `out`; returns `(written, done)` and advances
/// `*pos` (reset to 0 when done).
fn copy_stream(stream: impl Iterator<Item = u8>, pos: &mut usize, out: &mut [u8]) -> (usize, bool) {
    let mut it = stream.skip(*pos).peekable();
    let mut written = 0usize;
    for (slot, b) in out.iter_mut().zip(&mut it) {
        *slot = b;
        written = written.saturating_add(1);
    }
    let done = it.peek().is_none();
    *pos = if done { 0 } else { pos.saturating_add(written) };
    (written, done)
}

fn bytes(s: &[u8]) -> impl Iterator<Item = u8> + '_ {
    s.iter().copied()
}

/// WHATWG escaping of one byte of a name or filename, plus `\` → `\\` so the
/// value survives RFC 2183 quoted-pair decoding (DESIGN.md §11).
fn escape(b: u8) -> impl Iterator<Item = u8> {
    let (buf, len) = match b {
        b'"' => (*b"%22", 3),
        b'\r' => (*b"%0D", 3),
        b'\n' => (*b"%0A", 3),
        b'\\' => (*b"\\\\\0", 2),
        b => ([b, 0, 0], 1),
    };
    buf.into_iter().take(len)
}

/// The byte stream of a part's prefix (§11).
fn part_header_bytes<'a>(
    boundary: &'a [u8],
    first: bool,
    part: &Part<'a>,
) -> impl Iterator<Item = u8> + 'a {
    let lead: &'static [u8] = if first { b"" } else { b"\r\n" };
    bytes(lead)
        .chain(bytes(b"--"))
        .chain(bytes(boundary))
        .chain(bytes(b"\r\nContent-Disposition: form-data; name=\""))
        .chain(bytes(part.name).flat_map(escape))
        .chain(bytes(b"\""))
        .chain(part.filename.into_iter().flat_map(|f| {
            bytes(b"; filename=\"")
                .chain(bytes(f).flat_map(escape))
                .chain(bytes(b"\""))
        }))
        .chain(bytes(b"\r\n"))
        .chain(part.content_type.into_iter().flat_map(|ct| {
            bytes(b"Content-Type: ")
                .chain(bytes(ct))
                .chain(bytes(b"\r\n"))
        }))
        .chain(bytes(b"\r\n"))
}

/// The byte stream of the close delimiter line.
fn close_bytes(boundary: &[u8]) -> impl Iterator<Item = u8> + '_ {
    bytes(b"\r\n--")
        .chain(bytes(boundary))
        .chain(bytes(b"--\r\n"))
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Maps `N` random bytes (e.g. from a hardware RNG) to a valid boundary of `N`
/// bchars from `A–Z a–z 0–9 - _` (6 bits of entropy per byte).
///
/// # Errors
/// [`Error::Boundary`] if `N` is 0 or greater than 70.
pub fn boundary_from_random<const N: usize>(random: &[u8; N]) -> Result<[u8; N], Error> {
    let mut out = [0u8; N];
    fill_boundary(random, &mut out).map(|()| out)
}

/// Non-generic worker of [`boundary_from_random`]: one copy in the binary for
/// all `N`, and llvm-cov does not count an uncovered per-`N` instantiation.
fn fill_boundary(random: &[u8], out: &mut [u8]) -> Result<(), Error> {
    if random.is_empty() || random.len() > 70 {
        return Err(Error::Boundary);
    }
    for (o, &r) in out.iter_mut().zip(random) {
        *o = ALPHABET.get(usize::from(r & 63)).copied().unwrap_or(b'x');
    }
    Ok(())
}
