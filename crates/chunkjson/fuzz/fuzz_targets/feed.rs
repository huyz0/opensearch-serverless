//! Arbitrary bytes under a configuration and chunk size (1 to 15 bytes, or
//! all at once) taken from the first byte. The push API (`feed`, the fast
//! path) gives exactly the tokens and the error the pull API gives; in single framing without comments it
//! accepts exactly what serde_json accepts; and what it accepts, written back
//! through the `Writer`, tokenizes to the same tokens again.
#![no_main]

use chunkjson::{Config, Error, Event, Framing, Sink, Text, Tokenizer, Writer};
use libfuzzer_sys::fuzz_target;

/// A token with its fragments joined, so chunking cannot change it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Begin(bool),
    End(bool),
    Key(String),
    Str(String),
    Num(String),
    Bool(bool),
    Null,
    Doc(u64, u64),
}

/// Joins fragments into whole tokens.
#[derive(Default)]
struct Join {
    toks: Vec<Tok>,
    text: String,
}

impl Join {
    fn add(&mut self, event: Event<'_>) {
        let tok = match event {
            Event::BeginObject => Tok::Begin(true),
            Event::BeginArray => Tok::Begin(false),
            Event::EndObject => Tok::End(true),
            Event::EndArray => Tok::End(false),
            Event::Key(t) | Event::String(t) => {
                self.text.push_str(t.fragment);
                if !t.last {
                    return;
                }
                let s = std::mem::take(&mut self.text);
                if matches!(event, Event::Key(_)) { Tok::Key(s) } else { Tok::Str(s) }
            }
            Event::Number(n) => Tok::Num(n.to_owned()),
            Event::Bool(b) => Tok::Bool(b),
            Event::Null => Tok::Null,
            Event::EndDocument { start, end } => Tok::Doc(start, end),
            Event::NeedMore | Event::Eof => return,
        };
        self.toks.push(tok);
    }
}

impl Sink for Join {
    type Error = Error;
    fn event(&mut self, event: Event<'_>) -> Result<(), Error> {
        self.add(event);
        Ok(())
    }
}

fn pull(bytes: &[u8], config: Config) -> (Vec<Tok>, Result<(), Error>) {
    let mut tok = Tokenizer::new(config);
    let mut join = Join::default();
    let mut input = bytes;
    loop {
        match tok.next(input, true) {
            Err(e) => return (join.toks, Err(e)),
            Ok((Event::Eof, _)) => return (join.toks, Ok(())),
            Ok((Event::NeedMore, _)) => panic!("NeedMore at eof"),
            Ok((event, used)) => {
                join.add(event);
                input = &input[used..];
            }
        }
    }
}

fn push(bytes: &[u8], size: usize, config: Config) -> (Vec<Tok>, Result<(), Error>) {
    let mut tok = Tokenizer::new(config);
    let mut join = Join::default();
    let mut chunks = bytes.chunks(size).peekable();
    if chunks.peek().is_none() {
        let r = tok.feed(&[], true, &mut join).map(|done| assert!(done));
        return (join.toks, r);
    }
    while let Some(chunk) = chunks.next() {
        let eof = chunks.peek().is_none();
        match tok.feed(chunk, eof, &mut join) {
            Err(e) => return (join.toks, Err(e)),
            Ok(done) => assert_eq!(done, eof, "complete only at eof"),
        }
    }
    (join.toks, Ok(()))
}

/// Writes `toks` back through the writer.
fn rewrite(toks: &[Tok], config: Config) -> Vec<u8> {
    let mut w = Writer::new(config);
    let mut out = Vec::new();
    for t in toks {
        let event = match t {
            Tok::Begin(true) => Event::BeginObject,
            Tok::Begin(false) => Event::BeginArray,
            Tok::End(true) => Event::EndObject,
            Tok::End(false) => Event::EndArray,
            Tok::Key(s) => Event::Key(whole(s)),
            Tok::Str(s) => Event::String(whole(s)),
            Tok::Num(n) => Event::Number(n),
            Tok::Bool(b) => Event::Bool(*b),
            Tok::Null => Event::Null,
            Tok::Doc(..) => continue,
        };
        w.event(event, &mut out).expect("the tokenizer's tokens are valid");
    }
    w.check_complete().expect("complete");
    out
}

/// A whole key or string as one fragment.
fn whole(s: &str) -> Text<'_> {
    Text { fragment: s, first: true, last: true }
}

fn without_ranges(toks: &[Tok]) -> Vec<Tok> {
    toks.iter().filter(|t| !matches!(t, Tok::Doc(..))).cloned().collect()
}

fuzz_target!(|data: &[u8]| {
    let Some((&first, bytes)) = data.split_first() else { return };
    // 1 to 15 bytes, or the whole input as one chunk
    let size = match first & 15 {
        15 => usize::MAX,
        n => usize::from(n) + 1,
    };
    let framing = match (first >> 5) & 3 {
        0 | 3 => Framing::Single,
        1 => Framing::Concatenated,
        _ => Framing::Ndjson,
    };
    let config = Config {
        comments: first & 16 != 0,
        allow_empty: first & 128 != 0,
        ..Config::with_framing(framing)
    };
    let pulled = pull(bytes, config);
    let pushed = push(bytes, size, config);
    assert_eq!(pushed, pulled, "push against pull, input {:?}", String::from_utf8_lossy(bytes));

    if !config.comments && framing == Framing::Single && !config.allow_empty {
        let oracle = serde_json::from_slice::<serde_json::Value>(bytes);
        let skip = matches!(&oracle, Err(e) if e.to_string().contains("number out of range")
            || e.to_string().contains("recursion limit"))
            // chunkjson's own bound on a number, which serde_json does not have.
            || matches!(&pulled.1, Err(e) if e.kind() == chunkjson::ErrorKind::NumberTooLong);
        if !skip {
            assert_eq!(pulled.1.is_ok(), oracle.is_ok(), "against serde_json");
        }
    }

    if pulled.1.is_ok() && !pulled.0.is_empty() {
        let out = rewrite(&pulled.0, config);
        let plain = Config { comments: false, ..config };
        let again = pull(&out, plain);
        assert_eq!(again.1, Ok(()), "the writer's output tokenizes: {:?}", String::from_utf8_lossy(&out));
        assert_eq!(without_ranges(&again.0), without_ranges(&pulled.0), "the same tokens");
    }
});
