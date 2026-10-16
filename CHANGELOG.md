# Changelog

## Unreleased

- Part-level API on `MultipartReader` / `MultipartReaderAsync`: `next_part()`
  returns a `PartInfo` with the part's `name()` / `filename()`, `next_chunk()`
  yields the body zero-copy, and the readers implement `embedded_io::Read` /
  `embedded_io_async::Read` for the current part's body (so `read_exact` works on
  a field); `read_body(&mut [u8; N])` copies a small field in one call. `ReadError` implements `embedded_io::Error`. No more hand-written
  `in_firmware` state machine; see DESIGN.md §14.
- `Encoder::write_part_header_to` / `write_close_to` (feature `embedded-io`)
  and `write_part_header_to_async` / `write_close_to_async` (feature
  `embedded-io-async`) write straight into a writer; new `WriteError`.
- Example `form_server`: try the parser from a browser without hardware.
- Example `picoserve-upload`: HTML form upload streamed into a flash sink
  through `MultipartReaderAsync` inside a picoserve handler.
- Test fixtures from native HTML form submissions in Chrome and Firefox.

## 0.1.1 — 2026-10-12

- `Parser::from_content_type` now rejects trailing garbage after a quoted
  boundary (`boundary="x" junk`), as `DESIGN.md` already required.
- Internal simplification of parser, `Content-Disposition` helper and
  `embedded-io` adapters; public API unchanged.
- README rewritten; Kani default proof set fits the 16 GB CI runner
  (`parse_two_calls_len4` moved behind `kani_full`).

## 0.1.0 — 2026-10-11

Initial release: streaming, zero-copy, heap-free `multipart/form-data` parser and
encoder for `#![no_std]`; `Content-Disposition` helper; optional `embedded-io` /
`embedded-io-async` adapters. See `DESIGN.md` for the normative specification and
`README.md` for the verification matrix.
