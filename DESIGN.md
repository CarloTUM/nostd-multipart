# nostd-multipart — normative design

This document is the specification. The parser (`src/parser.rs`), the encoder
(`src/encoder.rs`), the header helpers (`src/disposition.rs`) and the reference
oracle (`tests/oracle.rs`) are each written straight from this text. When code and
this document disagree, the code is wrong.

Sources: RFC 2046 §5.1.1 (multipart syntax), RFC 7578 (`multipart/form-data`),
RFC 2183 / RFC 6266 (`Content-Disposition`), RFC 5987 (`filename*`),
RFC 7230 §3.2 (header field syntax, `token`), WHATWG HTML §4.10.21.8
("multipart/form-data encoding algorithm").

Notation: `CR` = 0x0D, `LF` = 0x0A, `CRLF` = CR LF, `SP` = 0x20, `HTAB` = 0x09,
`B` = the boundary bytes, `|B|` = its length, `D` = the delimiter `CRLF "--" B`,
`D0` = the *first-delimiter-at-offset-0* form `"--" B` (see rule 2).

---

## 0. Hard constraints (checked by CI)

* `#![no_std]`, no `alloc`, `#![forbid(unsafe_code)]`, zero runtime dependencies.
  Optional features: `embedded-io`, `embedded-io-async` (adapters), `std` (tests only).
* MSRV 1.81, edition 2021. Builds for `thumbv7em-none-eabihf` and
  `riscv32imc-unknown-none-elf` without `alloc`.
* Non-test code denies `clippy::panic`, `unwrap_used`, `expect_used`,
  `indexing_slicing`, `arithmetic_side_effects`, `unreachable`. No recursion.
* All memory is fixed at compile time via const generics. `size_of::<Parser<..>>()`
  is asserted by a test and documented in README.

## 1. Boundary

```
bchars      = DIGIT / ALPHA / "'" / "(" / ")" / "+" / "_" / "," / "-" / "." / "/" / ":" / "=" / "?" / SP
boundary    = 1*70 bchars, last byte is not SP
```

* `Parser::new(b)` returns `Error::Boundary` unless `b` satisfies the grammar above.
* `Parser::from_content_type(ct)` parses `ct` as an RFC 7231 media type:
  `type "/" subtype *( OWS ";" OWS token "=" ( token / quoted-string ) )`.
  * The type must be `multipart` (case-insensitive); the subtype is not checked.
  * The first parameter whose name is `boundary` (case-insensitive) is used.
    Its value may be a token or a quoted-string. Inside a quoted-string no
    backslash processing is done: bchars contain neither `\` nor `"`, so an escaped
    byte can never form a valid boundary and the result would be `Error::Boundary`
    anyway. The returned parser borrows the boundary bytes from `ct` (zero-copy).
  * Anything else (no `multipart/`, syntax error before or in the `boundary`
    parameter, no `boundary` parameter, invalid boundary) is `Error::Boundary`.
    Parameters after the `boundary` parameter are not validated.
* `boundary_from_random(&[u8; N])` maps every random byte to one of 64 bchars
  (`A–Z a–z 0–9 - _`); `N` must be 1..=70, otherwise `Error::Boundary`.

## 2. Delimiter

```
D  = CRLF "--" B           ; delimiter
D0 = "--" B                ; permitted only at body byte offset 0
```

* `D` may begin at any body offset. `D0` matches only when its first byte is the
  very first byte of the body (RFC 2046: the preamble and its trailing CRLF may be
  absent). Everything else needs the CRLF.
* CRLF is required. A bare LF is an ordinary data byte, never a line break.
* Because `B` is made of bchars (which exclude CR and LF), CR occurs in `D` only at
  index 0 and LF only at index 1. Consequence used by the implementation: a
  candidate delimiter can only start at a CR byte (or at offset 0 for `D0`), and
  a failed candidate can never overlap the start of another candidate except at
  a later CR. No prefix table (KMP) is needed.

## 3. What follows the boundary

After `D` (or `D0`) has matched completely, the next bytes decide:

```
"--"                                     => close delimiter      (len = |D| + 2)
0*MAX_PADDING (SP / HTAB) CRLF           => part delimiter       (len = |D| + pad + 2)
anything else                            => NOT a delimiter: all of these bytes are
                                            ordinary data (or preamble)
```

