//! Tests; compiled only under `#[cfg(test)]` from the parent module.

#[cfg(test)]
use proptest::prelude::*;
use serde_json::{Map, Value};

use super::*;

/// Rebuilds a `serde_json::Value` from tokens, checking piece sizes.
#[derive(Debug, Default)]
struct Build {
    stack: Vec<(Value, Option<String>)>,
    key: Option<String>,
    text: String,
    done: Option<Value>,
    max_piece: usize,
    pieces: usize,
}

impl Build {
    fn put(&mut self, v: Value) {
        match self.stack.last_mut() {
            Some((Value::Array(a), _)) => a.push(v),
            Some((Value::Object(o), _)) => {
                let k = self.key.take().expect("a key before an object value");
                o.insert(k, v);
            }
            _ => self.done = Some(v),
        }
    }
}

impl TokenSink for Build {
    type Error = JsonError;

    fn token(&mut self, t: Token<'_>) -> Result<(), JsonError> {
        match t {
            Token::StartObject => self
                .stack
                .push((Value::Object(Map::new()), self.key.take())),
            Token::StartArray => self.stack.push((Value::Array(Vec::new()), self.key.take())),
            Token::EndObject | Token::EndArray => {
                let (v, key) = self.stack.pop().expect("balanced");
                self.key = key;
                self.put(v);
            }
            Token::Key(k) => self.key = Some(k.to_owned()),
            Token::Str { text, last } => {
                self.max_piece = self.max_piece.max(text.len());
                self.pieces = self.pieces.saturating_add(1);
                self.text.push_str(text);
                if last {
                    let s = std::mem::take(&mut self.text);
                    self.put(Value::String(s));
                }
            }
            Token::Number(n) => self.put(serde_json::from_str(n).expect("a valid number")),
            Token::Bool(b) => self.put(Value::Bool(b)),
            Token::Null => self.put(Value::Null),
        }
        Ok(())
    }
}

fn limits(chunk: usize) -> Limits {
    Limits {
        string_chunk: chunk,
        ..Limits::default()
    }
}

fn parse_chunked(
    doc: &[u8],
    feed: usize,
    lim: Limits,
) -> Result<(Option<Value>, Build), JsonError> {
    let mut t = Tokenizer::new(lim);
    let mut b = Build::default();
    for chunk in doc.chunks(feed.max(1)) {
        t.feed(chunk, &mut b)?;
    }
    let c = t.finish(&mut b)?;
    assert_eq!(c == Completion::Value, b.done.is_some());
    Ok((b.done.take(), b))
}

fn parse(doc: &str) -> Result<Option<Value>, JsonError> {
    parse_chunked(doc.as_bytes(), doc.len(), Limits::default()).map(|(v, _)| v)
}

fn kind(doc: &str) -> ErrorKind {
    parse(doc).expect_err(doc).kind
}

