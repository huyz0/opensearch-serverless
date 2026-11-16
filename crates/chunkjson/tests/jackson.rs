//! Comments, positions and empty input: what OpenSearch's Jackson parser does too.
// ARITH: test inputs are small and fixed; an overflow would be a test bug, and debug builds panic on it.
#![allow(missing_docs, clippy::arithmetic_side_effects)]

mod common;

use chunkjson::{Config, Error, ErrorKind, Event, Framing, Position, Sink, Tokenizer};
use common::{feed_chunked, parse_chunked, position_of};
use proptest::prelude::*;
use serde_json::{Map, Number, Value};

fn leaf() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        any::<f64>().prop_filter_map("finite", |f| Number::from_f64(f).map(Value::Number)),
        "[ /*\"\\\\a-zé\\n]{0,8}".prop_map(Value::String),
    ]
}

fn value() -> impl Strategy<Value = Value> {
    leaf().prop_recursive(5, 48, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            prop::collection::vec(("[a-z/*]{0,4}", inner), 0..6)
                .prop_map(|kv| Value::Object(kv.into_iter().collect::<Map<_, _>>())),
        ]
    })
}

fn sizes() -> impl Strategy<Value = Vec<usize>> {
    prop_oneof![
        Just(vec![1]),
        Just(vec![usize::MAX]),
        prop::collection::vec(1usize..24, 1..8)
    ]
}

