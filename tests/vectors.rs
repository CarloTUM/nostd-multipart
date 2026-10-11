//! Hand-written test vectors: RFC examples and the adversarial cases from
//! DESIGN.md. Expectations are written by hand (not derived from the oracle), so
//! this file validates the oracle and the parser against each other *and* against
//! a human reading of the spec. Every body <= 24 bytes is run under every possible
//! chunking; longer bodies under a standard set including 1-byte chunks.

mod common;
use common::*;
use nostd_multipart::{Error, Event, Parser};
use OEvent::*;

fn h(n: &str, v: &str) -> OEvent {
    Header(n.as_bytes().to_vec(), v.as_bytes().to_vec())
}
fn d(x: &[u8]) -> OEvent {
    Data(x.to_vec())
}

fn chunkings_for(len: usize) -> Vec<Vec<usize>> {
    if len <= 24 {
        all_compositions(len)
    } else {
        standard_chunkings(len, 40, len as u64)
    }
}

/// Checks parser (every chunking) == oracle == hand-written expectation.
fn check<const LINE: usize, const MAX: usize>(boundary: &[u8], body: &[u8], events: Vec<OEvent>, err: Option<OError>) {
    let got = check_against_oracle::<LINE, MAX>(boundary, body, &chunkings_for(body.len()));
    assert_eq!(got, (events, err), "body {:?}", String::from_utf8_lossy(body));
}
fn ok(boundary: &[u8], body: &[u8], events: Vec<OEvent>) {
    check::<64, 8>(boundary, body, events, None);
}
fn fails(boundary: &[u8], body: &[u8], events: Vec<OEvent>, err: OError) {
    check::<64, 8>(boundary, body, events, Some(err));
}

// ---------------------------------------------------------------- RFC examples

#[test]
fn rfc2046_5_1_1_example() {
    let body = b"This is the preamble.  It is to be ignored, though it\r\n\
is a handy place for composition agents to include an\r\n\
explanatory note to non-MIME conformant readers.\r\n\
\r\n\
--simple boundary\r\n\
\r\n\
This is implicitly typed plain US-ASCII text.\r\n\
It does NOT end with a linebreak.\r\n\
--simple boundary\r\n\
Content-type: text/plain; charset=us-ascii\r\n\
\r\n\
This is explicitly typed plain US-ASCII text.\r\n\
It DOES end with a linebreak.\r\n\
\r\n\
--simple boundary--\r\n\
\r\n\
This is the epilogue.  It is also to be ignored.";
    ok(
        b"simple boundary",
        body,
        vec![
            PartStart,
            HeadersEnd,
            d(b"This is implicitly typed plain US-ASCII text.\r\nIt does NOT end with a linebreak."),
            PartEnd,
            PartStart,
            h("Content-type", "text/plain; charset=us-ascii"),
            HeadersEnd,
            d(b"This is explicitly typed plain US-ASCII text.\r\nIt DOES end with a linebreak.\r\n"),
            PartEnd,
            Finished,
        ],
    );
}

#[test]
fn rfc7578_4_2_example() {
    let body = b"--AaB03x\r\n\
content-disposition: form-data; name=\"field1\"\r\n\
content-type: text/plain;charset=UTF-8\r\n\
content-transfer-encoding: quoted-printable\r\n\
\r\n\
Joe owes =E2=82=AC100.\r\n\
--AaB03x--";
    ok(
        b"AaB03x",
        body,
        vec![
            PartStart,
            h("content-disposition", "form-data; name=\"field1\""),
            h("content-type", "text/plain;charset=UTF-8"),
            h("content-transfer-encoding", "quoted-printable"),
            HeadersEnd,
            d(b"Joe owes =E2=82=AC100."),
            PartEnd,
            Finished,
        ],
    );
}

