//! The writer against serde_json as the oracle.
// ARITH: test inputs are small and fixed; an overflow would be a test bug, and debug builds panic on it.
#![allow(missing_docs, clippy::arithmetic_side_effects)]

mod common;

use chunkjson::{Config, Error, Event, Framing, Misuse, Sink, Text, Tokenizer, WriteError, Writer};
use common::Builder;
use proptest::prelude::*;
use serde_json::{Map, Number, Value};

fn leaf() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        any::<u64>().prop_map(Value::from),
        any::<f64>().prop_filter_map("finite", |f| Number::from_f64(f).map(Value::Number)),
        any::<String>().prop_map(Value::String),
        "[\\x00-\\x1f\\x7f\"\\\\/a-zé€😀]{0,12}".prop_map(Value::String),
    ]
}

fn value() -> impl Strategy<Value = Value> {
    leaf().prop_recursive(6, 64, 8, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
            prop::collection::vec((any::<String>(), inner), 0..8)
                .prop_map(|kv| Value::Object(kv.into_iter().collect::<Map<_, _>>())),
        ]
    })
}

/// serde_json's output with `\u00xx` escapes in upper case, as Jackson writes them.
fn jackson_case(serde: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(serde.len());
    let mut i = 0;
    while i < serde.len() {
        out.push(serde[i]);
        if serde[i] == b'\\' {
            let next = serde[i + 1];
            out.push(next);
            i += 2;
            if next == b'u' {
                out.extend(serde[i..i + 4].iter().map(u8::to_ascii_uppercase));
                i += 4;
            }
            continue;
        }
        i += 1;
    }
    out
}

/// Forwards tokenizer events into a writer.
struct Pipe<'a> {
    writer: &'a mut Writer,
    out: &'a mut Vec<u8>,
}

impl Sink for Pipe<'_> {
    type Error = Error;
    fn event(&mut self, event: Event<'_>) -> Result<(), Error> {
        self.writer
            .event(event, self.out)
            .expect("tokenizer events are well formed");
        Ok(())
    }
}

fn rewrite(bytes: &[u8], chunk: usize, framing: Framing) -> Vec<u8> {
    let config = Config::with_framing(framing);
    let mut tok = Tokenizer::new(config);
    let mut writer = Writer::new(config);
    let mut out = Vec::new();
    let chunks: Vec<&[u8]> = bytes.chunks(chunk.max(1)).collect();
    for (i, c) in chunks.iter().enumerate() {
        let mut pipe = Pipe {
            writer: &mut writer,
            out: &mut out,
        };
        tok.feed(c, i + 1 == chunks.len(), &mut pipe).unwrap();
    }
    if chunks.is_empty() {
        let mut pipe = Pipe {
            writer: &mut writer,
            out: &mut out,
        };
        tok.feed(b"", true, &mut pipe).unwrap();
    }
    writer.check_complete().expect("complete");
    out
}

/// One writer call, for the model test.
#[derive(Debug, Clone)]
enum Op {
    BeginObject,
    EndObject,
    BeginArray,
    EndArray,
    Key(String),
    Str(String),
    Fragment {
        key: bool,
        first: bool,
        last: bool,
        text: String,
    },
    Number(String),
    U64(u64),
    I64(i64),
    Bool(bool),
    Null,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => Just(Op::BeginObject),
        3 => Just(Op::EndObject),
        3 => Just(Op::BeginArray),
        3 => Just(Op::EndArray),
        3 => "[a-z\"\\\\\\x01é]{0,4}".prop_map(Op::Key),
        2 => "[a-z\"\\\\\\x01é]{0,4}".prop_map(Op::Str),
        2 => (any::<bool>(), any::<bool>(), any::<bool>(), "[a-zé\\n]{0,3}")
            .prop_map(|(key, first, last, text)| Op::Fragment { key, first, last, text }),
        2 => prop_oneof![
            Just("0"), Just("-1.5e3"), Just("12"), Just("01"), Just("1."), Just("-"),
            Just(""), Just("1e"), Just("x"), Just("1 "),
        ].prop_map(|s| Op::Number(s.to_owned())),
        1 => any::<u64>().prop_map(Op::U64),
        1 => any::<i64>().prop_map(Op::I64),
        1 => any::<bool>().prop_map(Op::Bool),
        1 => Just(Op::Null),
    ]
}

fn apply(w: &mut Writer, op: &Op, out: &mut Vec<u8>) -> Result<(), Misuse> {
    let r = match op {
        Op::BeginObject => w.begin_object(out),
        Op::EndObject => w.end_object(out),
        Op::BeginArray => w.begin_array(out),
        Op::EndArray => w.end_array(out),
        Op::Key(s) => w.key(s, out),
        Op::Str(s) => w.string(s, out),
        Op::Fragment {
            key,
            first,
            last,
            text,
        } => {
            let t = Text {
                fragment: text,
                first: *first,
                last: *last,
            };
            w.event(
                if *key {
                    Event::Key(t)
                } else {
                    Event::String(t)
                },
                out,
            )
        }
        Op::Number(s) => w.number(s, out),
        Op::U64(n) => w.u64(*n, out),
        Op::I64(n) => w.i64(*n, out),
        Op::Bool(b) => w.bool(*b, out),
        Op::Null => w.null(out),
    };
    r.map_err(|e| match e {
        WriteError::Misuse(m) => m,
        WriteError::Output(never) => match never {},
    })
}

