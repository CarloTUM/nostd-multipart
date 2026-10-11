//! Heap-free, panic-free, streaming `multipart/form-data` parser and encoder for
//! `#![no_std]`.
//!
//! The crate is *sans-IO*: you feed bytes to [`Parser::parse`] and get back at most
//! one [`Event`] per call plus the number of bytes consumed. Part data is yielded
//! as sub-slices of your own input buffer (zero-copy). Only part headers are
//! copied, into a line buffer whose size is the const generic `LINE`.
//!
//! The normative specification is `DESIGN.md` in the repository; the rule numbers
//! referenced in the source (`rule 3`, ...) point there.
//!
//! ```
//! use nostd_multipart::{Event, Parser};
//!
//! let body = b"--xyz\r\nContent-Disposition: form-data; name=\"f\"\r\n\r\nhello\r\n--xyz--";
//! let mut p: Parser<'_> = Parser::new(b"xyz").unwrap();
//! let mut pos = 0;
//! let mut data = Vec::new();
//! loop {
//!     let (used, ev) = p.parse(&body[pos..], true).unwrap();
//!     pos += used;
//!     match ev {
//!         Some(Event::Data(d)) => data.extend_from_slice(d),
//!         Some(Event::Finished) => break,
//!         _ => {}
//!     }
//! }
//! assert_eq!(data, b"hello");
//! ```
#![no_std]
#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented
    )
)]
#![deny(missing_docs, rust_2018_idioms, unused_must_use)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

#[cfg(any(test, feature = "std"))]
extern crate std;

mod chars;
mod error;
mod parser;


pub use error::Error;
pub use parser::{Event, Parser, MAX_PADDING};