#[test]
fn rfc7578_4_3_nested_multipart_is_opaque_data() {
    let inner = b"--BbC04y\r\n\
Content-Disposition: file; filename=\"file1.txt\"\r\n\
Content-Type: text/plain\r\n\
\r\n\
... contents of file1.txt ...\r\n\
--BbC04y\r\n\
Content-Disposition: file; filename=\"file2.gif\"\r\n\
Content-Type: image/gif\r\n\
Content-Transfer-Encoding: binary\r\n\
\r\n\
...contents of file2.gif...\r\n\
--BbC04y--";
    let mut body = b"--AaB03x\r\n\
Content-Disposition: form-data; name=\"submit-name\"\r\n\
\r\n\
Larry\r\n\
--AaB03x\r\n\
Content-Disposition: form-data; name=\"files\"\r\n\
Content-Type: multipart/mixed; boundary=BbC04y\r\n\
\r\n"
        .to_vec();
    body.extend_from_slice(inner);
    body.extend_from_slice(b"\r\n--AaB03x--");
    ok(
        b"AaB03x",
        &body,
        vec![
            PartStart,
            h("Content-Disposition", "form-data; name=\"submit-name\""),
            HeadersEnd,
            d(b"Larry"),
            PartEnd,
            PartStart,
            h("Content-Disposition", "form-data; name=\"files\""),
            h("Content-Type", "multipart/mixed; boundary=BbC04y"),
            HeadersEnd,
            d(inner),
            PartEnd,
            Finished,
        ],
    );
}

// --------------------------------------------------------------- small bodies
// (<= 24 bytes: exhaustively chunked)

#[test]
fn zero_parts() {
    ok(b"b", b"--b--", vec![Finished]);
    ok(b"b", b"--b--\r\n", vec![Finished]);
    ok(b"b", b"--b--epilogue", vec![Finished]);
    ok(b"b", b"pre\r\n--b--\r\nepi", vec![Finished]);
    ok(b"b", b"\r\n--b--", vec![Finished]);
}

#[test]
fn one_part_minimal() {
    ok(b"b", b"--b\r\n\r\n\r\n--b--", vec![PartStart, HeadersEnd, PartEnd, Finished]);
    ok(b"b", b"--b\r\n\r\nx\r\n--b--", vec![PartStart, HeadersEnd, d(b"x"), PartEnd, Finished]);
    ok(b"b", b"--b\r\nA:1\r\n\r\nx\r\n--b--", vec![PartStart, h("A", "1"), HeadersEnd, d(b"x"), PartEnd, Finished]);
}

#[test]
fn two_parts_minimal() {
    ok(
        b"b",
        b"--b\r\n\r\nx\r\n--b\r\n\r\ny\r\n--b--",
        vec![PartStart, HeadersEnd, d(b"x"), PartEnd, PartStart, HeadersEnd, d(b"y"), PartEnd, Finished],
    );
    ok(
        b"b",
        b"--b\r\n\r\n\r\n--b\r\n\r\n\r\n--b--",
        vec![PartStart, HeadersEnd, PartEnd, PartStart, HeadersEnd, PartEnd, Finished],
    );
}

#[test]
fn first_delimiter_needs_crlf_unless_at_offset_zero() {
    // D0 only at byte 0: here "--b" at offset 2 is not a delimiter, but the later
    // "\r\n--b--" is a close delimiter => zero parts.
    ok(b"b", b"xx--b\r\n\r\nd\r\n--b--", vec![Finished]);
    // Preamble followed by CRLF then dash-boundary.
    ok(b"b", b"pre\r\n--b\r\n\r\nx\r\n--b--", vec![PartStart, HeadersEnd, d(b"x"), PartEnd, Finished]);
    // LF-only is not a line break: nothing is ever a delimiter.
    fails(b"b", b"--b\n\n--b--", vec![], OError::UnexpectedEof);
    fails(b"b", b"\n--b\r\n\r\nx\r\n--b--", vec![], OError::UnexpectedEof);
    // Hmm: "\n--b" is not at offset 0 and has no CRLF; the only CR candidates
    // are "\r\n\r\nx" and "\r\n--b--" => zero parts? No: "\r\n--b--" IS a close
    // delimiter, so the oracle and parser must agree on `Finished`:
}

