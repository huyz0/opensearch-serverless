//! Shared test helpers: rebuild serde_json values from events.
// ARITH: test inputs are small and fixed; an overflow would be a test bug, and debug builds panic on it.
#![allow(dead_code, clippy::arithmetic_side_effects, missing_docs)]

use chunkjson::{Config, Error, Event, Framing, Tokenizer};
use serde_json::{Map, Value};

/// Builds values from events, one per top-level document.
#[derive(Debug, Default)]
pub struct Builder {
    stack: Vec<(Value, Option<String>)>,
    key: String,
    string: String,
    pub docs: Vec<Value>,
    pub ranges: Vec<(u64, u64)>,
    pub fragments: usize,
    pub longest_fragment: usize,
}

/// Where `offset` is in `text`, counted from scratch.
pub fn position_of(text: &[u8], offset: u64) -> (u64, u64) {
    let before = &text[..offset as usize];
    let line = 1 + before.iter().filter(|&&b| b == b'\n').count() as u64;
    let line_start = before
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    (line, offset - line_start as u64 + 1)
}

/// The value of a number's text. Text past f64 range, which the tokenizer
/// accepts and serde_json refuses, becomes a string naming it, so it never
/// equals a value serde_json built.
fn number(n: &str) -> Value {
    serde_json::from_str(n).unwrap_or_else(|_| Value::String(format!("<number {n}>")))
}

impl Builder {
    fn put(&mut self, v: Value) {
        match self.stack.last_mut() {
            None => self.docs.push(v),
            Some((Value::Array(a), _)) => a.push(v),
            Some((Value::Object(m), key)) => {
                m.insert(key.take().expect("key before value"), v);
            }
            Some(_) => unreachable!(),
        }
    }

    /// Applies one event; returns false at `Eof`.
    pub fn apply(&mut self, event: Event<'_>) -> bool {
        match event {
            Event::BeginObject => self.stack.push((Value::Object(Map::new()), None)),
            Event::BeginArray => self.stack.push((Value::Array(Vec::new()), None)),
            Event::EndObject | Event::EndArray => {
                let (v, _) = self.stack.pop().expect("open container");
                self.put(v);
            }
            Event::Key(t) => {
                self.note(t.fragment.len());
                if t.first {
                    self.key.clear();
                }
                self.key.push_str(t.fragment);
                if t.last {
                    let key = std::mem::take(&mut self.key);
                    self.stack.last_mut().expect("object").1 = Some(key);
                }
            }
            Event::String(t) => {
                self.note(t.fragment.len());
                if t.first {
                    self.string.clear();
                }
                self.string.push_str(t.fragment);
                if t.last {
                    let s = std::mem::take(&mut self.string);
                    self.put(Value::String(s));
                }
            }
            Event::Number(n) => self.put(number(n)),
            Event::Bool(b) => self.put(Value::Bool(b)),
            Event::Null => self.put(Value::Null),
            Event::EndDocument { start, end } => self.ranges.push((start, end)),
            Event::NeedMore => {}
            Event::Eof => return false,
        }
        true
    }

    fn note(&mut self, len: usize) {
        self.fragments += 1;
        self.longest_fragment = self.longest_fragment.max(len);
    }
}

/// Tokenizes `bytes` split into chunks of the given sizes (cycled), then
/// rebuilds the documents.
pub fn parse_chunked(bytes: &[u8], sizes: &[usize], config: Config) -> Result<Builder, Error> {
    let mut tok = Tokenizer::new(config);
    let mut b = Builder::default();
    let mut pos: usize = 0;
    let mut k = 0;
    loop {
        let size = sizes
            .get(k % sizes.len().max(1))
            .copied()
            .unwrap_or(usize::MAX)
            .max(1);
        let end = pos.saturating_add(size).min(bytes.len());
        let mut chunk = &bytes[pos..end];
        let eof = end == bytes.len();
        loop {
            let (event, used) = tok.next(chunk, eof)?;
            chunk = &chunk[used..];
            if event == Event::NeedMore {
                assert!(!eof, "never NeedMore once eof is set");
                break;
            }
            if !b.apply(event) {
                assert_eq!(
                    tok.offset(),
                    bytes.len() as u64,
                    "Eof only after every byte"
                );
                return Ok(b);
            }
        }
        assert!(chunk.is_empty(), "NeedMore only once the slice is used up");
        pos = end;
        k += 1;
    }
}

pub fn parse_one(bytes: &[u8], sizes: &[usize]) -> Result<Value, Error> {
    let b = parse_chunked(bytes, sizes, Config::default())?;
    assert_eq!(b.docs.len(), 1);
    Ok(b.docs.into_iter().next().expect("one doc"))
}

pub fn config(framing: Framing) -> Config {
    Config::with_framing(framing)
}

/// As [`parse_chunked`], through the push API.
pub fn feed_chunked(bytes: &[u8], sizes: &[usize], config: Config) -> Result<Builder, Error> {
    struct Into<'b>(&'b mut Builder);
    impl chunkjson::Sink for Into<'_> {
        type Error = Error;
        fn event(&mut self, event: Event<'_>) -> Result<(), Error> {
            assert!(
                !matches!(event, Event::NeedMore | Event::Eof),
                "a sink never sees {event:?}"
            );
            self.0.apply(event);
            Ok(())
        }
    }
    let mut tok = Tokenizer::new(config);
    let mut b = Builder::default();
    let mut pos: usize = 0;
    let mut k = 0;
    loop {
        let size = sizes
            .get(k % sizes.len().max(1))
            .copied()
            .unwrap_or(usize::MAX)
            .max(1);
        let end = pos.saturating_add(size).min(bytes.len());
        let eof = end == bytes.len();
        if tok.feed(&bytes[pos..end], eof, &mut Into(&mut b))? {
            assert_eq!(
                tok.offset(),
                bytes.len() as u64,
                "complete only after every byte"
            );
            return Ok(b);
        }
        assert!(!eof, "eof completes or fails");
        pos = end;
        k += 1;
    }
}