fn arb_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        any::<f64>()
            .prop_filter("finite", |f| f.is_finite())
            .prop_map(Value::from),
        ".{0,40}".prop_map(Value::String),
    ];
    leaf.prop_recursive(5, 64, 8, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
            prop::collection::btree_map(".{0,12}", inner, 0..8)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

proptest! {
    #[test]
    fn any_document_fed_in_any_chunks_matches_serde_json(v in arb_json(), feed in 1usize..64, piece in 4usize..32, pretty in any::<bool>()) {
        let text = if pretty { serde_json::to_string_pretty(&v) } else { serde_json::to_string(&v) }.expect("serialises");
        let (got, b) = parse_chunked(text.as_bytes(), feed, limits(piece)).expect("parses");
        // serde_json reads floats back to the nearest value only with its
        // `float_roundtrip` feature, so compare with its reading of the text.
        let want: Value = serde_json::from_str(&text).expect("serde_json reads it");
        prop_assert_eq!(got, Some(want));
        prop_assert!(b.max_piece <= piece, "piece {} over {}", b.max_piece, piece);
    }
}

#[test]
fn scalars_at_the_root_and_empty_documents() {
    assert_eq!(parse("42").expect("ok"), Some(serde_json::json!(42)));
    assert_eq!(
        parse(" -1.5e3 ").expect("ok"),
        Some(serde_json::json!(-1500.0))
    );
    assert_eq!(parse("\"x\"").expect("ok"), Some(serde_json::json!("x")));
    assert_eq!(parse("null").expect("ok"), Some(Value::Null));
    assert_eq!(parse("").expect("ok"), None);
    assert_eq!(parse("  // only a comment").expect("ok"), None);
}

#[test]
fn comments_are_whitespace_as_jackson_allows() {
    let doc = "/* lead */ {\"a\" /* in */ : // line\n [1, /**/ 2 /* * / */]}\n// tail";
    assert_eq!(
        parse(doc).expect("ok"),
        Some(serde_json::json!({"a": [1, 2]}))
    );
    assert_eq!(kind("{} / x"), ErrorKind::Unexpected(b'/'));
    assert_eq!(kind("{} /* open"), ErrorKind::UnexpectedEnd);
    assert_eq!(kind("1/"), ErrorKind::UnexpectedEnd);
}

#[test]
fn escapes_and_surrogate_pairs_decode() {
    let doc = r#"["\"\\\/\b\f\n\r\t", "é€", "😀"]"#;
    assert_eq!(
        parse(doc).expect("ok"),
        Some(serde_json::json!(["\"\\/\u{8}\u{c}\n\r\t", "é€", "😀"]))
    );
    assert_eq!(kind(r#""\ud83d""#), ErrorKind::LoneSurrogate);
    assert_eq!(kind(r#""\ud83dx""#), ErrorKind::LoneSurrogate);
    assert_eq!(kind(r#""\ud83d\n""#), ErrorKind::LoneSurrogate);
    assert_eq!(kind(r#""\ud83dA""#), ErrorKind::LoneSurrogate);
    assert_eq!(kind(r#""\ude00""#), ErrorKind::LoneSurrogate);
    assert_eq!(kind(r#""\x""#), ErrorKind::BadEscape);
    assert_eq!(kind(r#""\u12g4""#), ErrorKind::BadEscape);
}

#[test]
fn utf8_split_across_feeds_and_pieces_stays_whole() {
    let s = "é😀€".repeat(50);
    let doc = format!("\"{s}\"");
    for feed in 1..8 {
        for piece in 4..9 {
            let (got, b) = parse_chunked(doc.as_bytes(), feed, limits(piece)).expect("ok");
            assert_eq!(got, Some(Value::String(s.clone())));
            assert!(b.max_piece <= piece);
        }
    }
    assert_eq!(
        parse_chunked(b"\"\xff\"", 8, Limits::default())
            .expect_err("bad")
            .kind,
        ErrorKind::InvalidUtf8
    );
    assert_eq!(
        parse_chunked(b"\"ab\xff\xfe\"", 8, limits(4))
            .expect_err("bad")
            .kind,
        ErrorKind::InvalidUtf8
    );
    assert_eq!(
        parse_chunked(b"{\"\xff\":1}", 8, Limits::default())
            .expect_err("bad")
            .kind,
        ErrorKind::InvalidUtf8
    );
}

#[test]
fn malformed_documents_are_refused_with_positions() {
    let e = parse("{\n  \"a\": tru }").expect_err("bad literal");
    assert_eq!(
        (e.kind, e.line, e.column, e.offset),
        (ErrorKind::Unexpected(b' '), 2, 11, 12)
    );
    assert_eq!(
        e.to_string(),
        "Unexpected character (code 32) at line: 2, column: 11"
    );
    assert_eq!(kind("{\"a\" 1}"), ErrorKind::Unexpected(b'1'));
    assert_eq!(kind("{1:2}"), ErrorKind::Unexpected(b'1'));
    assert_eq!(kind("[1,]"), ErrorKind::Unexpected(b']'));
    assert_eq!(kind("[1 2]"), ErrorKind::Unexpected(b'2'));
    assert_eq!(kind("{\"a\":1,}"), ErrorKind::Unexpected(b'}'));
    assert_eq!(kind("[}"), ErrorKind::Unexpected(b'}'));
    assert_eq!(kind("{]"), ErrorKind::Unexpected(b']'));
    assert_eq!(kind("[1}"), ErrorKind::Unexpected(b'}'));
    assert_eq!(kind("{\"a\":1]"), ErrorKind::Unexpected(b']'));
    assert_eq!(kind("{} {}"), ErrorKind::Trailing(b'{'));
    assert_eq!(kind("01"), ErrorKind::BadNumber);
    assert_eq!(kind("[-]"), ErrorKind::BadNumber);
    assert_eq!(kind("[1.e5]"), ErrorKind::BadNumber);
    assert_eq!(kind("[\"a\nb\"]"), ErrorKind::ControlInString(b'\n'));
    assert_eq!(kind("{\"a\":"), ErrorKind::UnexpectedEnd);
    assert_eq!(kind("[\"abc"), ErrorKind::UnexpectedEnd);
    assert_eq!(kind("@"), ErrorKind::Unexpected(b'@'));
    assert_eq!(kind("nul"), ErrorKind::UnexpectedEnd);
    assert_eq!(kind("nulx"), ErrorKind::Unexpected(b'x'));
    assert!(kind("{\"a\": falsy}") == ErrorKind::Unexpected(b'y'));
}

#[test]
fn an_error_sticks_until_reset_and_reset_reuses_the_tokenizer() {
    let mut t = Tokenizer::new(Limits::default());
    let mut b = Build::default();
    assert!(t.feed(b"[1,,", &mut b).is_err());
    assert!(t.feed(b"2]", &mut b).is_err(), "still failed");
    assert!(t.finish(&mut b).is_err());
    t.reset();
    let mut b = Build::default();
    t.feed(b"[true, false]", &mut b).expect("fresh");
    assert_eq!(t.offset(), 13);
    assert_eq!(t.finish(&mut b).expect("done"), Completion::Value);
    assert_eq!(b.done, Some(serde_json::json!([true, false])));
}

fn nested(depth: usize) -> String {
    format!("{}{}", "[".repeat(depth), "]".repeat(depth))
}

#[test]
fn the_depth_limit_holds_at_max_and_refuses_one_more() {
    let lim = Limits {
        max_depth: 8,
        ..Limits::default()
    };
    assert!(parse_chunked(nested(8).as_bytes(), 3, lim).is_ok());
    assert_eq!(
        parse_chunked(nested(9).as_bytes(), 3, lim)
            .expect_err("too deep")
            .kind,
        ErrorKind::TooDeep
    );
    assert!(parse(&nested(1000)).is_ok(), "Jackson's default depth");
    assert_eq!(kind(&nested(1001)), ErrorKind::TooDeep);
}

#[test]
fn the_key_limit_holds_at_max_and_refuses_one_more() {
    let lim = Limits {
        max_key_bytes: 8,
        ..Limits::default()
    };
    let at = format!("{{\"{}\":1}}", "k".repeat(8));
    let over = format!("{{\"{}\":1}}", "k".repeat(9));
    assert!(parse_chunked(at.as_bytes(), 2, lim).is_ok());
    assert_eq!(
        parse_chunked(over.as_bytes(), 2, lim)
            .expect_err("long key")
            .kind,
        ErrorKind::KeyTooLong
    );
}

#[test]
fn the_number_limit_holds_at_max_and_refuses_one_more() {
    let lim = Limits {
        max_number_bytes: 6,
        ..Limits::default()
    };
    assert!(parse_chunked(b"[123456]", 1, lim).is_ok());
    assert_eq!(
        parse_chunked(b"[1234567]", 1, lim).expect_err("long").kind,
        ErrorKind::NumberTooLong
    );
}

#[test]
fn the_string_limit_holds_at_max_and_refuses_one_more() {
    let lim = Limits {
        max_string_bytes: 10,
        string_chunk: 4,
        ..Limits::default()
    };
    let (v, b) = parse_chunked(b"\"0123456789\"", 3, lim).expect("exactly the limit");
    assert_eq!(v, Some(serde_json::json!("0123456789")));
    assert!(b.pieces >= 3 && b.max_piece <= 4);
    assert_eq!(
        parse_chunked(b"\"0123456789a\"", 3, lim)
            .expect_err("long")
            .kind,
        ErrorKind::StringTooLong
    );
    // An escape counts too.
    assert_eq!(
        parse_chunked(b"\"0123456789\\n\"", 3, lim)
            .expect_err("long")
            .kind,
        ErrorKind::StringTooLong
    );
}

#[test]
fn pieces_fill_across_feeds() {
    let s = "aé😀".repeat(400);
    let doc = format!("\"{s}\"");
    for (feed, piece) in [(1, 4), (7, 16), (100, 64), (5000, 1000)] {
        let mut t = Tokenizer::new(limits(piece));
        let mut sizes = Pieces(Vec::new());
        for chunk in doc.as_bytes().chunks(feed) {
            t.feed(chunk, &mut sizes).expect("feeds");
        }
        t.finish(&mut sizes).expect("ends");
        let (last, full) = sizes.0.split_last().expect("pieces");
        assert!(last.1, "the last piece says so");
        // a piece ends on a character boundary, at most 3 bytes short
        assert!(
            full.iter()
                .all(|(t, l)| !l && t.len() <= piece && t.len() + 3 >= piece),
            "{feed} {piece}"
        );
        assert_eq!(sizes.0.iter().map(|p| p.0.as_str()).collect::<String>(), s);
    }
}

/// Each string piece and its `last`.
struct Pieces(Vec<(String, bool)>);

impl TokenSink for Pieces {
    type Error = JsonError;
    fn token(&mut self, t: Token<'_>) -> Result<(), JsonError> {
        if let Token::Str { text, last } = t {
            self.0.push((text.to_owned(), last));
        }
        Ok(())
    }
}

#[test]
fn a_piece_too_small_for_a_character_is_refused() {
    let mut t = Tokenizer::new(limits(3));
    let mut b = Build::default();
    assert_eq!(
        t.feed(b"1", &mut b).expect_err("bad limits").kind,
        ErrorKind::BadLimits
    );
    assert!(t.finish(&mut b).is_err());
    t.reset();
    assert!(t.feed(b"1", &mut b).is_err(), "still bad after reset");
}

#[test]
fn every_error_message_names_its_position() {
    let at = |kind| {
        JsonError {
            kind,
            offset: 0,
            line: 3,
            column: 4,
        }
        .to_string()
    };
    for k in [
        ErrorKind::Unexpected(b'x'),
        ErrorKind::UnexpectedEnd,
        ErrorKind::Trailing(b'}'),
        ErrorKind::BadNumber,
        ErrorKind::BadEscape,
        ErrorKind::LoneSurrogate,
        ErrorKind::InvalidUtf8,
        ErrorKind::ControlInString(1),
        ErrorKind::TooDeep,
        ErrorKind::KeyTooLong,
        ErrorKind::StringTooLong,
        ErrorKind::NumberTooLong,
        ErrorKind::BadLimits,
    ] {
        assert!(at(k).ends_with("at line: 3, column: 4"), "{k:?}");
    }
    assert!(at(ErrorKind::Unexpected(b'x')).starts_with("Unexpected character ('x' (code 120))"));
    let e: crate::error::RestError = JsonError {
        kind: ErrorKind::BadNumber,
        offset: 0,
        line: 1,
        column: 1,
    }
    .into();
    assert_eq!(e.status(), 400);
    assert_eq!(
        e.as_exception().map(|x| x.kind()),
        Some("json_parse_exception")
    );
}

/// A sink whose error stops the feed.
struct Stop;

impl TokenSink for Stop {
    type Error = String;
    fn token(&mut self, _: Token<'_>) -> Result<(), String> {
        Err("stop".to_owned())
    }
}

impl From<JsonError> for String {
    fn from(e: JsonError) -> String {
        e.to_string()
    }
}

#[test]
fn a_sink_error_stops_the_feed() {
    let mut t = Tokenizer::new(Limits::default());
    assert_eq!(t.feed(b"[1]", &mut Stop), Err("stop".to_owned()));
}

/// Collects each token's kind and where it starts.
struct Located(Vec<(String, u64, u64)>);

impl TokenSink for Located {
    type Error = JsonError;

    fn token(&mut self, _: Token<'_>) -> Result<(), JsonError> {
        unreachable!("the tokenizer always gives a location")
    }

    fn token_at(&mut self, token: Token<'_>, at: super::Location) -> Result<(), JsonError> {
        let name = match token {
            Token::StartObject => "{".to_owned(),
            Token::EndObject => "}".to_owned(),
            Token::StartArray => "[".to_owned(),
            Token::EndArray => "]".to_owned(),
            Token::Key(k) => format!("key {k}"),
            Token::Str { text, .. } => format!("str {text}"),
            Token::Number(n) => format!("num {n}"),
            Token::Bool(b) => format!("bool {b}"),
            Token::Null => "null".to_owned(),
        };
        self.0.push((name, at.line, at.column));
        Ok(())
    }
}

#[test]
fn every_token_starts_where_jackson_locates_it() {
    let doc =
        "{\"query\":{\"term\":{\"tag\":{\"value\":\"x\",\"nope\":1}}},\n  \"a\": [true, null]}";
    // One byte at a time, so a position never depends on a chunk boundary.
    let mut t = Tokenizer::new(limits(4));
    let mut sink = Located(Vec::new());
    for b in doc.as_bytes() {
        t.feed(std::slice::from_ref(b), &mut sink).expect("feeds");
    }
    t.finish(&mut sink).expect("finishes");
    let at = |name: &str| {
        sink.0
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, l, c)| (*l, *c))
    };
    assert_eq!(at("{"), Some((1, 1)));
    assert_eq!(at("key query"), Some((1, 2)));
    assert_eq!(at("key nope"), Some((1, 38)));
    assert_eq!(at("num 1"), Some((1, 45)));
    assert_eq!(at("str x"), Some((1, 34)));
    assert_eq!(at("key a"), Some((2, 3)));
    assert_eq!(at("["), Some((2, 8)));
    assert_eq!(at("bool true"), Some((2, 9)));
    assert_eq!(at("null"), Some((2, 15)));
    assert_eq!(at("]"), Some((2, 19)));
    assert_eq!(
        sink.0.last().map(|(n, l, c)| (n.as_str(), *l, *c)),
        Some(("}", 2, 20))
    );
}
