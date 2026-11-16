//! JSONTestSuite's parsing cases (tests/data/JSONTestSuite.md): every `y_`
//! case is accepted and gives serde_json's value, every `n_` case is refused,
//! and each `i_` case, where RFC 8259 lets parsers differ, goes the way
//! pinned below. Each case runs through the pull and the push API, whole and
//! at several chunk sizes.
// ARITH: test inputs are small and fixed; an overflow would be a test bug, and debug builds panic on it.
#![allow(missing_docs, clippy::arithmetic_side_effects)]

mod common;

use chunkjson::{Config, Error, Event, Sink, Tokenizer};
use serde_json::Value;

/// One case's name and bytes.
fn cases() -> Vec<(String, Vec<u8>)> {
    let text = include_str!("data/jsontestsuite.json");
    let all: Vec<Value> = serde_json::from_str(text).expect("the data parses");
    all.iter()
        .map(|c| {
            let name = c["name"].as_str().expect("a name").to_owned();
            let bytes = if let Some(unit) = c["repeat"].as_str() {
                let times = c["times"].as_u64().expect("a count") as usize;
                let mut b = unit.repeat(times).into_bytes();
                b.extend_from_slice(c["tail"].as_str().expect("a tail").as_bytes());
                b
            } else {
                let input = c["input"].as_str().expect("an input");
                if c["invalid_utf8"].as_bool() == Some(true) {
                    // the copy lost these bytes; 0xFF is invalid UTF-8 anywhere
                    let mut b = Vec::new();
                    for ch in input.chars() {
                        if ch == '\u{FFFD}' {
                            b.push(0xFF);
                        } else {
                            let mut buf = [0; 4];
                            b.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                    }
                    b
                } else {
                    input.as_bytes().to_vec()
                }
            };
            (name, bytes)
        })
        .collect()
}

const SIZES: [usize; 4] = [usize::MAX, 1, 3, 7];

/// Whether the pull API accepts `bytes` fed in chunks of `size`.
fn pulled(bytes: &[u8], size: usize) -> bool {
    let mut tok = Tokenizer::new(Config::default());
    let mut pos: usize = 0;
    loop {
        let end = pos.saturating_add(size).min(bytes.len());
        let eof = end == bytes.len();
        let mut chunk = &bytes[pos..end];
        loop {
            match tok.next(chunk, eof) {
                Err(_) => return false,
                Ok((Event::Eof, _)) => return true,
                Ok((Event::NeedMore, _)) => {
                    assert!(!eof, "never NeedMore once eof is set");
                    break;
                }
                Ok((_, used)) => chunk = &chunk[used..],
            }
        }
        pos = end;
    }
}

/// Whether the push API accepts `bytes` fed in chunks of `size`.
fn pushed(bytes: &[u8], size: usize) -> bool {
    struct Ignore;
    impl Sink for Ignore {
        type Error = Error;
        fn event(&mut self, _: Event<'_>) -> Result<(), Error> {
            Ok(())
        }
    }
    let mut tok = Tokenizer::new(Config::default());
    let mut pos: usize = 0;
    loop {
        let end = pos.saturating_add(size).min(bytes.len());
        let eof = end == bytes.len();
        match tok.feed(&bytes[pos..end], eof, &mut Ignore) {
            Err(_) => return false,
            Ok(done) if eof => return done,
            Ok(_) => pos = end,
        }
    }
}

/// Whether every API and chunking accepts `bytes`; panics if they disagree.
fn accepted(name: &str, bytes: &[u8]) -> bool {
    let first = pulled(bytes, usize::MAX);
    for size in SIZES {
        assert_eq!(pulled(bytes, size), first, "{name}: pull at {size}");
        assert_eq!(pushed(bytes, size), first, "{name}: push at {size}");
    }
    first
}

/// The `i_` cases accepted: numbers past f64's range or precision, passed
/// through as text (serde_json refuses the ones past its range). Every other
/// `i_` case (lone surrogates, invalid UTF-8, a byte-order mark, nesting past
/// the default 128) is refused.
const ACCEPTED_I: [&str; 10] = [
    "i_number_double_huge_neg_exp.json",
    "i_number_huge_exp.json",
    "i_number_neg_int_huge_exp.json",
    "i_number_pos_double_huge_exp.json",
    "i_number_real_neg_overflow.json",
    "i_number_real_pos_overflow.json",
    "i_number_real_underflow.json",
    "i_number_too_big_neg_int.json",
    "i_number_too_big_pos_int.json",
    "i_number_very_big_negative_int.json",
];

#[test]
fn every_case_goes_the_way_the_suite_says() {
    let cases = cases();
    assert_eq!(cases.len(), 318, "the whole suite");
    let mut seen = [0usize; 3];
    for (name, bytes) in &cases {
        let ours = accepted(name, bytes);
        match name.as_bytes()[0] {
            b'y' => {
                assert!(ours, "{name} is valid JSON");
                let want: Value = serde_json::from_slice(bytes).expect("serde_json reads it");
                for size in SIZES {
                    let got =
                        common::parse_chunked(bytes, &[size], Config::default()).expect("accepted");
                    assert_eq!(got.docs, std::slice::from_ref(&want), "{name} at {size}");
                }
                seen[0] += 1;
            }
            b'n' => {
                assert!(!ours, "{name} is not JSON");
                seen[1] += 1;
            }
            _ => {
                assert_eq!(ours, ACCEPTED_I.contains(&name.as_str()), "{name}");
                seen[2] += 1;
            }
        }
    }
    assert_eq!(seen, [95, 188, 35]);
}

#[test]
fn comments_turned_on_refuse_no_valid_case() {
    let config = Config {
        comments: true,
        ..Config::default()
    };
    for (name, bytes) in cases().iter().filter(|(n, _)| n.starts_with("y_")) {
        let got = common::parse_chunked(bytes, &[3], config).expect(name);
        assert_eq!(got.docs.len(), 1, "{name}");
    }
}