* `MAX_PADDING = 8`. RFC 2046 lets transports add linear whitespace after the
  boundary but forbids composers from generating it. A fixed-memory streaming
  parser must bound the look-ahead, so padding is bounded; `MAX_PADDING + 1` or more
  SP/HTAB bytes mean "not a delimiter" (rule 3, last line).
  *Ambiguity resolved:* the RFC grammar has unbounded `transport-padding`; the
  bound is the RFC-conforming option that keeps memory fixed, and 8 bytes exceed
  anything observed in practice (composers emit zero).
* "Anything else" includes: `D` followed by a single `-` and a non-`-` byte; `D`
  followed by a byte that is neither `-`, SP, HTAB nor CR; padding followed by a
  non-CR; CR not followed by LF. A boundary that is a prefix of a longer token
  (`--abc` vs `--abcdef`) therefore never matches.
* Nothing after the close delimiter is examined (rule 4).

## 4. Preamble, epilogue, zero parts

* Bytes before the first delimiter are the preamble: discarded, no event.
* Bytes after the close delimiter are the epilogue: discarded, no event.
* A body consisting of just a close delimiter (optionally with preamble/epilogue)
  is valid and yields exactly one event, `Finished`.

## 5. Part headers

After a part delimiter, header lines follow up to and including the first empty
line. Each line is terminated by CRLF.

```
header-line = field-name ":" OWS field-value OWS CRLF
field-name  = token                          ; RFC 7230 tchar, 1 or more bytes
tchar       = "!" / "#" / "$" / "%" / "&" / "'" / "*" / "+" / "-" / "." /
              "^" / "_" / "`" / "|" / "~" / DIGIT / ALPHA
field-value = *( any byte except CR, LF )    ; bytes >= 0x80 allowed (UTF-8 from browsers)
OWS         = *( SP / HTAB )
```

Checks, in this order, for every line:

1. If the bytes of the line *including* its CRLF exceed `LINE` =>
   `Error::HeaderTooLong`. This is detected as soon as more than `LINE` bytes
   have been accumulated without a LF, i.e. possibly before the line ends.
   Exactly `LINE` bytes without a LF are stored; if the input then ends
   (`eof`), the error is `UnexpectedEof`, not `HeaderTooLong`.
2. The line must end in CRLF. A LF not preceded by CR (bare LF) => `Error::Header`.
3. A line whose content (without CRLF) is empty => `HeadersEnd`; the body starts
   with the next byte.
4. If this is the `(MAX_HEADERS + 1)`-th non-empty line of the part =>
   `Error::TooManyHeaders`.
5. The content must contain a `:`; the bytes before it are the name and must be a
   non-empty `token`; otherwise `Error::Header`. This covers: missing colon,
   whitespace before the colon, empty name, non-ASCII name, and obs-fold (a line
   starting with SP/HTAB cannot be a token).
6. If the content contains a CR anywhere => `Error::Header`.
7. The value is the bytes after the colon with leading and trailing SP/HTAB
   removed. The event is `Header { name, value }`; both borrow the parser's line
   buffer and are valid until the next `parse` call.

Headers are not interpreted (no `Content-Disposition` requirement, duplicates are
delivered in order). `MAX_HEADERS` is a count, not storage: the parser never
stores more than one line.

## 6. Part body

Body data runs from the byte after the headers' terminating empty line up to, but
not including, the CR that starts the next delimiter (`D`). A part whose body is
empty is `... CRLF CRLF CRLF "--" B ...`.

Every `Data(slice)` event is a sub-slice of the `input` passed to that `parse`
call. The concatenation of `Data` slices between `HeadersEnd` and `PartEnd` is the
exact body. The parser may split the body into any number of `Data` events; every
`Data` event carries at least one byte.

## 7. End of input

* `eof == true` tells the parser that no bytes follow `input`.
* If the close delimiter has not been seen when the input is exhausted with
  `eof == true` => `Error::UnexpectedEof`. This applies in the preamble, in the
  headers and in a body.
* A held-back candidate (rule 9) that cannot be completed because `eof == true` is
  data (or preamble) and is consumed as such.
* A close delimiter needs nothing after it: `... CRLF "--" B "--"` at EOF is valid.

## 8. Header helpers (pure functions, `src/disposition.rs`)

`content_disposition(value) -> Result<ContentDisposition, Error>` parses the value
of a `Content-Disposition` header:

```
disposition = token *( OWS ";" OWS token OWS "=" OWS ( token / quoted-string ) ) OWS
```

* `disposition_type` is the first token, raw (compare case-insensitively; the
  helper `is_form_data()` does so).
* Parameters named `name` and `filename` (case-insensitive) are returned as
  `ParamValue`, the first occurrence wins. `ParamValue::raw()` is the undecoded
  slice between the quotes (or the token). `ParamValue::bytes()` iterates the
  decoded bytes, `decode_into(&mut [u8])` writes them, `eq_bytes(&[u8])` compares them.
  Decoding: inside a quoted-string `\x` yields `x` (RFC 822 quoted-pair) and the
  three WHATWG percent escapes `%22`, `%0D`, `%0A` yield `"`, CR, LF. Every other
  `%` sequence is left untouched (a literal `%` is common in filenames). A token
  value is decoded the same way. Decoding is lossy by design (WHATWG's own
  encoding is): a filename that literally contained `%22` is indistinguishable
  from an escaped quote.
