# nostd-multipart

Heap-free, panic-free, streaming `multipart/form-data` parser and encoder for
`#![no_std]` Rust. Built for embedded HTTP servers that must accept HTML file
uploads (OTA firmware, configuration files) whose body does not fit in RAM
([picoserve#91](https://github.com/sammhicks/picoserve/issues/91)).

* `#![no_std]`, **no `alloc`**, `#![forbid(unsafe_code)]`, zero runtime dependencies.
* Sans-IO and zero-copy: you feed bytes, part data comes back as sub-slices of
  your own buffer. Only header lines are copied, into one fixed `LINE`-byte buffer.
* Memory is fixed at compile time through const generics; `size_of` is asserted by
  a test (table below).
* Panic-free by construction: non-test code denies `clippy::panic`, `unwrap_used`,
  `expect_used`, `indexing_slicing`, `arithmetic_side_effects`, `unreachable`. No
  recursion.
* Verified by independent oracles: a whole-buffer reference parser, exhaustive
  chunking, property tests, a differential test against `multer`, real-client
  fixtures (curl, Python requests, Node FormData), libFuzzer targets and Kani
  proofs. See [Verification](#verification).
* Optional adapters for `embedded-io` and `embedded-io-async`.
* MSRV 1.81, edition 2021. Builds for `thumbv7em-none-eabihf` and
  `riscv32imc-unknown-none-elf`.

The normative specification is [`DESIGN.md`](DESIGN.md).

## Usage

```rust
use nostd_multipart::{content_disposition, Event, Parser};

fn receive(content_type: &[u8], mut read: impl FnMut(&mut [u8]) -> usize, mut flash: impl FnMut(&[u8])) -> Result<(), nostd_multipart::Error> {
    let mut parser: Parser<'_, 256, 8> = Parser::from_content_type(content_type)?;
    let mut buf = [0u8; 1024];                 // >= parser.min_buffer()
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
            // Hold-back: the parser needs more bytes. Compact, then read.
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

Events per part: `PartStart`, `Header{name, value}`\*, `HeadersEnd`, `Data(..)`\*,
`PartEnd`; then `Finished` once the close delimiter is seen. `Data` is never empty
and the concatenation of all `Data` slices of a part is exactly the part body.

With the `embedded-io` feature the loop above is
`MultipartReader::<_, 1024, 256, 8>::new(socket, parser)` + `next_event()`
(see `examples/thumbv7em-mock-flash`); `embedded-io-async` provides
`MultipartReaderAsync`.

### Encoder

```rust
use nostd_multipart::{Encoder, Part};
let mut enc = Encoder::new(b"boundary").unwrap();
let part = Part { name: b"file", filename: Some(b"a.bin"), content_type: Some(b"application/octet-stream") };
let mut out = [0u8; 64];                       // any size; the call is resumable
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

`Encoder::content_length(&[(part, data_len)])` returns the exact body length for the
`Content-Length` header; `boundary_from_random(&[u8; N])` turns RNG bytes into a
valid boundary. Names and file names are escaped WHATWG-style (`"`→`%22`,
CR→`%0D`, LF→`%0A`; additionally `\`→`\\`, see DESIGN.md §11).

## Memory

Everything is on the stack or in your statics; the crate never allocates.

| type                                              | size (64-bit) | size (32-bit) | formula                                   |
|---------------------------------------------------|--------------:|--------------:|-------------------------------------------|
| `Parser<'_, 512, 16>` (defaults)                  | 552 B         | 532 B         | `LINE + 5 * size_of::<usize>()` (asserted) |
| `Parser<'_, 256, 8>`                              | 296 B         | 276 B         |                                           |
| `Parser<'_, 128, 4>`                              | 168 B         | 148 B         |                                           |
| `Encoder<'_>`                                     | 32 B          | 16 B          | boundary slice + 2 words                  |
| `MultipartReader<'_, R, 1024, 512, 16>`           | 1656 B + `R`  | ≈ 1580 B + `R`| `BUF + size_of::<Parser>() + 8 words`     |
| `MultipartReader<'_, R, 256, 128, 8>`             | 488 B + `R`   | ≈ 444 B + `R` |                                           |
| `Event<'_, '_>`                                   | 40 B          | 20 B          |                                           |

`LINE` is the only tunable buffer: a header line *including its CRLF* longer than
`LINE` is `Error::HeaderTooLong`. Browsers send `Content-Disposition` lines of
roughly 60 bytes plus the (percent-escaped, UTF-8) file name; 256 is comfortable,
128 works for short file names. `MAX_HEADERS` is a counter, not storage.

`Parser::min_buffer()` = `boundary.len() + MAX_PADDING + 6` (≤ 84 bytes) is the
smallest input slice that guarantees progress on every call. Any larger working
buffer only changes the size of `Data` events, never the result.

The host example `examples/stream_upload.rs` streams a 50 MiB upload through a
1 KiB buffer with a 296-byte parser at ~300 MiB/s; the `thumbv7em-mock-flash`
example links to a 7.2 KiB Cortex-M4F binary with a 256-byte buffer.

## Guarantees

* **Correct framing per RFC 2046 §5.1.1 / RFC 7578**: delimiter = CRLF `--` boundary,
  the first delimiter may sit at offset 0, close delimiter `--boundary--`, up to 8
  bytes of SP/HTAB transport padding, preamble and epilogue ignored, zero parts
  allowed, EOF before the close delimiter is `Error::UnexpectedEof`.
* **A boundary that is a prefix of a longer token never matches**; `\r\n--` without
  the boundary, every prefix of the delimiter, the delimiter followed by a
  non-terminating byte, and bare-LF text are all ordinary data.
* **Chunking invariance**: the event stream (with `Data` concatenated per part) is
  identical for every way of splitting the input, including 1-byte chunks.
* **Progress**: with `input.len() >= min_buffer()` or `eof`, each call consumes ≥ 1
  byte, yields an event, or fails. `consumed <= input.len()` always. After an error
  the same error is returned forever; after `Finished`, `Finished` forever.
* **Headers**: RFC 7230 token names, values trimmed of SP/HTAB, bytes ≥ 0x80 allowed
  (UTF-8 file names), obs-fold and bare LF rejected, `LINE`/`MAX_HEADERS` bounds.
  Headers are not interpreted; duplicates are delivered in order.
* **`content_disposition`**: RFC 2183 syntax, quoted-pair and WHATWG `%22 %0D %0A`
  decoding without allocation (`ParamValue::bytes()` iterator, `decode_into`,
  `eq_bytes`), `filename*` returned raw.
* No panics, no recursion, no `unsafe`, no `alloc`.

## Non-goals

* **Bare-LF leniency.** CRLF is required by RFC 2046; `\n--boundary` is data.
* **Nested `multipart/mixed` expansion** (RFC 7578 §4.3, deprecated): the inner
  body is delivered as the opaque data of the outer part.
* **Charset conversion / `_charset_`**, `Content-Transfer-Encoding` decoding,
  decoding of `filename*` (RFC 5987), storing headers for you.
* Transport padding longer than `MAX_PADDING` (8) bytes after a boundary (treated
  as data; composers must not generate padding at all).

## Verification

Each item is independent of the others; all must pass (`cargo test --all-features`
runs 1–5 and 9; the rest have their own commands).

| # | what                                                                                           | where                                  | command                                                   |
|---|------------------------------------------------------------------------------------------------|----------------------------------------|-----------------------------------------------------------|
| 1 | Reference oracle: whole-buffer parser written from DESIGN.md, no shared code                   | `tests/oracle.rs`                      | `cargo test --test oracle`                                |
| 2 | Chunking invariance: every split (2^(n-1)) of 36 bodies ≤ 24 bytes; random + 1-byte splits for everything else | `tests/exhaustive.rs`, `tests/common` | `cargo test --test exhaustive`              |
| 3 | Round-trip: proptest encoder → parser over adversarial names/filenames/data                    | `tests/encoder.rs`                     | `cargo test --test encoder`                               |
| 4 | Differential against `multer` for valid bodies, all chunkings                                  | `tests/proptests.rs`                   | `cargo test --test proptests`                             |
| 5 | Real clients: curl `-F`, Python `requests`, Node `FormData`; RFC 2046/7578 examples            | `tests/fixtures/`, `tests/vectors.rs`  | `cargo test --test fixtures --test vectors`               |
| 6 | Fuzzing: 4 libFuzzer targets (chunked parse vs oracle, round-trip, header helpers, io adapter); last release run: 2.3M / 9.9M / 31.6M / 10.3M executions in 10 min each, no findings | `fuzz/`                                | `cd fuzz && cargo +nightly fuzz run <target> -- -max_total_time=600` |
| 7 | Kani: no panics, `consumed <= len`, progress; default bounds 12 B input / 3 B boundary (fits 16 GB RAM), `--cfg kani_full` for 16 B / 4 B | `src/kani.rs`                          | `cargo kani --all-features` (full: `RUSTFLAGS="--cfg kani_full" cargo kani --all-features`) |
| 8 | Coverage: 100 % of lines in `src/`; every `Error` variant produced by a test                   | CI gate                                | `cargo llvm-cov --all-features --fail-under-lines 100`    |
| 9 | Adversarial cases (DESIGN.md list), `size_of` assertion, `min_buffer` worst case               | `tests/vectors.rs`                     | `cargo test --test vectors`                               |

CI (`.github/workflows/ci.yml`): test, clippy `-D warnings`, rustfmt, docs,
`thumbv7em-none-eabihf` + `riscv32imc-unknown-none-elf` builds of the library and
of the `no_main` example (no allocator ⇒ no `alloc`), MSRV 1.81, 60 s fuzz smoke per
target, Kani, coverage gate.

Regenerate the fixtures with `tests/fixtures/capture/capture.sh` (needs curl,
python3 + requests, node ≥ 18).

## License

MIT OR Apache-2.0.