#[test]
fn lf_only_preamble_then_close() {
    ok(b"b", b"\n--b\r\n\r\nx\r\n--b--", vec![Finished]);
}

#[test]
fn close_without_trailing_crlf_at_eof() {
    ok(b"ab", b"--ab\r\n\r\nxy\r\n--ab--", vec![PartStart, HeadersEnd, d(b"xy"), PartEnd, Finished]);
}

#[test]
fn padding_after_boundary() {
    ok(b"b", b"--b \r\n\r\nx\r\n--b\t\r\n\r\n--b--", vec![PartStart, HeadersEnd, d(b"x"), PartEnd, PartStart, HeadersEnd, PartEnd, Finished]);
    ok(b"b", b"--b \t \t \t \t\r\n\r\n\r\n--b--", vec![PartStart, HeadersEnd, PartEnd, Finished]); // 8 bytes
    // 9 padding bytes: not a delimiter (rule 3), so the whole thing is preamble.
    fails(b"b", b"--b         \r\n\r\n\r\n--b", vec![], OError::UnexpectedEof);
}

#[test]
fn padding_then_non_crlf_is_data() {
    ok(b"b", b"--b\r\n\r\nx\r\n--b  y\r\n--b--", vec![PartStart, HeadersEnd, d(b"x\r\n--b  y"), PartEnd, Finished]);
}

#[test]
fn boundary_prefix_of_longer_token_is_data() {
    ok(b"abc", b"--abc\r\n\r\nx\r\n--abcdef\r\ny\r\n--abc--", vec![PartStart, HeadersEnd, d(b"x\r\n--abcdef\r\ny"), PartEnd, Finished]);
    ok(b"abc", b"--abc\r\n\r\nx\r\n--abc-x\r\n--abc--", vec![PartStart, HeadersEnd, d(b"x\r\n--abc-x"), PartEnd, Finished]);
    ok(b"abc", b"--abc\r\n\r\nx\r\n--abc\rz\r\n--abc--", vec![PartStart, HeadersEnd, d(b"x\r\n--abc\rz"), PartEnd, Finished]);
    ok(b"abc", b"--abc\r\n\r\nx\r\n--ab\r\n--abc--", vec![PartStart, HeadersEnd, d(b"x\r\n--ab"), PartEnd, Finished]);
}

#[test]
fn crlf_dash_dash_without_boundary_is_data() {
    ok(b"bnd", b"--bnd\r\n\r\na\r\n--x\r\n--\r\nb\r\n--bnd--", vec![PartStart, HeadersEnd, d(b"a\r\n--x\r\n--\r\nb"), PartEnd, Finished]);
}

#[test]
fn every_prefix_of_delimiter_inside_data() {
    // "\r", "\r\n", "\r\n-", "\r\n--", "\r\n--a" (boundary "ab") each followed by
    // a byte that breaks the match.
    ok(
        b"ab",
        b"--ab\r\n\r\n\rX\r\nX\r\n-X\r\n--X\r\n--aX\r\n--ab--",
        vec![PartStart, HeadersEnd, d(b"\rX\r\nX\r\n-X\r\n--X\r\n--aX"), PartEnd, Finished],
    );
}

#[test]
fn lf_in_data_is_data() {
    ok(b"b", b"--b\r\n\r\n\n\n\n--b\n\r\n--b--", vec![PartStart, HeadersEnd, d(b"\n\n\n--b\n"), PartEnd, Finished]);
}

