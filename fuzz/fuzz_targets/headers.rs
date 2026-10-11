//! Fuzz: `Parser::from_content_type`, `content_disposition` and `ParamValue`
//! decoding never panic, and decoding is self-consistent.
#![no_main]

use libfuzzer_sys::fuzz_target;
use nostd_multipart::{content_disposition, Parser};

fuzz_target!(|data: &[u8]| {
    if let Ok(p) = Parser::<64, 4>::from_content_type(data) {
        assert!(!p.boundary().is_empty() && p.boundary().len() <= 70);
        assert!(Parser::<64, 4>::new(p.boundary()).is_ok());
    }
    if let Ok(cd) = content_disposition(data) {
        assert!(!cd.disposition_type().is_empty());
        for v in [cd.name(), cd.filename()].into_iter().flatten() {
            let decoded: Vec<u8> = v.bytes().collect();
            assert_eq!(decoded.len(), v.decoded_len());
            assert!(decoded.len() <= v.raw().len());
            let mut buf = vec![0u8; decoded.len()];
            assert_eq!(v.decode_into(&mut buf), Some(&decoded[..]));
            if !decoded.is_empty() {
                assert_eq!(v.decode_into(&mut buf[..decoded.len() - 1]), None);
            }
            assert!(v.eq_bytes(&decoded));
        }
    }
});
