//! Each contender consumes the input and returns a checksum, so no work is
//! optimised away. Streaming contenders see 64 KiB chunks; the DOM parsers
//! need the whole document (or, for NDJSON, the whole line) in memory.

use chunkjson::{Config, Event, Framing, Tokenizer};
use serde::de::IgnoredAny;

const CHUNK: usize = 64 << 10;

pub fn names() -> Vec<&'static str> {
    vec![
        "chunkjson/64k",
        "chunkjson/whole",
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