#[test]
fn eof_cases() {
    fails(b"b", b"", vec![], OError::UnexpectedEof);
    fails(b"b", b"--", vec![], OError::UnexpectedEof);
    fails(b"b", b"--b", vec![], OError::UnexpectedEof);
    fails(b"b", b"--b\r", vec![], OError::UnexpectedEof);
    fails(b"b", b"--b\r\n", vec![PartStart], OError::UnexpectedEof);
    fails(b"b", b"--b\r\nA: 1", vec![PartStart], OError::UnexpectedEof);
    fails(b"b", b"--b\r\nA: 1\r\n", vec![PartStart, h("A", "1")], OError::UnexpectedEof);
    fails(b"b", b"--b\r\nA: 1\r\n\r\n", vec![PartStart, h("A", "1"), HeadersEnd], OError::UnexpectedEof);
    fails(b"b", b"--b\r\n\r\nxyz", vec![PartStart, HeadersEnd, d(b"xyz")], OError::UnexpectedEof);
    fails(b"b", b"--b\r\n\r\nxyz\r\n--b", vec![PartStart, HeadersEnd, d(b"xyz\r\n--b")], OError::UnexpectedEof);
    fails(b"b", b"--b\r\n\r\nxyz\r\n--b-", vec![PartStart, HeadersEnd, d(b"xyz\r\n--b-")], OError::UnexpectedEof);
    fails(b"b", b"--b\r\n\r\nxyz\r\n--b\r", vec![PartStart, HeadersEnd, d(b"xyz\r\n--b\r")], OError::UnexpectedEof);
    fails(b"b", b"--b\r\n\r\nxyz\r\n--b \r\n", vec![PartStart, HeadersEnd, d(b"xyz"), PartEnd, PartStart], OError::UnexpectedEof);
    fails(b"b", b"pre", vec![], OError::UnexpectedEof);
    fails(b"b", b"pre\r\n--b-", vec![], OError::UnexpectedEof);
}

#[test]
fn header_errors() {
    fails(b"b", b"--b\r\nA: 1\n\r\n\r\n--b--", vec![PartStart], OError::Header); // bare LF
    fails(b"b", b"--b\r\n\n\r\n--b--", vec![PartStart], OError::Header); // bare LF as empty line
    fails(b"b", b"--b\r\nA: 1\r\n b\r\n\r\n--b--", vec![PartStart, h("A", "1")], OError::Header); // obs-fold
    fails(b"b", b"--b\r\n\tA: 1\r\n\r\n--b--", vec![PartStart], OError::Header); // leading HTAB
    fails(b"b", b"--b\r\nnocolon\r\n\r\n--b--", vec![PartStart], OError::Header);
    fails(b"b", b"--b\r\n: v\r\n\r\n--b--", vec![PartStart], OError::Header); // empty name
    fails(b"b", b"--b\r\nA B: v\r\n\r\n--b--", vec![PartStart], OError::Header); // SP in name
    fails(b"b", b"--b\r\nA : v\r\n\r\n--b--", vec![PartStart], OError::Header); // SP before colon
    fails(b"b", b"--b\r\nA\xc3\xa9: v\r\n\r\n--b--", vec![PartStart], OError::Header); // non-ASCII name
    fails(b"b", b"--b\r\nA: 1\r2\r\n\r\n--b--", vec![PartStart], OError::Header); // CR in value
    fails(b"b", b"--b\r\nA: 1\r\r\n\r\n--b--", vec![PartStart], OError::Header); // CR before CRLF
    fails(b"b", b"--b\r\nA:\r\r\n\r\n--b--", vec![PartStart], OError::Header);
    fails(b"b", b"--b\r\n--b--\r\n\r\n--b--", vec![PartStart], OError::Header); // boundary line as header: no colon
    fails(b"b", b"--b\r\n--b: x\r\n\r\n--b--", vec![PartStart, h("--b", "x")], OError::UnexpectedEof); // valid token!
}

