//! Each contender consumes the input and returns a checksum, so no work is
//! optimised away. Streaming contenders see 64 KiB chunks; the DOM parsers
//! need the whole document (or, for NDJSON, the whole line) in memory.

use chunkjson::{Config, Event, Framing, Tokenizer};
use serde::de::IgnoredAny;

const CHUNK: usize = 64 << 10;

pub fn names() -> Vec<&'static str> {
    let mut v = vec![
        "chunkjson/64k",
        "chunkjson/whole",
        "serde_json/ignored",
        "serde_json/value",
        "sonic_rs/ignored",
        "sonic_rs/value",
        "simd_json/tape",
        "simd_json/borrowed",
    ];
    if cfg!(feature = "oss-rest") {
        v.push("oss_rest/64k");
    }
    v
}

pub fn run(contender: &str, case: &str, input: &[u8]) -> u64 {
    let ndjson = crate::cases::is_ndjson(case);
    match contender {
        "chunkjson/64k" => chunkjson(input, CHUNK, ndjson),
        "chunkjson/whole" => chunkjson(input, usize::MAX, ndjson),
        "serde_json/ignored" => per_doc(input, ndjson, |d| {
            serde_json::from_slice::<IgnoredAny>(d).unwrap();
            1
        }),
        "serde_json/value" => per_doc(input, ndjson, |d| shape(&serde_json::from_slice::<serde_json::Value>(d).unwrap())),
        "sonic_rs/ignored" => per_doc(input, ndjson, |d| {
            sonic_rs::from_slice::<IgnoredAny>(d).unwrap();
            1
        }),
        "sonic_rs/value" => per_doc(input, ndjson, |d| {
            use sonic_rs::JsonValueTrait;
            let v: sonic_rs::Value = sonic_rs::from_slice(d).unwrap();
            u64::from(v.is_object()) + u64::from(v.is_array())
        }),
        "simd_json/tape" => per_doc(input, ndjson, |d| {
            let mut copy = d.to_vec();
            simd_json::to_tape(&mut copy).unwrap().0.len() as u64
        }),
        "simd_json/borrowed" => per_doc(input, ndjson, |d| {
            use simd_json::prelude::*;
            let mut copy = d.to_vec();
            let v = simd_json::to_borrowed_value(&mut copy).unwrap();
            u64::from(v.is_object()) + u64::from(v.is_array())
        }),
        #[cfg(feature = "oss-rest")]
        "oss_rest/64k" => oss_rest::run(input, CHUNK, ndjson),
        _ => panic!("unknown contender {contender}"),
    }
}

fn shape(v: &serde_json::Value) -> u64 {
    u64::from(v.is_object()) + u64::from(v.is_array())
}

/// Runs `parse` on each document: the whole input, or each NDJSON line.
fn per_doc(input: &[u8], ndjson: bool, mut parse: impl FnMut(&[u8]) -> u64) -> u64 {
    if !ndjson {
        return parse(input);
    }
    input.split(|&b| b == b'\n').filter(|l| !l.is_empty()).map(parse).sum()
}

fn chunkjson(input: &[u8], chunk: usize, ndjson: bool) -> u64 {
    let framing = if ndjson { Framing::Ndjson } else { Framing::Single };
    let mut tok = Tokenizer::new(Config::with_framing(framing));
    let mut sum = 0u64;
    let mut chunks = input.chunks(chunk.min(input.len().max(1))).peekable();
    while let Some(mut c) = chunks.next() {
        let eof = chunks.peek().is_none();
        loop {
            let (event, used) = tok.next(c, eof).unwrap();
            c = &c[used..];
            match event {
                Event::Key(t) | Event::String(t) => sum += t.fragment.len() as u64,
                Event::Number(n) => sum += n.len() as u64,
                Event::NeedMore | Event::Eof => break,
                _ => sum += 1,
            }
        }
    }
    sum
}

#[cfg(feature = "oss-rest")]
mod oss_rest {
    use oss_rest::RestError;
    use oss_rest::json::{Limits, Token, TokenSink, Tokenizer};
    use oss_rest::ndjson::{LineSink, Splitter};

    struct Count(u64);

    impl TokenSink for Count {
        type Error = RestError;
        fn token(&mut self, token: Token<'_>) -> Result<(), RestError> {
            self.0 += match token {
                Token::Key(t) | Token::Str { text: t, .. } | Token::Number(t) => t.len() as u64,
                _ => 1,
            };
            Ok(())
        }
    }

    struct Lines {
        tok: Tokenizer,
        count: Count,
    }

    impl LineSink for Lines {
        type Error = RestError;
        fn part(&mut self, bytes: &[u8]) -> Result<(), RestError> {
            self.tok.feed(bytes, &mut self.count)
        }
        fn end_line(&mut self, _: u64) -> Result<(), RestError> {
            self.tok.finish(&mut self.count)?;
            self.tok.reset();
            Ok(())
        }
    }

    pub fn run(input: &[u8], chunk: usize, ndjson: bool) -> u64 {
        let limits = Limits { max_string_bytes: u64::MAX, ..Limits::default() };
        if ndjson {
            let mut lines = Lines { tok: Tokenizer::new(limits), count: Count(0) };
            let mut split = Splitter::new();
            for c in input.chunks(chunk) {
                split.feed(c, &mut lines).unwrap();
            }
            split.finish().unwrap();
            return lines.count.0;
        }
        let mut tok = Tokenizer::new(limits);
        let mut count = Count(0);
        for c in input.chunks(chunk) {
            tok.feed(c, &mut count).unwrap();
        }
        tok.finish(&mut count).unwrap();
        count.0
    }
}