* `filename*` (RFC 5987 ext-value) is returned raw in `filename_star` with no
  charset or percent decoding; decoding needs allocation or a caller buffer and
  is out of scope. Browsers never send it for form uploads.
* Other parameters are ignored. An unterminated quoted-string, a missing `=`, an
  empty attribute, a non-token attribute, an empty or non-token disposition type,
  or bytes other than `;` after a closing quote is `Error::Disposition`. A
  trailing `;` is tolerated. *Leniency (documented deviation from the RFC 2183
  `token` rule):* an unquoted value extends to the next `;` and may contain any
  byte, because some non-browser clients send unquoted file names with spaces.

## 9. Streaming model and hold-back

`parse(input, eof) -> Result<(consumed, Option<Event>), Error>`

* `consumed <= input.len()` always.
* At most one event per call. The caller drops the first `consumed` bytes, appends
  new bytes, and calls again.
* **Hold-back.** In the preamble and in a body, a *candidate* is the earliest
  position `c` where `input[c..]` could still be the start of a delimiter
  (`input[c] == CR`, or `c == 0` and `D0` is allowed). The parser never consumes
  a candidate until it is resolved as *part delimiter*, *close delimiter* or *not
  a delimiter*. If the candidate cannot be resolved with the available bytes and
  `eof == false`, the bytes before it are emitted as `Data` (or discarded as
  preamble) and the candidate is left unconsumed. This is what keeps `Data`
  zero-copy: a failed candidate is still in `input` and is emitted as data.
* The longest look-ahead needed to resolve a candidate is
  `|D| + MAX_PADDING + 2` = `|B| + MAX_PADDING + 6` bytes. This is `min_buffer()`.

### Progress guarantee

If `input.len() >= min_buffer()` **or** `eof == true`, every call either consumes at
least one byte, returns `Some(event)`, or returns `Err`. Proof sketch per state:

| state                 | why progress holds                                                   |
|-----------------------|----------------------------------------------------------------------|
| Preamble              | a candidate at c = 0 resolves within `min_buffer` bytes; a candidate at c > 0 lets us discard c bytes; no candidate discards everything; empty + eof => Err |
| Headers               | any non-empty input is appended (>= 1 byte consumed) or errors; empty + eof => Err |
| Body                  | as Preamble, with `Data` instead of discarding                       |
| NewPart               | returns `PartStart` with 0 consumed                                  |
| Finished              | consumes everything, returns `Finished`                              |
| Failed(e)             | returns `Err(e)`                                                     |

After `Finished` every further call returns `Ok((input.len(), Some(Finished)))`.
After an error every further call returns the same `Err`.

## 10. State machine

```
                     ┌────────────────────────────────────────────────────────────┐
                     │                                                            │
   start ──► Preamble ──(D0/D + pad + CRLF)──► NewPart ──► Headers ──(empty line)──► Body
                │  ▲                              ▲  PartStart    │  Header*         │  Data*
                │  └── not a delimiter ───┘       │              │  HeadersEnd       │
                │     (discard, loop)             │              │                   │
                │                                 └──────(D + pad + CRLF: PartEnd)───┤
                │                                                                   │
                └──(D0/D "--": Finished)──► Finished ◄──(D "--": PartEnd, then Finished)┘

   any state ──(Err)──► Failed(e)        Finished / Failed are absorbing.
```