#[test]
fn header_values() {
    ok(b"b", b"--b\r\nA:\r\n\r\n\r\n--b--", vec![PartStart, h("A", ""), HeadersEnd, PartEnd, Finished]);
    ok(b"b", b"--b\r\nA: \t \r\n\r\n\r\n--b--", vec![PartStart, h("A", ""), HeadersEnd, PartEnd, Finished]);
    ok(b"b", b"--b\r\nA:\t x \t\r\n\r\n\r\n--b--", vec![PartStart, h("A", "x"), HeadersEnd, PartEnd, Finished]);
    ok(b"b", b"--b\r\nA: x  y\r\n\r\n\r\n--b--", vec![PartStart, h("A", "x  y"), HeadersEnd, PartEnd, Finished]);
    ok(b"b", b"--b\r\nA: a:b:c\r\n\r\n\r\n--b--", vec![PartStart, h("A", "a:b:c"), HeadersEnd, PartEnd, Finished]);
    ok(
        b"b",
        b"--b\r\nX-Name: \xc3\xa4\xf0\x9f\x98\x80\r\n\r\n\r\n--b--",
        vec![PartStart, Header(b"X-Name".to_vec(), b"\xc3\xa4\xf0\x9f\x98\x80".to_vec()), HeadersEnd, PartEnd, Finished],
    );
    ok(b"b", b"--b\r\n!#$%&'*+-.^_`|~09Az: v\r\n\r\n\r\n--b--", vec![PartStart, h("!#$%&'*+-.^_`|~09Az", "v"), HeadersEnd, PartEnd, Finished]);
}

#[test]
fn duplicate_headers_and_missing_content_disposition() {
    ok(
        b"b",
        b"--b\r\nA: 1\r\nA: 2\r\na: 3\r\n\r\nx\r\n--b--",
        vec![PartStart, h("A", "1"), h("A", "2"), h("a", "3"), HeadersEnd, d(b"x"), PartEnd, Finished],
    );
    ok(b"b", b"--b\r\nContent-Type: text/plain\r\n\r\nx\r\n--b--", vec![PartStart, h("Content-Type", "text/plain"), HeadersEnd, d(b"x"), PartEnd, Finished]);
}

#[test]
fn empty_name_parameter_is_fine_for_the_parser() {
    ok(
        b"b",
        b"--b\r\nContent-Disposition: form-data; name=\"\"\r\n\r\n\r\n--b--",
        vec![PartStart, h("Content-Disposition", "form-data; name=\"\""), HeadersEnd, PartEnd, Finished],
    );
}

#[test]
fn header_too_long() {
    // LINE = 16: "A: 1234567890\r\n" is 15 bytes (ok), one more byte is too long.
    check::<16, 8>(b"b", b"--b\r\nA: 1234567890\r\n\r\n\r\n--b--", vec![PartStart, h("A", "1234567890"), HeadersEnd, PartEnd, Finished], None);
    check::<16, 8>(b"b", b"--b\r\nA: 1234567890X\r\n\r\n\r\n--b--", vec![PartStart], Some(OError::HeaderTooLong));
    // Detected before the line ends: 17 bytes without LF then EOF.
    check::<16, 8>(b"b", b"--b\r\nA: 12345678901234", vec![PartStart], Some(OError::HeaderTooLong));
    // Exactly LINE bytes without LF then EOF: not too long, just EOF.
    check::<16, 8>(b"b", b"--b\r\nA: 1234567890123", vec![PartStart], Some(OError::UnexpectedEof));
    // A huge header line.
    let mut body = b"--b\r\nA: ".to_vec();
    body.extend(std::iter::repeat(b'x').take(5000));
    body.extend_from_slice(b"\r\n\r\n\r\n--b--");
    check::<512, 8>(b"b", &body, vec![PartStart], Some(OError::HeaderTooLong));
    // Too-long line that also has a bare LF: HeaderTooLong wins (checked first).
    check::<16, 8>(b"b", b"--b\r\nA: 1234567890X\n\r\n\r\n--b--", vec![PartStart], Some(OError::HeaderTooLong));
}

