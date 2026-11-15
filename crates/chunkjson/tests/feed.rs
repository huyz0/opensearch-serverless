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
                Ok((Event::NeedMore, _)) => break,
                Ok((Event::Eof, _)) => return (log, Ok(())),
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
    let chunks: Vec<&[u8]> = if bytes.is_empty() {
        vec![]
    } else {
        bytes.chunks(size).collect()
    };
    for (i, chunk) in chunks.iter().chain(std::iter::once(&&b""[..])).enumerate() {
        let eof = i >= chunks.len();
        match tok.feed(chunk, eof, &mut log) {
            Err(e) => return (log.0, Err(e)),
            Ok(true) => return (log.0, Ok(())),
            Ok(false) => assert!(!eof, "eof always completes or fails"),
        }
    }
    unreachable!("eof was fed")
}

const BYTES: &[u8] =
    b"{}[]:,\"\\/ubfnrt0123456789-e \n\x00\xc3\xa9\xe2\x82\xac\xf0\x9f\x98\x80\xed\xa0\xff";

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3_000))]

    #[test]
    fn push_and_pull_agree_on_any_bytes(bytes in prop::collection::vec(prop::sample::select(BYTES), 0..64), size in 1usize..20, ndjson in any::<bool>()) {
        let cfg = Config::with_framing(if ndjson { Framing::Ndjson } else { Framing::Concatenated });
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
