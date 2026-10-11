//! `content_disposition` helper (DESIGN.md rule 8).

use nostd_multipart::{content_disposition, Error, ParamValue};

fn dec(p: ParamValue<'_>) -> Vec<u8> {
    let v: Vec<u8> = p.bytes().collect();
    assert_eq!(v.len(), p.decoded_len());
    let mut buf = [0u8; 64];
    assert_eq!(p.decode_into(&mut buf).unwrap(), &v[..]);
    assert!(p.eq_bytes(&v));
    assert!(p == v.as_slice());
    assert!(p == v[..]);
    v
}

#[test]
fn browser_style() {
    let cd = content_disposition(b"form-data; name=\"field\"").unwrap();
    assert!(cd.is_form_data());
    assert_eq!(cd.disposition_type(), b"form-data");
    assert_eq!(dec(cd.name().unwrap()), b"field");
    assert_eq!(cd.filename(), None);
    assert_eq!(cd.filename_star(), None);

    let cd = content_disposition(b"form-data; name=\"file\"; filename=\"a b.bin\"").unwrap();
    assert_eq!(dec(cd.name().unwrap()), b"file");
    assert_eq!(dec(cd.filename().unwrap()), b"a b.bin");
    assert_eq!(cd.filename().unwrap().raw(), b"a b.bin");
    assert!(cd.filename().unwrap() == "a b.bin");
    assert!(cd.filename().unwrap() == *"a b.bin");
    assert!(!(cd.filename().unwrap() == "a b.bi"));
}

#[test]
fn whatwg_percent_escapes() {
    let cd = content_disposition(b"form-data; name=\"q%22uote%0D%0Anl%0d%0a\"; filename=\"100%25 %2 %\"").unwrap();
    assert_eq!(dec(cd.name().unwrap()), b"q\"uote\r\nnl\r\n");
    // Other % sequences are untouched.
    assert_eq!(dec(cd.filename().unwrap()), b"100%25 %2 %");
    assert_eq!(cd.name().unwrap().raw(), b"q%22uote%0D%0Anl%0d%0a");
}

#[test]
fn quoted_pairs() {
    let cd = content_disposition(b"form-data; name=\"a\\\"b\\\\c\\x\"").unwrap();
    assert_eq!(dec(cd.name().unwrap()), b"a\"b\\cx");
    // Inside quotes a trailing backslash escapes the quote => unterminated.
    assert_eq!(content_disposition(b"form-data; name=\"a\\\"").err(), Some(Error::Disposition));
    // In an (lenient) unquoted value a lone trailing backslash is literal.
    let cd = content_disposition(b"form-data; name=a\\").unwrap();
    assert_eq!(dec(cd.name().unwrap()), b"a\\");
}

#[test]
fn case_whitespace_and_order() {
    let cd = content_disposition(b"  Form-Data ;  NAME = \"n\" ; FileName=\"f\" ;").unwrap();
    assert!(cd.is_form_data());
    assert_eq!(cd.disposition_type(), b"Form-Data");
    assert_eq!(dec(cd.name().unwrap()), b"n");
    assert_eq!(dec(cd.filename().unwrap()), b"f");
    // Unquoted values, lenient: up to the next ';'.
    let cd = content_disposition(b"form-data; name=plain; filename=my file.txt").unwrap();
    assert_eq!(dec(cd.name().unwrap()), b"plain");
    assert_eq!(dec(cd.filename().unwrap()), b"my file.txt");
    // First occurrence wins.
    let cd = content_disposition(b"form-data; name=\"a\"; name=\"b\"").unwrap();
    assert_eq!(dec(cd.name().unwrap()), b"a");
    // Unknown parameters are skipped; semicolons inside quotes are fine.
    let cd = content_disposition(b"attachment; size=3; x=\"a;b=c\"; name=\"n\"").unwrap();
    assert!(!cd.is_form_data());
    assert_eq!(cd.disposition_type(), b"attachment");
    assert_eq!(dec(cd.name().unwrap()), b"n");
    // Empty values.
    let cd = content_disposition(b"form-data; name=\"\"; filename=").unwrap();
    assert_eq!(dec(cd.name().unwrap()), b"");
    assert_eq!(dec(cd.filename().unwrap()), b"");
    // No parameters at all.
    let cd = content_disposition(b"inline").unwrap();
    assert_eq!(cd.name(), None);
    assert_eq!(cd.filename(), None);
}

#[test]
fn filename_star_is_raw() {
    let cd = content_disposition(b"form-data; name=\"f\"; filename*=utf-8''%E2%82%AC%20rates.txt").unwrap();
    assert_eq!(cd.filename_star(), Some(&b"utf-8''%E2%82%AC%20rates.txt"[..]));
    assert_eq!(cd.filename(), None);
    let cd = content_disposition(b"form-data; filename*=\"x\"; filename*=y").unwrap();
    assert_eq!(cd.filename_star(), Some(&b"x"[..]));
}

#[test]
fn utf8_values() {
    let cd = content_disposition("form-data; name=\"ünïcode\"; filename=\"nötes %22q%22.txt\"".as_bytes()).unwrap();
    assert_eq!(dec(cd.name().unwrap()), "ünïcode".as_bytes());
    assert_eq!(dec(cd.filename().unwrap()), "nötes \"q\".txt".as_bytes());
}

#[test]
fn errors() {
    for bad in [
        &b""[..],
        b"   ",
        b"; name=\"a\"",
        b"form data; name=\"a\"",
        b"form-data; name",
        b"form-data; =\"a\"",
        b"form-data; na me=\"a\"",
        b"form-data; name=\"a",
        b"form-data; name=\"a\\",
        b"form-data; name=\"a\"x",
        b"form-data; name=\"a\" x; filename=\"b\"",
        b"form-data; name=\"a\";;",
        b"form-data;;",
    ] {
        assert_eq!(content_disposition(bad).err(), Some(Error::Disposition), "{:?}", String::from_utf8_lossy(bad));
    }
}

#[test]
fn decode_into_too_small() {
    let cd = content_disposition(b"form-data; name=\"abcdef\"").unwrap();
    let mut small = [0u8; 3];
    assert_eq!(cd.name().unwrap().decode_into(&mut small), None);
    let mut exact = [0u8; 6];
    assert_eq!(cd.name().unwrap().decode_into(&mut exact), Some(&b"abcdef"[..]));
    let mut zero = [0u8; 0];
    assert_eq!(content_disposition(b"form-data; name=\"\"").unwrap().name().unwrap().decode_into(&mut zero), Some(&b""[..]));
    let d = cd.name().unwrap().bytes();
    assert!(format!("{d:?}").contains("Decoded"));
    let _ = d.clone();
    assert!(format!("{cd:?}").contains("ContentDisposition"));
    let c2 = cd;
    assert_eq!(c2, cd);
}