Events emitted on the arcs: `Preamble→NewPart` emits `PartStart` directly (NewPart
is skipped, state becomes `Headers`); `Body→NewPart` emits `PartEnd`, the next call
emits `PartStart` (0 consumed) and moves to `Headers`. `Body→Finished` emits
`PartEnd`; the next call emits `Finished`.

Per-state consumption rules:

**Preamble** (`at_start` is true until the first byte is consumed)
1. `c` = 0 if `at_start` and `input[0] == '-'` (try `D0`), else the index of the
   first CR at or after the scan position; if none, consume all of `input`
   (preamble), return `(len, None)`; if `len == 0` and `eof` => `UnexpectedEof`.
2. Classify `input[c..]` with rule 3:
   * part delimiter of length `n` => consume `c + n`, `PartStart`, state `Headers`.
   * close delimiter of length `n` => consume `c + n`, `Finished`, state `Finished`.
   * not a delimiter => continue scanning at `c + 1`.
   * undecidable (needs more bytes): if `eof` treat as *not a delimiter*; else
     consume `c`, return `(c, None)` (c may be 0 only if `len < min_buffer`).

**Headers**
1. Find the first LF in `input`. If none: append all of `input` to the line
   buffer (`HeaderTooLong` if it would exceed `LINE`), consume all, `(len, None)`;
   if `len == 0` and `eof` => `UnexpectedEof`.
2. Else append `input[..=lf]` (same overflow check), consume `lf + 1`, and apply
   rule 5 to the completed line: `HeadersEnd` (state `Body`), `Header`, or `Err`.

**Body** — like Preamble with `D0` never allowed and these differences:
* a run of non-candidate bytes `input[..c]` with `c > 0` is emitted as `Data`
  (consume `c`) instead of being discarded; a resolved delimiter at `c > 0` is
  *not* consumed in the same call — the `Data` is emitted first and the delimiter
  is consumed by the next call at `c == 0`;
* part delimiter at `c == 0` => consume `n`, `PartEnd`, state `NewPart`;
* close delimiter at `c == 0` => consume `n`, `PartEnd`, state `Finished`;
* no candidate at all => `Data(input)`, consume `len`; `len == 0 && eof` =>
  `UnexpectedEof`.

## 11. Encoder (`src/encoder.rs`)

`Encoder::new(boundary)` validates the boundary (rule 1).

The body produced for parts `P1..Pn` is:

```
"--" B CRLF  headers(P1) CRLF  data(P1)
CRLF "--" B CRLF  headers(P2) CRLF  data(P2)
...
CRLF "--" B "--" CRLF
```

`headers(P)` is
```
Content-Disposition: form-data; name="<esc(name)>"[; filename="<esc(filename)>"] CRLF
[Content-Type: <content_type> CRLF]
```
with `esc` replacing `"` => `%22`, CR => `%0D`, LF => `%0A` (WHATWG) **and**
`\` => `\\`. `content_type` must not contain CR or LF (`Error::Header` otherwise).

*Ambiguity resolved:* WHATWG's encoding algorithm does not escape `\`, but the
value is an RFC 2183 quoted-string, whose decoder (rule 8, and e.g. Python's
`cgi.parse_header`, Node's busboy) treats `\x` as a quoted-pair. A name ending in
`\` would therefore be emitted as `name="a\"`, which is an unterminated string.
The RFC-conforming option is to escape the backslash; browsers do not, which is
part of WHATWG's documented lossiness and is accepted on the decoding side.

* `write_part_header(&mut self, part, out) -> Result<(written, done), Error>`
  writes as many bytes of the part's prefix (leading CRLF for parts after the
  first, boundary line, headers, empty line) as fit into `out`; the caller flushes
  `out[..written]` and calls again with the *same* `part` until `done`. The
  caller then writes the part data itself.
* `write_close(&mut self, out) -> (written, done)` writes `CRLF "--" B "--" CRLF`
  resumably.
* `content_length(&self, parts: &[(Part, u64)]) -> u64` returns the exact byte
  length of the body (saturating at `u64::MAX`).
