//! The push API gives exactly the pull API's events and errors.
// ARITH: test inputs are small and fixed; an overflow would be a test bug, and debug builds panic on it.
#![allow(missing_docs, clippy::arithmetic_side_effects)]

mod common;

use chunkjson::{Config, Error, Event, Framing, Sink, Tokenizer};
use proptest::prelude::*;

/// Events as owned text, so runs can be compared.
#[derive(Default, Debug, PartialEq)]
struct Log(Vec<String>);

impl Sink for Log {
    type Error = Error;
    fn event(&mut self, event: Event<'_>) -> Result<(), Error> {
        assert!(!matches!(event, Event::NeedMore | Event::Eof));
        self.0.push(format!("{event:?}"));
        Ok(())
    }
}

fn pull(bytes: &[u8], size: usize, cfg: Config) -> (Vec<String>, Result<(), Error>) {
    let mut tok = Tokenizer::new(cfg);
    let mut log = Vec::new();
    let mut offsets = Vec::new();
    let chunks: Vec<&[u8]> = if bytes.is_empty() {
        vec![]
    } else {
        bytes.chunks(size).collect()
    };
    for (i, chunk) in chunks.iter().chain(std::iter::once(&&b""[..])).enumerate() {
        let eof = i >= chunks.len();
        let mut c = *chunk;
        loop {
            match tok.next(c, eof) {
                Err(e) => return (log, Err(e)),
                Ok((Event::NeedMore, _)) => {
                    offsets.push(tok.offset());
                    break;
                }
                Ok((Event::Eof, _)) => {
                    log.push(format!("offsets {offsets:?} end {}", tok.offset()));
                    return (log, Ok(()));
                }
                Ok((event, used)) => {
                    log.push(format!("{event:?}"));
                    c = &c[used..];
                }
            }
        }
    }
    unreachable!("eof was fed")
}

fn push(bytes: &[u8], size: usize, cfg: Config) -> (Vec<String>, Result<(), Error>) {
    let mut tok = Tokenizer::new(cfg);
    let mut log = Log::default();
    let mut offsets = Vec::new();
    let chunks: Vec<&[u8]> = if bytes.is_empty() {
        vec![]
    } else {
        bytes.chunks(size).collect()
    };
    for (i, chunk) in chunks.iter().chain(std::iter::once(&&b""[..])).enumerate() {
        let eof = i >= chunks.len();
        match tok.feed(chunk, eof, &mut log) {
            Err(e) => return (log.0, Err(e)),
            Ok(true) => {
                log.0
                    .push(format!("offsets {offsets:?} end {}", tok.offset()));
                return (log.0, Ok(()));
            }
            Ok(false) => {
                assert!(!eof, "eof always completes or fails");
                offsets.push(tok.offset());
            }
        }
    }
    unreachable!("eof was fed")
}

const BYTES: &[u8] =
    b"{}[]:,\"\\/ubfnrt0123456789-e \n\x00\xc3\xa9\xe2\x82\xac\xf0\x9f\x98\x80\xed\xa0\xff";

fn config() -> impl Strategy<Value = Config> {
    (
        prop::sample::select(&[Framing::Single, Framing::Concatenated, Framing::Ndjson][..]),
        1usize..7,
        1usize..25,
    )
        .prop_map(|(framing, max_depth, max_number_len)| Config {
            framing,
            max_depth,
            max_number_len,
            ..Config::default()
        })
}