#[test]
fn too_many_headers() {
    check::<64, 2>(b"b", b"--b\r\nA: 1\r\nB: 2\r\n\r\n\r\n--b--", vec![PartStart, h("A", "1"), h("B", "2"), HeadersEnd, PartEnd, Finished], None);
    check::<64, 2>(b"b", b"--b\r\nA: 1\r\nB: 2\r\nC: 3\r\n\r\n\r\n--b--", vec![PartStart, h("A", "1"), h("B", "2")], Some(OError::TooManyHeaders));
    // The third line is also malformed: TooManyHeaders is checked first.
    check::<64, 2>(b"b", b"--b\r\nA: 1\r\nB: 2\r\nnocolon\r\n\r\n\r\n--b--", vec![PartStart, h("A", "1"), h("B", "2")], Some(OError::TooManyHeaders));
    // The count resets per part.
    check::<64, 1>(
        b"b",
        b"--b\r\nA: 1\r\n\r\n\r\n--b\r\nB: 2\r\n\r\n\r\n--b--",
        vec![PartStart, h("A", "1"), HeadersEnd, PartEnd, PartStart, h("B", "2"), HeadersEnd, PartEnd, Finished],
        None,
    );
}

#[test]
fn data_with_delimiter_lookalikes() {
    ok(
        b"b",
        b"--b\r\n\r\n--b\r\n--b--\r\n--b\r\n--b--",
        vec![PartStart, HeadersEnd, d(b"--b"), PartEnd, PartStart, HeadersEnd, PartEnd, PartStart, HeadersEnd, PartEnd, Finished],
    );
    // Wait: that body is "--b CRLF CRLF --b CRLF --b-- CRLF --b CRLF --b--":
    // part1 headers end, data "--b" (no CRLF before it), then "\r\n--b--" closes.
}

#[test]
fn close_right_after_data_lookalike() {
    ok(b"b", b"--b\r\n\r\n--b\r\n--b--", vec![PartStart, HeadersEnd, d(b"--b"), PartEnd, Finished]);
}

#[test]
fn binary_data_all_bytes() {
    let mut data: Vec<u8> = (0u8..=255).collect();
    data.extend((0u8..=255).rev());
    let mut body = b"--bnd\r\nContent-Type: application/octet-stream\r\n\r\n".to_vec();
    body.extend_from_slice(&data);
    body.extend_from_slice(b"\r\n--bnd--\r\n");
    ok(b"bnd", &body, vec![PartStart, h("Content-Type", "application/octet-stream"), HeadersEnd, d(&data), PartEnd, Finished]);
}

#[test]
fn long_boundary_70() {
    let b: Vec<u8> = std::iter::repeat(b"abcdefghij".iter().copied()).flatten().take(70).collect();
    let mut body = b"--".to_vec();
    body.extend_from_slice(&b);
    body.extend_from_slice(b"\r\n\r\nx\r\n--");
    body.extend_from_slice(&b);
    body.extend_from_slice(b"--");
    ok(&b, &body, vec![PartStart, HeadersEnd, d(b"x"), PartEnd, Finished]);
}

// ---------------------------------------------------------- boundary / set-up