/// The indexes of whitespace bytes outside strings.
fn gaps(text: &[u8]) -> Vec<usize> {
    let (mut in_string, mut escaped) = (false, false);
    let mut out = Vec::new();
    for (i, &b) in text.iter().enumerate() {
        if in_string {
            match (escaped, b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
        } else if b == b'"' {
            in_string = true;
        } else if matches!(b, b' ' | b'\n') {
            out.push(i);
        }
    }
    out
}

const COMMENTS: [&str; 6] = [
    "/* c */",
    "// line\n",
    "/**/",
    "/***/",
    "/* a * / b ** */",
    "/* two\nlines */",
];

fn commented(text: &str, picks: &[(prop::sample::Index, usize)]) -> String {
    let at = gaps(text.as_bytes());
    let mut cuts: Vec<(usize, &str)> = if at.is_empty() {
        Vec::new()
    } else {
        picks
            .iter()
            .map(|(i, c)| (at[i.index(at.len())], COMMENTS[c % COMMENTS.len()]))
            .collect()
    };
    cuts.sort_unstable();
    cuts.dedup_by_key(|c| c.0);
    let mut out = String::new();
    let mut from = 0;
    for (i, c) in cuts {
        out.push_str(&text[from..i]);
        out.push_str(c);
        from = i;
    }
    out.push_str(&text[from..]);
    // a leading and a trailing comment too
    format!("/* head */{out}// tail")
}

fn with_comments() -> Config {
    Config {
        comments: true,
        ..Config::default()
    }
}

/// The start of every token, computed from the whole text, which may hold
/// the comments of [`COMMENTS`].
fn token_starts(text: &[u8]) -> Vec<Position> {
    let (mut line, mut line_start) = (1u64, 0usize);
    let (mut in_string, mut escaped, mut in_token) = (false, false, false);
    // inside `//` (Some(false)) or `/* */` (Some(true))
    let mut comment: Option<bool> = None;
    let mut out = Vec::new();
    let at = |i: usize, line: u64, line_start: usize| Position {
        offset: i as u64,
        line,
        column: (i - line_start + 1) as u64,
    };
    for (i, &b) in text.iter().enumerate() {
        if let Some(block) = comment {
            if b == b'\n' {
                line += 1;
                line_start = i + 1;
                if !block {
                    comment = None;
                }
            } else if block && b == b'/' && text[i - 1] == b'*' && text[i - 2..i] != *b"/*" {
                comment = None;
            }
            continue;
        }
        if !in_string && b == b'/' {
            comment = Some(text[i + 1] == b'*');
            in_token = false;
            continue;
        }
        if in_string {
            match (escaped, b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            continue;
        }
        let scalar_byte = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'+' | b'.');
        match b {
            b'\n' => {
                line += 1;
                line_start = i + 1;
                in_token = false;
            }
            b'{' | b'}' | b'[' | b']' => {
                out.push(at(i, line, line_start));
                in_token = false;
            }
            b'"' => {
                out.push(at(i, line, line_start));
                in_string = true;
                in_token = false;
            }
            _ if scalar_byte => {
                if !in_token {
                    out.push(at(i, line, line_start));
                }
                in_token = true;
            }
            _ => in_token = false,
        }
    }
    out
}

/// Checks one start the tokenizer reported, then keeps it: one per token, so
/// only a string's first fragment counts. `EndDocument` starts where its
/// document's first token does, at `doc` in `out`.
fn record(out: &mut Vec<Position>, doc: &mut usize, event: &Event<'_>, start: Position) {
    match event {
        Event::EndDocument { start: s, .. } => {
            assert_eq!(start.offset, *s);
            assert_eq!(
                Some(&start),
                out.get(*doc),
                "a document ends where it started"
            );
            *doc = out.len();
        }
        Event::Key(t) | Event::String(t) if !t.first => {
            assert_eq!(Some(&start), out.last(), "a later fragment keeps the start");
        }
        _ => out.push(start),
    }
}

/// Each token's start as the pull API reports it.
fn pull_starts(text: &[u8], sizes: &[usize], config: Config) -> Vec<Position> {
    let mut tok = Tokenizer::new(config);
    let (mut out, mut doc) = (Vec::new(), 0);
    let mut pos: usize = 0;
    let mut k = 0;
    loop {
        let end = pos
            .saturating_add(sizes[k % sizes.len()].max(1))
            .min(text.len());
        let mut chunk = &text[pos..end];
        loop {
            let eof = end == text.len();
            let (event, used, start) = tok.next_at(chunk, eof).unwrap();
            chunk = &chunk[used..];
            match event {
                Event::NeedMore => {
                    assert!(!eof, "never NeedMore once eof is set");
                    break;
                }
                Event::Eof => return out,
                _ => record(&mut out, &mut doc, &event, start),
            }
        }
        pos = end;
        k += 1;
    }
}

/// As [`pull_starts`], through the push API.
fn push_starts(text: &[u8], sizes: &[usize], config: Config) -> Vec<Position> {
    struct Starts(Vec<Position>, usize);
    impl Sink for Starts {
        type Error = Error;
        fn event(&mut self, _: Event<'_>) -> Result<(), Error> {
            unreachable!("event_at is overridden")
        }
        fn event_at(&mut self, event: Event<'_>, start: Position) -> Result<(), Error> {
            record(&mut self.0, &mut self.1, &event, start);
            Ok(())
        }
    }
    let mut tok = Tokenizer::new(config);
    let mut sink = Starts(Vec::new(), 0);
    let mut pos: usize = 0;
    let mut k = 0;
    while pos < text.len() {
        let end = pos
            .saturating_add(sizes[k % sizes.len()].max(1))
            .min(text.len());
        tok.feed(&text[pos..end], end == text.len(), &mut sink)
            .unwrap();
        pos = end;
        k += 1;
    }
    sink.0
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// AC-2
    #[test]
    fn comments_anywhere_between_tokens_change_no_event(
        v in value(),
        picks in prop::collection::vec((any::<prop::sample::Index>(), 0usize..6), 0..12),
        sizes in sizes(),
    ) {
        let text = serde_json::to_string_pretty(&v).unwrap();
        let plain = parse_chunked(text.as_bytes(), &[usize::MAX], Config::default()).unwrap();
        let noisy = commented(&text, &picks);
        let pulled = parse_chunked(noisy.as_bytes(), &sizes, with_comments()).unwrap();
        let pushed = feed_chunked(noisy.as_bytes(), &sizes, with_comments()).unwrap();
        prop_assert_eq!(&pulled.docs, &plain.docs);
        prop_assert_eq!(&pushed.docs, &plain.docs);
    }

    /// AC-3: on one document or several, with comments between tokens.
    #[test]
    fn every_event_starts_where_its_token_starts(
        docs in prop::collection::vec(value(), 1..4),
        picks in prop::collection::vec((any::<prop::sample::Index>(), 0usize..6), 0..12),
        sizes in sizes(),
        compact in any::<bool>(),
    ) {
        let text = docs
            .iter()
            .map(|v| if compact {
                serde_json::to_string(v).unwrap()
            } else {
                serde_json::to_string_pretty(v).unwrap()
            })
            .collect::<Vec<_>>()
            .join("\n");
        let text = commented(&text, &picks);
        let config = Config {
            comments: true,
            ..Config::with_framing(Framing::Concatenated)
        };
        let want = token_starts(text.as_bytes());
        prop_assert_eq!(pull_starts(text.as_bytes(), &sizes, config), want.clone());
        prop_assert_eq!(push_starts(text.as_bytes(), &sizes, config), want);
    }

    /// AC-3: an error's line and column match its offset.
    #[test]
    fn an_error_is_placed_at_its_line_and_column(
        v in value(),
        at in any::<prop::sample::Index>(),
        byte in prop::sample::select(b"}]:,x\"\x01/".to_vec()),
        sizes in sizes(),
    ) {
        let mut text = serde_json::to_string_pretty(&v).unwrap().into_bytes();
        let i = at.index(text.len());
        text[i] = byte;
        for e in [
            parse_chunked(&text, &sizes, Config::default()).err(),
            feed_chunked(&text, &sizes, Config::default()).err(),
        ]
        .into_iter()
        .flatten()
        {
            prop_assert_eq!((e.line(), e.column()), position_of(&text, e.offset()), "{}", e);
        }
    }
}

fn first_error(text: &[u8], config: Config) -> Error {
    let pulled = parse_chunked(text, &[usize::MAX], config).unwrap_err();
    let pushed = feed_chunked(text, &[1], config).unwrap_err();
    assert_eq!(pulled, pushed);
    pulled
}

/// AC-2: what comments refuse.
#[test]
fn comment_faults_are_typed_and_placed() {
    // the slash is named, wherever the next byte arrives
    let e = first_error(b"[1 /x]", with_comments());
    assert_eq!((e.kind(), e.offset()), (ErrorKind::UnexpectedByte(b'/'), 3));
    assert_eq!((e.line(), e.column()), (1, 4));
    let e = first_error(b"[1 /* never closed", with_comments());
    assert_eq!(e.kind(), ErrorKind::UnexpectedEof);
    let e = first_error(b"[1 /", with_comments());
    assert_eq!(e.kind(), ErrorKind::UnexpectedEof);
    let e = first_error(b"[1 /**/]", Config::default());
    assert_eq!((e.kind(), e.offset()), (ErrorKind::UnexpectedByte(b'/'), 3));
    let e = first_error(b"\"a\" /* x */ 2", with_comments());
    assert_eq!(e.kind(), ErrorKind::TrailingData);
    // a line comment may end the input
    assert!(parse_chunked(b"[1] // done", &[1], with_comments()).is_ok());
    // a comment ends a number or literal as whitespace does
    let b = parse_chunked(b"[1/**/,true//x\n]", &[1], with_comments()).unwrap();
    assert_eq!(b.docs, vec![serde_json::json!([1, true])]);
}

#[test]
fn a_line_comment_in_ndjson_ends_at_the_newline() {
    let config = Config {
        comments: true,
        ..Config::with_framing(Framing::Ndjson)
    };
    let b = feed_chunked(b"1 // one\n2\n", &[1], config).unwrap();
    assert_eq!(b.docs, vec![serde_json::json!(1), serde_json::json!(2)]);
    // a block comment may not span a line break, in a document or between two
    for text in [
        &b"[1, /* a\n */ 2]\n"[..],
        b"{\"a\":1} /*\n*/ {\"b\":2}\n",
        b"{\"a\":1}\n/*\n*/\n{\"b\":2}\n",
    ] {
        let e = first_error(text, config);
        assert_eq!(e.kind(), ErrorKind::NewlineInDocument);
    }
}

/// AC-4
#[test]
fn empty_input_is_allowed_only_when_asked() {
    let allow = Config {
        allow_empty: true,
        comments: true,
        ..Config::default()
    };
    for text in [&b""[..], b"  \n\t", b"/* nothing */ // here"] {
        let b = parse_chunked(text, &[1], allow).unwrap();
        assert!(b.docs.is_empty());
        let b = feed_chunked(text, &[1], allow).unwrap();
        assert!(b.docs.is_empty());
        let e = first_error(
            text,
            Config {
                allow_empty: false,
                ..allow
            },
        );
        assert_eq!(e.kind(), ErrorKind::UnexpectedEof);
    }
    // a document still has to be complete
    assert_eq!(first_error(b"[", allow).kind(), ErrorKind::UnexpectedEof);
    assert!(parse_chunked(b" 7 ", &[1], allow).is_ok());
}

#[test]
fn a_document_ends_where_it_started_its_first_line() {
    let text = b"\n  {\"a\":\n [1]}\n";
    let mut tok = Tokenizer::new(Config::default());
    let mut chunk = &text[..];
    loop {
        let (event, used) = tok.next(chunk, true).unwrap();
        chunk = &chunk[used..];
        if let Event::EndDocument { .. } = event {
            let at = tok.event_start();
            assert_eq!((at.offset, at.line, at.column), (3, 2, 3));
            break;
        }
    }
}

#[test]
fn a_line_break_inside_a_block_comment_starts_a_line() {
    let text = b"/* a\n */ [1]";
    let mut tok = Tokenizer::new(with_comments());
    let (event, _, at) = tok.next_at(text, true).unwrap();
    assert_eq!(event, Event::BeginArray);
    assert_eq!(
        at,
        Position {
            offset: 9,
            line: 2,
            column: 5
        }
    );
}

#[test]
fn an_error_after_line_breaks_names_its_line() {
    let e = first_error(b"{\n  \"a\": 1,\n  \"b\" 2\n}", Config::default());
    assert_eq!((e.line(), e.column()), (3, 7));
    assert_eq!(e.kind(), ErrorKind::UnexpectedByte(b'2'));
}

/// AC-5: the events after `reset` are a new tokenizer's.
#[test]
fn reset_gives_a_fresh_tokenizer() {
    let mut tok = Tokenizer::new(with_comments());
    let _ = tok.next(b"[1, {\"a\":", false).unwrap();
    let _ = tok.next(b"x", false);
    tok.reset();
    assert_eq!(tok.offset(), 0);
    assert_eq!(tok.depth(), 0);
    let mut events = Vec::new();
    let mut chunk = &b"/**/\n[2]"[..];
    loop {
        let (event, used) = tok.next(chunk, true).unwrap();
        chunk = &chunk[used..];
        if event == Event::Eof {
            break;
        }
        events.push((format!("{event:?}"), tok.event_start()));
    }
    let mut fresh = Tokenizer::new(with_comments());
    let mut want = Vec::new();
    let mut chunk = &b"/**/\n[2]"[..];
    loop {
        let (event, used) = fresh.next(chunk, true).unwrap();
        chunk = &chunk[used..];
        if event == Event::Eof {
            break;
        }
        want.push((format!("{event:?}"), fresh.event_start()));
    }
    assert_eq!(events, want);
    assert_eq!(
        events[0].1,
        Position {
            offset: 5,
            line: 2,
            column: 1
        }
    );
}