/// The event an accepted call stands for, as the tokenizer would yield it.
fn as_event(op: &Op) -> Event<'_> {
    let whole = |fragment| Text {
        fragment,
        first: true,
        last: true,
    };
    match op {
        Op::BeginObject => Event::BeginObject,
        Op::EndObject => Event::EndObject,
        Op::BeginArray => Event::BeginArray,
        Op::EndArray => Event::EndArray,
        Op::Key(s) => Event::Key(whole(s)),
        Op::Str(s) => Event::String(whole(s)),
        Op::Fragment {
            key,
            first,
            last,
            text,
        } => {
            let t = Text {
                fragment: text,
                first: *first,
                last: *last,
            };
            if *key {
                Event::Key(t)
            } else {
                Event::String(t)
            }
        }
        Op::Number(s) => Event::Number(s),
        Op::Bool(b) => Event::Bool(*b),
        Op::Null => Event::Null,
        Op::U64(_) | Op::I64(_) => unreachable!("handled by the caller"),
    }
}

/// Fails unless `bytes` could begin valid JSON in `framing`.
fn assert_valid_prefix(bytes: &[u8], framing: Framing) {
    let mut tok = Tokenizer::new(Config::with_framing(framing));
    struct Drop;
    impl Sink for Drop {
        type Error = Error;
        fn event(&mut self, _: Event<'_>) -> Result<(), Error> {
            Ok(())
        }
    }
    if let Err(e) = tok.feed(bytes, false, &mut Drop) {
        panic!("{e} in {:?}", String::from_utf8_lossy(bytes));
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// AC-1
    #[test]
    fn rewriting_tokenizer_events_gives_serde_json_bytes(
        v in value(),
        chunk in prop_oneof![Just(1usize), Just(usize::MAX), 2usize..64],
    ) {
        let serde = serde_json::to_vec(&v).unwrap();
        prop_assert_eq!(
            String::from_utf8(rewrite(&serde, chunk, Framing::Single)).unwrap(),
            String::from_utf8(jackson_case(&serde)).unwrap()
        );
    }

    /// AC-1, NDJSON and concatenated framing: one value per line.
    #[test]
    fn rewriting_ndjson_keeps_one_value_per_line(
        vs in prop::collection::vec(value(), 0..5),
        chunk in prop_oneof![Just(1usize), Just(usize::MAX), 2usize..64],
    ) {
        let mut serde = Vec::new();
        for v in &vs {
            serde.extend(serde_json::to_vec(v).unwrap());
            serde.push(b'\n');
        }
        prop_assert_eq!(rewrite(&serde, chunk, Framing::Ndjson), jackson_case(&serde));
        prop_assert_eq!(rewrite(&serde, chunk, Framing::Concatenated), jackson_case(&serde));
    }

    /// AC-2
    #[test]
    fn any_call_sequence_writes_only_valid_json(
        ops in prop::collection::vec(op(), 0..40),
        framing in prop_oneof![Just(Framing::Single), Just(Framing::Ndjson), Just(Framing::Concatenated)],
    ) {
        let mut w = Writer::new(Config { max_depth: 4, ..Config::with_framing(framing) });
        let mut out = Vec::new();
        let mut model = Builder::default();
        for op in &ops {
            let before = out.len();
            match apply(&mut w, op, &mut out) {
                Ok(()) => {
                    match op {
                        Op::U64(n) => { model.apply(Event::Number(&n.to_string())); }
                        Op::I64(n) => { model.apply(Event::Number(&n.to_string())); }
                        _ => { model.apply(as_event(op)); }
                    }
                    assert_valid_prefix(&out, framing);
                }
                Err(_) => prop_assert_eq!(out.len(), before, "a refused call wrote"),
            }
        }
        if w.check_complete().is_ok() {
            let docs: Vec<Value> = serde_json::Deserializer::from_slice(&out)
                .into_iter()
                .collect::<Result<_, _>>()
                .unwrap();
            prop_assert_eq!(docs, model.docs);
        }
    }

    /// AC-3
    #[test]
    fn a_string_in_fragments_writes_the_same_bytes(
        s in any::<String>(),
        cuts in prop::collection::vec(any::<prop::sample::Index>(), 0..6),
    ) {
        let mut whole = Vec::new();
        let mut w = Writer::new(Config::default());
        w.string(&s, &mut whole).unwrap();

        let mut bounds: Vec<usize> = cuts
            .iter()
            .map(|c| c.index(s.len() + 1))
            .filter(|&i| s.is_char_boundary(i))
            .collect();
        bounds.extend([0, s.len()]);
        bounds.sort_unstable();
        bounds.dedup();
        let mut parts = Vec::new();
        let mut w = Writer::new(Config::default());
        let pieces: Vec<&str> = if bounds.len() == 1 {
            vec![""]
        } else {
            bounds.windows(2).map(|b| &s[b[0]..b[1]]).collect()
        };
        for (i, piece) in pieces.iter().enumerate() {
            let t = Text { fragment: piece, first: i == 0, last: i + 1 == pieces.len() };
            w.event(Event::String(t), &mut parts).unwrap();
        }
        prop_assert_eq!(parts, whole);
    }
}
