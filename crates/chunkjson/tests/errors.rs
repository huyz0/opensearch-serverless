//! Each refusal: its kind and absolute offset, whole and at one byte per call.
// ARITH: test inputs are small and fixed; an overflow would be a test bug, and debug builds panic on it.
#![allow(missing_docs, clippy::arithmetic_side_effects)]

mod common;

use chunkjson::{Config, ErrorKind, Event, Framing, Tokenizer};
use common::{config, feed_chunked, parse_chunked, position_of};

/// The error for `bytes`, which must be the same whole and byte by byte.
fn fails(bytes: &[u8], cfg: Config) -> (ErrorKind, u64) {
    let whole = parse_chunked(bytes, &[usize::MAX], cfg).expect_err("refused whole");
    let bytewise = parse_chunked(bytes, &[1], cfg).expect_err("refused byte by byte");
    assert_eq!(whole, bytewise, "{:?}", String::from_utf8_lossy(bytes));
    (whole.kind(), whole.offset())
}

fn single(bytes: &[u8]) -> (ErrorKind, u64) {
    let r = fails(bytes, Config::default());
    assert!(
        serde_json::from_slice::<serde_json::Value>(bytes).is_err(),
        "serde_json agrees"
    );
    r
}

#[test]
fn surrogate_escapes_must_pair_in_order() {
    assert_eq!(single(br#""\ud800""#), (ErrorKind::LoneSurrogate, 7));
    assert_eq!(single(br#""\ud800x""#), (ErrorKind::LoneSurrogate, 7));
    assert_eq!(single(br#""\ud800\n""#), (ErrorKind::LoneSurrogate, 8));
    assert_eq!(single(br#""\ud800\u0041""#), (ErrorKind::LoneSurrogate, 12));
    assert_eq!(single(br#""\ud800\ud800""#), (ErrorKind::LoneSurrogate, 12));
    assert_eq!(single(br#""\udc00""#), (ErrorKind::LoneSurrogate, 6));
    assert_eq!(single(br#""\udfff\ud800""#), (ErrorKind::LoneSurrogate, 6));
    let ok = common::parse_one(r#""\ud83d\ude00\u00e9\u0000""#.as_bytes(), &[1]).unwrap();
    assert_eq!(ok, serde_json::Value::String("😀é\0".into()));
}

#[test]
fn invalid_utf8_is_refused_where_it_breaks() {
    assert_eq!(single(b"\"a\xff\""), (ErrorKind::InvalidUtf8, 2));
    assert_eq!(single(b"\"ab\xc3\""), (ErrorKind::InvalidUtf8, 3));
    // a two-byte character split across slices, then broken by an ASCII byte
    assert_eq!(single(b"\"\xc3a\""), (ErrorKind::InvalidUtf8, 1));
    // an encoded surrogate and an overlong form, whole and split
    assert_eq!(single(b"\"\xed\xa0\x80\""), (ErrorKind::InvalidUtf8, 1));
    assert_eq!(single(b"\"\xc0\xaf\""), (ErrorKind::InvalidUtf8, 1));
    let tail_split = parse_chunked(b"\"xy\xe2\x82\xac\"", &[4, 1, 9], Config::default()).unwrap();
    assert_eq!(
        tail_split.docs,
        vec![serde_json::Value::String("xy€".into())]
    );
}

/// The kinds of invalid UTF-8 JSONTestSuite's `i_string_*` cases name, with
/// their raw bytes, which the vendored copy lost (`tests/data/JSONTestSuite.md`):
/// refused at the same byte under every chunking, pull and push.
#[test]
fn every_kind_of_invalid_utf8_is_refused_under_every_chunking() {
    let cases: [(&[u8], u64); 9] = [
        (b"[\"\xc0\xaf\"]", 2),                 // overlong 2-byte
        (b"[\"\xe0\x80\xaf\"]", 2),             // overlong 3-byte
        (b"[\"\xfc\x83\xbf\xbf\xbf\xbf\"]", 2), // 6-byte form
        (b"[\"\xf8\x88\x80\x80\x80\"]", 2),     // 5-byte form
        (b"[\"\xed\xa0\x80\"]", 2),             // encoded surrogate U+D800
        (b"[\"\xf4\xbf\xbf\xbf\"]", 2),         // beyond U+10FFFF
        (b"[\"a\xe2\x82\"]", 3),                // truncated
        (b"[\"a\x81b\"]", 3),                   // lone continuation byte
        (b"[\"ab\xe9\"]", 4),                   // lead byte cut off by the quote
    ];
    for (bytes, at) in cases {
        let want = (ErrorKind::InvalidUtf8, at);
        let label = String::from_utf8_lossy(bytes);
        for size in (1..=bytes.len()).chain([usize::MAX]) {
            let pulled = parse_chunked(bytes, &[size], Config::default()).expect_err("refused");
            assert_eq!(
                (pulled.kind(), pulled.offset()),
                want,
                "pull {label} at {size}"
            );
            let pushed =
                common::feed_chunked(bytes, &[size], Config::default()).expect_err("refused");
            assert_eq!(
                (pushed.kind(), pushed.offset()),
                want,
                "push {label} at {size}"
            );
        }
    }
}

#[test]
fn split_character_broken_after_the_slice_names_the_breaking_byte() {
    let err = parse_chunked(b"\"\xe2\x82a\"", &[3, 9], Config::default()).unwrap_err();
    assert_eq!((err.kind(), err.offset()), (ErrorKind::InvalidUtf8, 1));
    let err = parse_chunked(b"\"x\xe2\x82\xc0\"", &[3, 9], Config::default()).unwrap_err();
    assert_eq!((err.kind(), err.offset()), (ErrorKind::InvalidUtf8, 2));
    // complete width but not UTF-8 (an encoded surrogate split across slices)
    let err = parse_chunked(b"\"\xed\xa0\x80\"", &[2, 9], Config::default()).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidUtf8);
}

#[test]
fn control_characters_and_bad_escapes_are_refused() {
    assert_eq!(single(b"\"a\nb\""), (ErrorKind::ControlCharacter, 2));
    assert_eq!(single(b"\"\x1f\""), (ErrorKind::ControlCharacter, 1));
    assert_eq!(single(br#""\x""#), (ErrorKind::InvalidEscape, 2));
    assert_eq!(single(br#""\u12g4""#), (ErrorKind::InvalidEscape, 5));
    assert_eq!(single(br#""\'""#), (ErrorKind::InvalidEscape, 2));
}

/// Found by the `feed` fuzz target: the pull API named the control byte,
/// the push API the earlier broken character.
#[test]
fn a_broken_character_before_a_control_byte_comes_first() {
    let bytes = b"\"$\xe0\x00\x00";
    assert_eq!(single(bytes), (ErrorKind::InvalidUtf8, 2));
    for size in [1, 2, usize::MAX] {
        let e = common::feed_chunked(bytes, &[size], Config::default()).expect_err("refused");
        assert_eq!(
            (e.kind(), e.offset()),
            (ErrorKind::InvalidUtf8, 2),
            "push at {size}"
        );
    }
}

#[test]
fn bad_numbers_are_refused() {
    assert_eq!(single(b"01"), (ErrorKind::InvalidNumber, 1));
    assert_eq!(single(b"-"), (ErrorKind::InvalidNumber, 1));
    assert_eq!(single(b"[-]"), (ErrorKind::InvalidNumber, 2));
    assert_eq!(single(b"1."), (ErrorKind::InvalidNumber, 2));
    assert_eq!(single(b"[1.]"), (ErrorKind::InvalidNumber, 3));
    assert_eq!(single(b"1e+"), (ErrorKind::InvalidNumber, 3));
    assert_eq!(single(b"[1x]"), (ErrorKind::UnexpectedByte(b'x'), 2));
}

#[test]
fn structure_errors_name_the_byte() {
    assert_eq!(single(b"[1,]"), (ErrorKind::UnexpectedByte(b']'), 3));
    assert_eq!(single(b"{\"a\" 1}"), (ErrorKind::UnexpectedByte(b'1'), 5));
    assert_eq!(single(b"{1:2}"), (ErrorKind::UnexpectedByte(b'1'), 1));
    assert_eq!(single(b"{\"a\":1,}"), (ErrorKind::UnexpectedByte(b'}'), 7));
    assert_eq!(single(b"[1}"), (ErrorKind::UnexpectedByte(b'}'), 2));
    assert_eq!(single(b"{\"a\":1]"), (ErrorKind::UnexpectedByte(b']'), 6));
    assert_eq!(single(b"tru"), (ErrorKind::UnexpectedEof, 3));
    assert_eq!(single(b"trUe"), (ErrorKind::UnexpectedByte(b'U'), 2));
    assert_eq!(single(b"x"), (ErrorKind::UnexpectedByte(b'x'), 0));
    assert_eq!(single(b"}"), (ErrorKind::UnexpectedByte(b'}'), 0));
}

#[test]
fn truncated_input_and_trailing_data_are_refused() {
    assert_eq!(single(b""), (ErrorKind::UnexpectedEof, 0));
    assert_eq!(single(b"  "), (ErrorKind::UnexpectedEof, 2));
    assert_eq!(single(b"[1, "), (ErrorKind::UnexpectedEof, 4));
    assert_eq!(single(b"\"abc"), (ErrorKind::UnexpectedEof, 4));
    assert_eq!(single(b"\"\\u00"), (ErrorKind::UnexpectedEof, 5));
    assert_eq!(single(b"\"\xe2\x82"), (ErrorKind::UnexpectedEof, 3));
    assert_eq!(single(b"{} {}"), (ErrorKind::TrailingData, 3));
    assert_eq!(single(b"1 2"), (ErrorKind::TrailingData, 2));
}

#[test]
fn nesting_at_the_limit_is_accepted_and_one_more_refused() {
    let deep = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n)).into_bytes();
    assert!(common::parse_one(&deep(128), &[1]).is_ok());
    assert!(
        serde_json::from_slice::<serde_json::Value>(&deep(127)).is_ok(),
        "serde_json stops at 127"
    );
    assert_eq!(single(&deep(129)), (ErrorKind::DepthExceeded, 128));
    let objects = format!("{}1{}", "{\"k\":".repeat(129), "}".repeat(129)).into_bytes();
    assert_eq!(single(&objects), (ErrorKind::DepthExceeded, 128 * 5));
    let tight = Config {
        max_depth: 2,
        ..Config::default()
    };
    assert_eq!(fails(b"[[[]]]", tight), (ErrorKind::DepthExceeded, 2));
}

#[test]
fn numbers_longer_than_the_limit_are_refused() {
    let cfg = Config {
        max_number_len: 8,
        ..Config::default()
    };
    assert!(parse_chunked(b"[12345678]", &[1], cfg).is_ok());
    assert!(parse_chunked(b"[12345678]", &[usize::MAX], cfg).is_ok());
    assert_eq!(fails(b"[123456789]", cfg), (ErrorKind::NumberTooLong, 9));
    assert_eq!(fails(b"123456789", cfg), (ErrorKind::NumberTooLong, 8));
    let err = parse_chunked(b"[123456789]", &[3, 2, 9], cfg).unwrap_err();
    assert_eq!((err.kind(), err.offset()), (ErrorKind::NumberTooLong, 9));
}

#[test]
fn every_call_after_an_error_returns_the_same_error() {
    let mut tok = Tokenizer::new(Config::default());
    let (e, _) = tok.next(b"[", false).unwrap();
    assert_eq!(e, Event::BeginArray);
    let first = tok.next(b"}", false).unwrap_err();
    assert_eq!(first.kind(), ErrorKind::UnexpectedByte(b'}'));
    assert_eq!(tok.next(b"1]", true).unwrap_err(), first);
    assert_eq!(tok.next(b"", true).unwrap_err(), first);

    let mut tok = Tokenizer::new(Config::default());
    let first = tok.next(b"\"\xff\"", true).unwrap_err();
    assert_eq!((first.kind(), first.offset()), (ErrorKind::InvalidUtf8, 1));
    assert_eq!(tok.next(b"1", true).unwrap_err(), first);
}

#[test]
fn ndjson_refuses_a_newline_inside_a_document() {
    let cfg = config(Framing::Ndjson);
    assert_eq!(
        fails(b"{\"a\":\n1}\n", cfg),
        (ErrorKind::NewlineInDocument, 5)
    );
    assert_eq!(fails(b"[1,\n2]", cfg), (ErrorKind::NewlineInDocument, 3));
    assert_eq!(fails(b"{} {}\n", cfg), (ErrorKind::TrailingData, 3));
    assert_eq!(fails(b"1 2\n", cfg), (ErrorKind::TrailingData, 2));
    assert_eq!(fails(b"{}\n{", cfg), (ErrorKind::UnexpectedEof, 4));
}

#[test]
fn eof_is_repeated_and_offsets_and_depth_are_reported() {
    let mut tok = Tokenizer::new(Config::default());
    assert_eq!(tok.config().max_depth, 128);
    let (e, used) = tok.next(b"[[", false).unwrap();
    assert_eq!((e, used), (Event::BeginArray, 1));
    let (e, used) = tok.next(b"[", false).unwrap();
    assert_eq!((e, used), (Event::BeginArray, 1));
    assert_eq!((tok.depth(), tok.offset()), (2, 2));
    let (e, _) = tok.next(b"", false).unwrap();
    assert_eq!(e, Event::NeedMore);
    for (input, want) in [(&b"]"[..], Event::EndArray), (b"]", Event::EndArray)] {
        assert_eq!(tok.next(input, true).unwrap().0, want);
    }
    assert_eq!(
        tok.next(b"", true).unwrap().0,
        Event::EndDocument { start: 0, end: 4 }
    );
    assert_eq!(tok.next(b"", true).unwrap().0, Event::Eof);
    assert_eq!(tok.next(b"", true).unwrap().0, Event::Eof);
}

/// A mutation can turn a number's text into one past f64 range, which the
/// tokenizer accepts as text; the shrunk case proptest found, with the comma
/// at every position.
#[test]
fn a_number_mutated_past_f64_range_is_still_placed() {
    let v = serde_json::json!([1.6279903139249872e295, null, 100000000000000000u64]);
    let pretty = serde_json::to_string_pretty(&v).unwrap().into_bytes();
    for i in 0..pretty.len() {
        let mut text = pretty.clone();
        text[i] = b',';
        for e in [
            parse_chunked(&text, &[1], Config::default()).err(),
            feed_chunked(&text, &[1], Config::default()).err(),
        ]
        .into_iter()
        .flatten()
        {
            assert_eq!(
                (e.line(), e.column()),
                position_of(&text, e.offset()),
                "{e}"
            );
        }
    }
}