fn nested() -> impl Strategy<Value = serde_json::Value> {
    let leaf = prop_oneof![
        any::<i64>().prop_map(serde_json::Value::from),
        any::<f64>().prop_filter_map("finite", |f| serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)),
        "[a-z\"]{0,6}".prop_map(serde_json::Value::String),
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(serde_json::Value::Bool),
    ];
    leaf.prop_recursive(8, 32, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..3).prop_map(serde_json::Value::Array),
            prop::collection::vec(("[a-z]{1,3}", inner), 0..3)
                .prop_map(|kv| serde_json::Value::Object(kv.into_iter().collect())),
        ]
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3_000))]

    #[test]
    fn push_and_pull_agree_on_any_bytes(bytes in prop::collection::vec(prop::sample::select(BYTES), 0..64), size in 1usize..20, cfg in config()) {
        prop_assert_eq!(push(&bytes, size, cfg), pull(&bytes, size, cfg));
    }

    #[test]
    fn push_and_pull_agree_at_tight_limits(v in prop::collection::vec(nested(), 1..3), size in 1usize..40, cfg in config()) {
        let mut bytes = Vec::new();
        for d in &v {
            bytes.extend(serde_json::to_vec(d).unwrap());
            bytes.push(b'\n');
        }
        prop_assert_eq!(push(&bytes, size, cfg), pull(&bytes, size, cfg));
    }

    #[test]
    fn push_and_pull_agree_on_json_with_multibyte_text(s in "[a-zé€😀\"\\\\\n]{0,40}", size in 1usize..12) {
        let doc = serde_json::to_vec(&serde_json::json!({ "k€y": [s.clone(), {"x": s}] })).unwrap();
        prop_assert_eq!(push(&doc, size, Config::default()), pull(&doc, size, Config::default()));
        let (log, end) = push(&doc, size, Config::default());
        prop_assert!(end.is_ok());
        prop_assert!(!log.is_empty());
    }
}

#[test]
fn invalid_utf8_in_a_chunk_is_reported_where_pull_reports_it() {
    for doc in [
        &b"[\"ok\",\"a\xffb\"]"[..],
        b"[\"\xe2\x82\"]",
        b"[\"\xed\xa0\x80\"]",
        b"[1,\xc3\xa9]",
    ] {
        for size in [1, 2, 3, 64] {
            let (pushed, pulled) = (
                push(doc, size, Config::default()),
                pull(doc, size, Config::default()),
            );
            assert!(pushed.1.is_err());
            assert_eq!(pushed, pulled, "{doc:?} at {size}");
        }
    }
}

#[test]
fn limits_hold_through_the_fast_loop() {
    let tight = Config {
        max_depth: 1,
        ..Config::default()
    };
    let err = push(b"[[1]]", 64, tight).1.unwrap_err();
    assert_eq!(
        (err.kind(), err.offset()),
        (chunkjson::ErrorKind::DepthExceeded, 1)
    );
    assert!(push(b"[1]", 64, tight).1.is_ok());
    let short = Config {
        max_number_len: 2,
        ..Config::default()
    };
    let err = push(b"[123]", 64, short).1.unwrap_err();
    assert_eq!(
        (err.kind(), err.offset()),
        (chunkjson::ErrorKind::NumberTooLong, 3)
    );
    assert!(push(b"[12]", 64, short).1.is_ok());
}

#[test]
fn a_sink_error_inside_the_fast_loop_leaves_the_offset_after_the_refused_event() {
    struct StopAt(usize);
    impl Sink for StopAt {
        type Error = Error;
        fn event(&mut self, _: Event<'_>) -> Result<(), Error> {
            self.0 -= 1;
            if self.0 == 0 {
                return Err(Tokenizer::new(Config::default())
                    .next(b"x", true)
                    .unwrap_err());
            }
            Ok(())
        }
    }
    let doc = b"[1,2,3]";
    for (nth, offset) in [(2, 2), (3, 4), (4, 6), (5, 7)] {
        let mut tok = Tokenizer::new(Config::default());
        assert!(tok.feed(doc, true, &mut StopAt(nth)).is_err());
        assert_eq!(tok.offset(), offset, "stopped at event {nth}");
        // the caller can carry on from there
        let mut rest = Log::default();
        assert!(tok.feed(&doc[offset as usize..], true, &mut rest).unwrap());
    }
}

#[test]
fn a_sink_error_stops_the_feed() {
    struct Stop;
    impl Sink for Stop {
        type Error = Error;
        fn event(&mut self, _: Event<'_>) -> Result<(), Error> {
            Err(Tokenizer::new(Config::default())
                .next(b"x", true)
                .unwrap_err())
        }
    }
    let mut tok = Tokenizer::new(Config::default());
    assert!(tok.feed(b"[1]", true, &mut Stop).is_err());
    assert_eq!(tok.offset(), 1, "stopped after the first event");
}
