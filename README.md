# nostd-multipart

[![crates.io](https://img.shields.io/crates/v/nostd-multipart.svg)](https://crates.io/crates/nostd-multipart)
[![docs.rs](https://docs.rs/nostd-multipart/badge.svg)](https://docs.rs/nostd-multipart)
[![CI](https://github.com/CarloTUM/nostd-multipart/actions/workflows/ci.yml/badge.svg)](https://github.com/CarloTUM/nostd-multipart/actions/workflows/ci.yml)
![MSRV 1.81](https://img.shields.io/badge/MSRV-1.81-blue)
![license](https://img.shields.io/crates/l/nostd-multipart.svg)

A streaming `multipart/form-data` parser and encoder for `#![no_std]` Rust that
never allocates and never panics.

I wrote this because I needed an embedded HTTP server to take a firmware
upload from a plain HTML `<input type="file">` form, and the upload was
much bigger than the RAM I had to hold it in
([picoserve#91](https://github.com/sammhicks/picoserve/issues/91)). With
this crate, a 256-byte parser and a 1 KiB buffer are enough to stream a
file of any size straight into flash.

## Highlights

- **No heap, no `alloc`, no `unsafe`, no dependencies.** All memory is fixed at
  compile time through const generics.
- **Sans-IO and zero-copy.** You push bytes in. File contents come back as
  slices of your own buffer, so the only thing the parser copies is
  header lines.
- **Panic-free by construction.** Library code denies `panic`, `unwrap`,
  `expect`, indexing, unchecked arithmetic and `unreachable`, and has no
  recursion.
- **Tested hard.** It is checked against an independent reference parser,
  every way of chunking the input, `multer`, captures from real clients,
  fuzzing and Kani proofs. See [Testing](#testing).
- Optional `embedded-io` / `embedded-io-async` adapters.
- Builds for `thumbv7em-none-eabihf` and `riscv32imc-unknown-none-elf`.

## Getting started

```toml
[dependencies]
nostd-multipart = "0.1"
```

The parser gives you one event per call. Here is a complete receive loop
that writes the part named `firmware` to flash:

```rust
use nostd_multipart::{content_disposition, Event, Parser};

fn receive(
    content_type: &[u8],
    mut read: impl FnMut(&mut [u8]) -> usize,
    mut flash: impl FnMut(&[u8]),
) -> Result<(), nostd_multipart::Error> {
    let mut parser: Parser<'_, 256, 8> = Parser::from_content_type(content_type)?;
    let mut buf = [0u8; 1024]; // must be >= parser.min_buffer()
    let (mut start, mut end, mut eof) = (0, 0, false);
    let mut is_firmware = false;
    loop {
        let (consumed, event) = parser.parse(&buf[start..end], eof)?;
        start += consumed;
        match event {
            Some(Event::Header { name, value }) if name.eq_ignore_ascii_case(b"content-disposition") => {
                let cd = content_disposition(value)?;
                is_firmware = cd.name().is_some_and(|n| n == "firmware");
            }
            Some(Event::Data(bytes)) if is_firmware => flash(bytes),
            Some(Event::Finished) => return Ok(()),
            _ => {}
        }
        if consumed == 0 && event.is_none() {
            // The parser needs more bytes: compact the buffer, then read.
            buf.copy_within(start..end, 0);
            end -= start;
            start = 0;
            let n = read(&mut buf[end..]);
            eof = n == 0;
            end += n;
        }
    }
}
```

Each part produces `PartStart`, any number of `Header { name, value }`,
`HeadersEnd`, any number of `Data(..)` and then `PartEnd`. `Finished` comes
after the closing boundary. A `Data` event is never empty, and joining all
`Data` slices of a part gives you exactly that part's body.

If you enable the `embedded-io` feature, `MultipartReader` runs this loop for
you. You construct it with `MultipartReader::<_, 1024, 256, 8>::new(socket, parser)`
and then call `next_event()`; see `examples/thumbv7em-mock-flash`.
`embedded-io-async` gives you `MultipartReaderAsync` in the same way.
With [picoserve](https://crates.io/crates/picoserve), pass the request body
reader to it; `examples/picoserve-upload` is a complete upload handler
(`cd examples/picoserve-upload && cargo run --release`).

### Trying it without a board

```sh
cargo run --release --features std --example form_server
```

This serves a plain HTML upload form on <http://127.0.0.1:8080> and streams
every submission through the loop above (256-byte parser, 1 KiB buffer),
printing each part's size and CRC-32. Upload something large to see that
memory stays flat.

### Sending a form

```rust
use nostd_multipart::{Encoder, Part};

let mut enc = Encoder::new(b"boundary").unwrap();
let part = Part {
    name: b"file",
    filename: Some(b"a.bin"),
    content_type: Some(b"application/octet-stream"),
};
let mut out = [0u8; 64]; // any size works, the calls are resumable
loop {
    let (n, done) = enc.write_part_header(&part, &mut out).unwrap();
    // send out[..n]
    if done { break; }
}
// send the file bytes yourself, then:
loop {
    let (n, done) = enc.write_close(&mut out);
    // send out[..n]
    if done { break; }
}
```

With the `embedded-io` feature you can skip the loops:
`enc.write_part_header_to(&part, &mut socket)?`, then write the file bytes,
then `enc.write_close_to(&mut socket)?` (async: the `_async` variants). They
encode through a 64-byte stack buffer.

`Encoder::content_length` computes the exact body size for the
`Content-Length` header up front. `boundary_from_random` turns a few RNG bytes
into a valid boundary. Field names and file names are escaped the way browsers
escape them (`"` → `%22`, CR → `%0D`, LF → `%0A`). The encoder also escapes
`\` as `\\`, as described in [DESIGN.md §11](DESIGN.md).

## Feature flags

| feature             | what it adds                                        |
|---------------------|-----------------------------------------------------|
| `embedded-io`       | blocking `MultipartReader` over an `embedded_io::Read`; `Encoder::write_part_header_to` / `write_close_to` for an `embedded_io::Write` |
| `embedded-io-async` | `MultipartReaderAsync` over an `embedded_io_async::Read`; `Encoder::write_part_header_to_async` / `write_close_to_async` |

None are enabled by default. (`std` exists only to build the host example;
`Error` implements `core::error::Error` regardless.)

## How much memory?

The crate never allocates. All of its memory is in your stack or your
statics:

| type                                    | 64-bit       | 32-bit         |
|-----------------------------------------|-------------:|---------------:|
| `Parser<'_, 512, 16>` (defaults)        | 552 B        | 532 B          |
| `Parser<'_, 256, 8>`                    | 296 B        | 276 B          |
| `Parser<'_, 128, 4>`                    | 168 B        | 148 B          |
| `Encoder<'_>`                           | 32 B         | 16 B           |
| `MultipartReader<'_, R, 1024, 512, 16>` | 1656 B + `R` | ≈ 1580 B + `R` |
| `MultipartReader<'_, R, 256, 128, 8>`   | 488 B + `R`  | ≈ 444 B + `R`  |

A parser takes `LINE + 5 * size_of::<usize>()` bytes, and a test checks that
this holds.

- **`LINE`** is the longest header line you accept, CRLF included. Browsers
  send `Content-Disposition` lines of about 60 bytes plus the file name. 256 is
  comfortable, and 128 is enough for short file names.
- **`MAX_HEADERS`** is just a counter and takes no storage.
- **Your read buffer** must be at least `parser.min_buffer()` bytes
  (boundary length + 14, at most 84). Any buffer larger than that only makes
  the `Data` events bigger. It never changes the result.

In numbers: `examples/stream_upload.rs` streams a 50 MiB upload through a
1 KiB buffer at about 300 MiB/s. The `thumbv7em-mock-flash` example builds
to a 7.2 KiB Cortex-M4F binary.

## What it guarantees

- **Correct framing** per RFC 2046 §5.1.1 and RFC 7578. That covers the
  preamble and epilogue, up to 8 bytes of transport padding after a boundary,
  and zero-part bodies. If the input ends before the closing boundary, you get
  `Error::UnexpectedEof`.
- **No false boundaries.** A boundary that is only a prefix of a longer token
  never matches. A stray `\r\n--`, a partial delimiter, or a lone LF is
  treated as ordinary data.
- **Chunking doesn't matter.** You get the same events whether you feed the
  body all at once or one byte at a time.
- **It always makes progress.** If you pass at least `min_buffer()` bytes (or
  set `eof`), each call consumes input, returns an event, or fails. Once the
  parser has failed it keeps returning the same error, and once it has
  finished it keeps returning `Finished`.
- **Strict headers.** Names must be RFC 7230 tokens. UTF-8 values are
  accepted, while obs-fold and bare LF are rejected. Headers are passed to you
  in order without interpretation.
- **`content_disposition` helper.** It parses RFC 2183 parameters and decodes
  quoted-pairs and the browser `%22 %0D %0A` escapes without allocating.
  `filename*` is returned raw.

## What it deliberately doesn't do

- Accept bare `\n` line endings. RFC 2046 requires CRLF.
- Expand nested `multipart/mixed` parts (deprecated by RFC 7578 §4.3). The
  inner body is passed through as opaque data.
- Convert charsets, decode `Content-Transfer-Encoding` or RFC 5987
  `filename*`, or store headers for you.

The full specification, including every edge case, is in [`DESIGN.md`](DESIGN.md).

## Testing

A parser that runs on a device you can't easily reflash shouldn't have
surprises, so every check below is independent of the others:

- **Reference oracle.** A separate whole-buffer parser written straight from
  DESIGN.md shares no code with the real one (`tests/oracle.rs`).
- **Exhaustive chunking.** Every possible split of 36 small bodies is tested,
  and every other test body gets random and 1-byte splits
  (`tests/exhaustive.rs`).
- **Round-trip property tests.** Encoder output goes through the parser with
  adversarial names and data (`tests/encoder.rs`).
- **Differential testing** against [`multer`](https://crates.io/crates/multer)
  (`tests/proptests.rs`).
- **Real clients.** Captured bodies from a plain HTML form submitted by
  Chrome and Firefox, from curl, Python `requests` and Node `FormData`, plus
  the RFC examples (`tests/fixtures/`, `tests/vectors.rs`).
- **Fuzzing.** There are 4 libFuzzer targets in `fuzz/`. Before 0.1.0 each ran
  for 10 minutes (2–30 M executions) and found nothing.
- **100 % line coverage** of `src/`, enforced in CI. Every `Error` variant is
  produced by at least one test.
- **Kani model checking.** See the next section.

`cargo test --all-features` runs everything except fuzzing, coverage and Kani.

### Kani proofs

[Kani](https://github.com/model-checking/kani) proves, for *every* possible
input of a given length, that the parser never panics and that
`consumed <= input.len()`. It also proves that `Data` is non-empty and lies
within the consumed bytes, and that a call with `eof` always makes progress.
CBMC can't handle symbolic input *lengths* on this code, so each harness fixes
the length.

The harnesses come in two sets:

- **Default set (`cargo kani --all-features`).** It runs in CI on a
  standard 16 GB GitHub runner. It covers inputs of 4 and 6 bytes in every
  parser state, the header state with 4 bytes, `content_disposition` on 4
  bytes, and `boundary_from_random`.
- **Full set (`RUSTFLAGS="--cfg kani_full" cargo kani --all-features`).**
  It needs a large machine and takes hours. For the 0.1.0 release it
  additionally proved inputs of 8 bytes, any split of 4 bytes across two
  `parse` calls, and the header state with 6 bytes. The remaining rungs (10
  and 12 bytes, 8-byte headers, 6/8-byte `content_disposition`, the encoder)
  are there for anyone with the hardware to finish them.

### Fixtures

To regenerate the real-client fixtures, run `tests/fixtures/capture/capture.sh`.
It needs curl, python3 with `requests`, and node ≥ 18.
`tests/fixtures/capture/browsers.sh` captures the HTML form
(`form.html`) from headless Chrome and Firefox.

## Minimum supported Rust version

Rust 1.81, checked in CI.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
