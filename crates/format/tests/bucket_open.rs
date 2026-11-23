//! docs/specs/bucket-assignment/SPEC.md, AC-9.

use oss_format::{BUCKET_OPEN_PREFIX, bucket_open_key, encode_open, parse_open};

#[test]
fn the_value_is_the_generation_in_decimal() {
    assert_eq!(encode_open(0), b"0");
    assert_eq!(encode_open(37), b"37");
    assert_eq!(encode_open(i64::MAX.unsigned_abs()).len(), 19);
    for generation in [0u64, 1, 9, 10, 12_345, u64::from(u32::MAX)] {
        assert_eq!(parse_open(&encode_open(generation)).unwrap(), generation);
    }
}

#[test]
fn whitespace_is_trimmed_and_anything_else_is_refused() {
    assert_eq!(parse_open(b" 12\n").unwrap(), 12);
    for bad in [
        &b""[..],
        b"  ",
        b"-1",
        b"+1",
        b"1x",
        b"x",
        b"1 2",
        b"9223372036854775808",
        b"\xff",
    ] {
        assert!(parse_open(bad).is_err(), "{bad:?}");
    }
    assert_eq!(
        parse_open(b"9223372036854775807").unwrap(),
        i64::MAX.unsigned_abs()
    );
}

#[test]
fn the_key_is_under_the_open_prefix() {
    assert_eq!(BUCKET_OPEN_PREFIX, "bucket-open/");
    assert_eq!(bucket_open_key("", 3).unwrap(), "bucket-open/3");
    assert_eq!(bucket_open_key("dep/", 15).unwrap(), "dep/bucket-open/15");
    assert!(bucket_open_key("dep", 3).is_err());
}
