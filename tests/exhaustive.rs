//! Chunking invariance, exhaustive: every body here is <= 24 bytes and is fed to
//! the parser under EVERY one of its 2^(n-1) chunkings; each run must equal the
//! oracle. (Longer bodies are covered by `vectors.rs`, `proptests.rs`, fixtures and
//! the fuzz targets with random and 1-byte splits.)

mod common;
use common::*;

fn exhaustive<const LINE: usize, const MAX: usize>(boundary: &[u8], body: &[u8]) {
    assert!(body.len() <= 24);
    check_against_oracle::<LINE, MAX>(boundary, body, &all_compositions(body.len()));
}

macro_rules! bodies {
    ($($name:ident : $boundary:literal => $body:literal;)*) => {
        $( #[test] fn $name() { exhaustive::<16, 2>($boundary, $body); } )*
    };
}

bodies! {
    ex_zero_parts: b"b" => b"--b--";
    ex_zero_parts_pre_epi: b"b" => b"pre\r\n--b--\r\nepilogue";
    ex_empty_body: b"b" => b"--b\r\n\r\n\r\n--b--";
    ex_one_byte: b"b" => b"--b\r\n\r\nx\r\n--b--";
    ex_header: b"b" => b"--b\r\nA:1\r\n\r\nx\r\n--b--";
    ex_header_ows: b"b" => b"--b\r\nA: \t1 \r\n\r\n\r\n--b--";
    ex_two_parts: b"b" => b"--b\r\n\r\nx\r\n--b\r\n\r\n\r\n--b--";
    ex_two_empty: b"b" => b"--b\r\n\r\n\r\n--b\r\n\r\n\r\n--b--";
    ex_pad: b"b" => b"--b \t\r\n\r\n\r\n--b--";
    ex_pad2: b"b" => b"--b\r\n\r\n\r\n--b \r\n\r\n\r\n--b--";
    ex_too_much_pad: b"b" => b"--b         \r\n\r\nx\r\n--b--";
    ex_lookalike_dash: b"b" => b"--b\r\n\r\nx\r\n--b-x\r\n--b--";
    ex_lookalike_cr: b"b" => b"--b\r\n\r\nx\r\n--b\rx\r\n--b--";
    ex_lookalike_pad: b"b" => b"--b\r\n\r\nx\r\n--b  x\r\n--b--";
    ex_prefixes: b"b" => b"--b\r\n\r\n\r\r\n-\r\n--\r\n--b--";
    ex_longer_token: b"ab" => b"--ab\r\n\r\n\r\n--abc\r\n--ab--";
    ex_lf_only: b"b" => b"--b\n\n--b--";
    ex_lf_data: b"b" => b"--b\r\n\r\n\n\n\n--b\n\r\n--b--";
    ex_preamble_crlf: b"b" => b"junk\r\n--b\r\n\r\nx\r\n--b--";
    ex_d0_not_at_zero: b"b" => b"x--b\r\n\r\nd\r\n--b--";
    ex_eof_after_data: b"b" => b"--b\r\n\r\nxyz\r\n--b";
    ex_eof_partial: b"b" => b"--b\r\n\r\nxyz\r\n--b\r";
    ex_eof_in_headers: b"b" => b"--b\r\nA:1\r\nB";
    ex_bare_lf_header: b"b" => b"--b\r\nA:1\n\r\n\r\n--b--";
    ex_obs_fold: b"b" => b"--b\r\nA:1\r\n b\r\n\r\n--b--";
    ex_no_colon: b"b" => b"--b\r\nAB\r\n\r\nx\r\n--b--";
    ex_too_long: b"b" => b"--b\r\nA:12345678901234\r\n";
    ex_too_many: b"b" => b"--b\r\nA:1\r\nB:2\r\nC:3\r\n";
    ex_cr_in_value: b"b" => b"--b\r\nA:1\r2\r\n\r\n\r\n--b--";
    ex_close_epilogue: b"b" => b"--b\r\n\r\nx\r\n--b--junk\r\n--b";
    ex_close_pad: b"b" => b"--b\r\n\r\nx\r\n--b-- \r\n";
    ex_crlf_at_start: b"b" => b"\r\n--b\r\n\r\nx\r\n--b--";
    ex_many_cr: b"b" => b"--b\r\n\r\n\r\r\r\r\r\r\r\n--b--";
    ex_dashes: b"b" => b"--b\r\n\r\n-----\r\n--b--";
    ex_dash_only_body: b"b" => b"--b\r\n\r\n--b--";
    ex_boundary_in_data: b"b" => b"--b\r\n\r\n--b\r\n--b--";
}

#[test]
fn composition_count_is_right() {
    assert_eq!(all_compositions(0), vec![Vec::<usize>::new()]);
    assert_eq!(all_compositions(1), vec![vec![1]]);
    assert_eq!(all_compositions(3).len(), 4);
    assert_eq!(all_compositions(10).len(), 512);
    for c in all_compositions(6) {
        assert_eq!(c.iter().sum::<usize>(), 6);
    }
}
