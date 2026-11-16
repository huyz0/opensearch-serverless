//! Each contender consumes the input and returns a checksum, so no work is
//! optimised away. Streaming contenders see 64 KiB chunks; the DOM parsers
//! need the whole document (or, for NDJSON, the whole line) in memory.
//!
//! Not every row does the same job. `chunkjson/*` and `*/ignored` validate
//! and tokenize without building anything; `*/value` builds a tree, which is
//! more work, so the gap between them is not a like-for-like speed-up. The
//! fair rewrite comparison is `chunkjson/rewrite-64k` against
//! `serde_json/transcode`, which also streams tokens into a writer. On NDJSON
//! the DOM parsers get a plain split on `\n` bytes and parse each line whole,
//! which needs every line in memory; chunkjson finds the framing itself.

use chunkjson::{Config, Event, Framing, Tokenizer, Writer};
use serde::de::IgnoredAny;

const CHUNK: usize = 64 << 10;

pub fn names() -> Vec<&'static str> {
    vec![
        "chunkjson/64k",
        "chunkjson/whole",
        "chunkjson/feed-64k",
        "chunkjson/rewrite-64k",
        "serde_json/value-rewrite",
        "serde_json/transcode",
        "serde_json/ignored",
        "serde_json/value",
        "sonic_rs/ignored",
        "sonic_rs/value",
        "simd_json/tape",
        "simd_json/borrowed",
    ]
}

pub fn run(contender: &str, case: &str, input: &[u8]) -> u64 {
    let ndjson = crate::cases::is_ndjson(case);
    match contender {
        "chunkjson/64k" => chunkjson(input, CHUNK, ndjson),
        "chunkjson/whole" => chunkjson(input, usize::MAX, ndjson),
        "chunkjson/feed-64k" => chunkjson_feed(input, CHUNK, ndjson),
        "chunkjson/rewrite-64k" => chunkjson_rewrite(input, CHUNK, ndjson),
        "serde_json/value-rewrite" => per_doc(input, ndjson, |d| {
            let v: serde_json::Value = serde_json::from_slice(d).unwrap();
            let mut out = Vec::with_capacity(d.len() + 1);
            serde_json::to_writer(&mut out, &v).unwrap();
            out.len() as u64
        }),
        "serde_json/transcode" => {
            // Streams tokens from the parser to the serializer with no
            // tree, into one output drained every 64 KiB as chunkjson's is.
            let mut out = Drained { buf: Vec::with_capacity(2 * CHUNK), written: 0 };
            per_doc(input, ndjson, |d| {
                let mut de = serde_json::Deserializer::from_slice(d);
                let mut ser = serde_json::Serializer::new(&mut out);
                serde_transcode::transcode(&mut de, &mut ser).unwrap();
                de.end().unwrap();
                0
            });
            out.written + out.buf.len() as u64
        }
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

struct Sum(u64);

impl chunkjson::Sink for Sum {
    type Error = chunkjson::Error;
    fn event(&mut self, event: Event<'_>) -> Result<(), chunkjson::Error> {
        self.0 += match event {
            Event::Key(t) | Event::String(t) => t.fragment.len() as u64,
            Event::Number(n) => n.len() as u64,
            _ => 1,
        };
        Ok(())
    }
}

fn chunkjson_feed(input: &[u8], chunk: usize, ndjson: bool) -> u64 {
    let framing = if ndjson { Framing::Ndjson } else { Framing::Single };
    let mut tok = Tokenizer::new(Config::with_framing(framing));
    let mut sum = Sum(0);
    let mut chunks = input.chunks(chunk).peekable();
    while let Some(c) = chunks.next() {
        tok.feed(c, chunks.peek().is_none(), &mut sum).unwrap();
    }
    sum.0
}

/// Tokenizes and writes the events back, into an output drained after each
/// chunk, as a streaming response would be.
fn chunkjson_rewrite(input: &[u8], chunk: usize, ndjson: bool) -> u64 {
    struct Pipe<'a> {
        writer: &'a mut Writer,
        out: &'a mut Vec<u8>,
    }
    impl chunkjson::Sink for Pipe<'_> {
        type Error = chunkjson::Error;
        fn event(&mut self, event: Event<'_>) -> Result<(), chunkjson::Error> {
            self.writer.event(event, self.out).unwrap();
            Ok(())
        }
    }
    let framing = if ndjson { Framing::Ndjson } else { Framing::Single };
    let config = Config::with_framing(framing);
    let mut tok = Tokenizer::new(config);
    let mut writer = Writer::new(config);
    let mut out = Vec::with_capacity(2 * chunk);
    let mut written = 0u64;
    let mut chunks = input.chunks(chunk).peekable();
    while let Some(c) = chunks.next() {
        let mut pipe = Pipe { writer: &mut writer, out: &mut out };
        tok.feed(c, chunks.peek().is_none(), &mut pipe).unwrap();
        written += out.len() as u64;
        out.clear();
    }
    writer.check_complete().unwrap();
    written
}

/// An output that keeps at most about 64 KiB, counting what it drops.
struct Drained {
    buf: Vec<u8>,
    written: u64,
}

impl std::io::Write for Drained {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(bytes);
        if self.buf.len() >= CHUNK {
            self.written += self.buf.len() as u64;
            self.buf.clear();
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