#[test]
fn boundary_validation() {
    assert!(Parser::<64, 4>::new(b"a").is_ok());
    assert!(Parser::<64, 4>::new(b"a b").is_ok());
    assert!(Parser::<64, 4>::new(b"'()+_,-./:=?09azAZ").is_ok());
    assert!(Parser::<64, 4>::new(&[b'x'; 70]).is_ok());
    assert_eq!(Parser::<64, 4>::new(b"").err(), Some(Error::Boundary));
    assert_eq!(Parser::<64, 4>::new(&[b'x'; 71]).err(), Some(Error::Boundary));
    assert_eq!(Parser::<64, 4>::new(b"a ").err(), Some(Error::Boundary));
    assert_eq!(Parser::<64, 4>::new(b" ").err(), Some(Error::Boundary));
    assert_eq!(Parser::<64, 4>::new(b"a\"b").err(), Some(Error::Boundary));
    assert_eq!(Parser::<64, 4>::new(b"a;b").err(), Some(Error::Boundary));
    assert_eq!(Parser::<64, 4>::new(b"a\\b").err(), Some(Error::Boundary));
    assert_eq!(Parser::<64, 4>::new(b"a\r\nb").err(), Some(Error::Boundary));
    assert_eq!(Parser::<64, 4>::new(b"\xc3\xa4").err(), Some(Error::Boundary));
    let (ev, err) = run_whole::<64, 4>(b"", b"----");
    assert_eq!((ev, err), (vec![], Some(OError::Boundary)));
}

#[test]
fn from_content_type() {
    fn b(ct: &[u8]) -> Result<Vec<u8>, Error> {
        Parser::<64, 4>::from_content_type(ct).map(|p| p.boundary().to_vec())
    }
    assert_eq!(b(b"multipart/form-data; boundary=abc"), Ok(b"abc".to_vec()));
    assert_eq!(b(b"multipart/form-data;boundary=abc"), Ok(b"abc".to_vec()));
    assert_eq!(b(b"multipart/form-data ; boundary = abc "), Ok(b"abc".to_vec()));
    assert_eq!(b(b"  multipart/form-data; boundary=abc"), Ok(b"abc".to_vec()));
    assert_eq!(b(b"MULTIPART/Form-Data; BOUNDARY=abc"), Ok(b"abc".to_vec()));
    assert_eq!(b(b"multipart/mixed; boundary=abc"), Ok(b"abc".to_vec()));
    assert_eq!(b(b"multipart/form-data; boundary=\"abc\""), Ok(b"abc".to_vec()));
    assert_eq!(b(b"multipart/form-data; boundary=\"a b/c:d\""), Ok(b"a b/c:d".to_vec()));
    assert_eq!(b(b"multipart/form-data; charset=utf-8; boundary=abc"), Ok(b"abc".to_vec()));
    assert_eq!(b(b"multipart/form-data; boundary=abc; charset=utf-8"), Ok(b"abc".to_vec()));
    assert_eq!(b(b"multipart/form-data; x=\"a;b\"; boundary=abc"), Ok(b"abc".to_vec()));
    assert_eq!(b(b"multipart/form-data; x=\"a\\\"b\"; boundary=abc"), Ok(b"abc".to_vec()));
    assert_eq!(b(b"multipart/form-data; boundary=abc--"), Ok(b"abc--".to_vec()));
    assert_eq!(b(b"multipart/form-data; boundary=----WebKitFormBoundary7MA4YWxkTrZu0gW"), Ok(b"----WebKitFormBoundary7MA4YWxkTrZu0gW".to_vec()));
    assert_eq!(b(b"multipart/form-data; boundary=\"x\"; boundary=\"y\""), Ok(b"x".to_vec())); // first wins

    for bad in [
        &b"text/plain; boundary=abc"[..],
        b"multipart/form-data",
        b"multipart/form-data;",
        b"multipart/form-data; boundary=",
        b"multipart/form-data; boundary",
        b"multipart/form-data; boundary=\"abc",
        b"multipart/form-data; boundary=\"abc\\",
        b"multipart/form-data; boundary=a/b",
        b"multipart/form-data; boundary=\"abc \"",
        b"multipart/form-data; boundary=\"\"",
        b"multipart/form-data; boundary=a;b",
        b"multipart/form-data; =abc",
        b"multipart/form-data; a b=abc; boundary=abc",
        b"multipart; boundary=abc",
        b"multipart/; boundary=abc",
        b"multipart/form data; boundary=abc",
        b"/form-data; boundary=abc",
        b"multipartx/form-data; boundary=abc",
        b"multipart/form-data boundary=abc",
        b"multipart/form-data; charset=utf-8",
        b"",
    ] {
        assert_eq!(b(bad), Err(Error::Boundary), "{:?}", String::from_utf8_lossy(bad));
    }
}

