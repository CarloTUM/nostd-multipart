//! Fuzz: arbitrary boundary, body and chunking. The shared test harness asserts
//! on every call: no panic, `consumed <= len`, progress guarantee, `Data` is a
//! sub-slice of input, sticky errors/Finished. The result must equal the oracle
//! (chunking invariance) and the whole-buffer run.
#![no_main]

#[path = "../../tests/common/mod.rs"]
#[allow(dead_code)]
mod common;

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

#[derive(Arbitrary, Debug)]
struct Input<'a> {
    boundary: &'a [u8],
    chunks: Vec<u8>,
    body: &'a [u8],
}

fuzz_target!(|input: Input<'_>| {
    if input.boundary.is_empty() || input.boundary.len() > 70 || input.body.len() > 4096 {
        return;
    }
    // Chunk sizes from the fuzzer (0 => 1 byte), padded to cover the body.
    let mut chunks: Vec<usize> = Vec::new();
    let mut rem = input.body.len();
    for &c in &input.chunks {
        if rem == 0 {
            break;
        }
        let n = (c as usize).max(1).min(rem);
        chunks.push(n);
        rem -= n;
    }
    if rem > 0 {
        chunks.push(rem);
    }
    let expected = common::oracle::parse(input.boundary, input.body, 32, 3);
    let whole = common::run_whole::<32, 3>(input.boundary, input.body);
    assert_eq!(whole, expected);
    let chunked = common::run_chunked::<32, 3>(input.boundary, input.body, &chunks);
    assert_eq!(chunked, expected);
    let ones = vec![1usize; input.body.len()];
    assert_eq!(common::run_chunked::<32, 3>(input.boundary, input.body, &ones), expected);
});