* The caller is responsible for data not containing `D` followed by `--`, padding
  or CRLF (RFC 2046 requirement; `boundary_from_random` with >= 16 bytes makes a
  collision negligible).

## 12. Error variants and where they come from

| variant          | source                                                             |
|------------------|--------------------------------------------------------------------|
| `Boundary`       | rule 1: invalid boundary, bad Content-Type, bad `boundary_from_random` size |
| `HeaderTooLong`  | rule 5.1                                                           |
| `TooManyHeaders` | rule 5.4                                                           |
| `Header`         | rule 5.2/5.5/5.6; encoder content_type with CR/LF                  |
| `UnexpectedEof`  | rule 7                                                             |
| `Disposition`    | rule 8                                                             |

## 13. Non-goals

Bare-LF leniency; nested `multipart/mixed` expansion (RFC 7578 §4.3 deprecated);
charset conversion / `_charset_`; `Content-Transfer-Encoding` decoding; decoding
`filename*`; storing headers in the parser (the application keeps what it needs
from each `Header` event; the adapters' `PartInfo` keeps one value for
convenience, §14).

## 14. Reader adapters (`src/io.rs`, features `embedded-io` / `embedded-io-async`)

`MultipartReader<R, BUF, LINE, MAX_HEADERS>` (and the `Async` twin) owns a
`BUF`-byte buffer and a `Parser`, and drives the parser from an `embedded_io::Read`.
Same semantics for both; the async version awaits the reader.

* `next_event()` is the §9 loop: run the parser on the buffered bytes; when it
  needs more, compact the buffer and read into the free space. A full buffer
  without progress is `BufferTooSmall` (`BUF < min_buffer()`). A read of 0 bytes
  sets `eof`. Errors: `Io(E)`, `Parse(Error)`, `BufferTooSmall`; `Parse` is
  sticky like the parser, `Finished` is absorbing.

On top of the events, three part-level operations. They are defined in terms of
the event stream and may be mixed freely with `next_event`:

* **`next_part() -> Option<PartInfo<LINE>>`** consumes events until `HeadersEnd`
  (=> `Some`) or `Finished` (=> `None`, and every later call is `None`). While
  doing so it skips `Data` and `PartEnd` of an unfinished part, and copies the
  value of the *first* `Content-Disposition` header seen since the last
  `PartStart` into `PartInfo` (the value is at most `LINE` bytes, so it always
  fits). Headers already consumed by `next_event` before the call are not seen.
  `PartInfo::disposition()` applies rule 8 lazily; a missing or malformed header
  gives `None` for `disposition()`, `name()`, `filename()`.
* **`next_chunk() -> Option<&[u8]>`**: *in a body* (after `HeadersEnd`, before
  the part's `PartEnd`), the undelivered rest of the current `Data` event if any,
  else the next event's `Data`; `None` when that event is `PartEnd`. *Outside a
  body* `None` without reading. A returned slice is never empty.
* **`read_body(out) -> Option<usize>`**: copies the body from the current
  position until `PartEnd` (=> `Some(bytes copied)`) or until `out` is full while
  the body continues (=> `None`; `out` is full, nothing is lost: the rest is still
  readable). Outside a body `Some(0)`. The "continues" test peeks at the next
  event without delivering it.
* **`Read::read(buf)`**: `0` if `buf` is empty or outside a body; otherwise copies
  the next bytes of the body as `next_chunk` would deliver them, at most
  `buf.len()`, and returns `0` exactly at `PartEnd`. `ReadError` implements
  `embedded_io::Error` (`Io` => inner kind, `Parse` => `InvalidData`,
  `BufferTooSmall` => `OutOfMemory`).

**No double delivery.** A `Data` event is delivered once, in order, whether it
goes out whole through `next_event` or `next_chunk`, or in pieces through
`read`. The adapter tracks how many bytes of the pending `Data` event have been
handed out; `next_event` marks the whole event delivered. Hence, for a part,
*concatenation of all bytes obtained from `next_event`'s `Data`, `next_chunk` and
`read`, in call order* equals the part body (rule 6).

`PartInfo<LINE>` is `LINE + 2 * size_of::<usize>()` bytes and lives on the
caller's stack; the adapter itself grows by two words.