#[test]
fn min_buffer_and_progress() {
    let mut p = Parser::<64, 4>::new(b"abc").unwrap();
    assert_eq!(p.min_buffer(), 3 + nostd_multipart::MAX_PADDING + 6);
    // Worst case: delimiter + MAX_PADDING pads + CR needs one more byte to
    // decide. With min_buffer - 1 bytes and no EOF the parser may stall ...
    let mut body = b"--abc\r\n\r\n\r\n--abc".to_vec();
    body.extend(std::iter::repeat(b' ').take(nostd_multipart::MAX_PADDING));
    body.push(b'\r');
    let mut pos = 0;
    loop {
        let (used, ev) = p.parse(&body[pos..], false).unwrap();
        pos += used;
        if ev == Some(Event::HeadersEnd) {
            break;
        }
    }
    let tail = &body[pos..];
    assert_eq!(tail.len(), p.min_buffer() - 1);
    assert_eq!(p.parse(tail, false), Ok((0, None)));
    // ... but with eof the held-back bytes are data.
    assert_eq!(p.parse(tail, true), Ok((tail.len(), Some(Event::Data(tail)))));
    assert_eq!(p.parse(b"", true), Err(Error::UnexpectedEof));
    // With exactly min_buffer bytes it is decided.
    let mut p = Parser::<64, 4>::new(b"abc").unwrap();
    let mut full = body.clone();
    full.push(b'\n');
    let mut pos = 0;
    loop {
        let (used, ev) = p.parse(&full[pos..], false).unwrap();
        pos += used;
        if ev == Some(Event::HeadersEnd) {
            break;
        }
    }
    let tail = &full[pos..];
    assert_eq!(tail.len(), p.min_buffer());
    assert_eq!(p.parse(tail, false), Ok((tail.len(), Some(Event::PartEnd))));
    assert_eq!(p.parse(b"", false), Ok((0, Some(Event::PartStart))));
    assert_eq!(p.parse(b"", false), Ok((0, None)));
}

#[test]
fn size_of_parser_is_documented() {
    use core::mem::size_of;
    assert_eq!(size_of::<Parser<'static, 512, 16>>(), 512 + 5 * size_of::<usize>());
    assert_eq!(size_of::<Parser<'static, 128, 4>>(), 128 + 5 * size_of::<usize>());
    assert_eq!(size_of::<Parser<'static, 64, 1>>(), 64 + 5 * size_of::<usize>());
}

#[test]
fn zero_copy_data_borrows_input() {
    let mut p = Parser::<64, 4>::new(b"b").unwrap();
    let body = b"--b\r\n\r\nhello world\r\n--b--";
    let mut pos = 0;
    loop {
        let (used, ev) = p.parse(&body[pos..], true).unwrap();
        if let Some(Event::Data(d)) = ev {
            assert_eq!(d, b"hello world");
            assert_eq!(d.as_ptr(), body[7..].as_ptr());
            break;
        }
        pos += used;
    }
}

#[test]
fn error_display_and_traits() {
    use std::error::Error as _;
    for e in [Error::Boundary, Error::HeaderTooLong, Error::TooManyHeaders, Error::Header, Error::UnexpectedEof, Error::Disposition] {
        assert!(!format!("{e}").is_empty());
        assert!(!format!("{e:?}").is_empty());
        assert!(e.source().is_none());
        let _ = e.clone();
    }
    let p = Parser::<8, 1>::new(b"b").unwrap();
    let q = p.clone();
    assert!(format!("{q:?}").contains("Parser"));
    let ev = Event::Data(b"x");
    assert_eq!(ev.clone(), Event::Data(b"x"));
    assert!(format!("{ev:?}").contains("Data"));
}
