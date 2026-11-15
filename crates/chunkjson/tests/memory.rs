//! One huge string streams through in fragments, with no heap growth.
// ARITH: test inputs are small and fixed; an overflow would be a test bug, and debug builds panic on it.
#![allow(missing_docs, clippy::arithmetic_side_effects)]

use std::alloc;

use cap::Cap;
use chunkjson::{Config, Event, Tokenizer};

#[global_allocator]
static ALLOCATOR: Cap<alloc::System> = Cap::new(alloc::System, usize::MAX);

const CHUNK: usize = 64 << 10;
const STRING: usize = 64 << 20;

/// Yields the bytes of `{"text":"<STRING bytes of text with escapes>"}` one
/// chunk at a time, into a buffer it reuses, so the document never exists whole.
struct Source {
    sent: usize,
    total: usize,
}

impl Source {
    fn fill(&mut self, buf: &mut [u8]) -> usize {
        let head = br#"{"text":""#;
        let mut n = 0;
        while n < buf.len() && self.sent < self.total {
            let i = self.sent;
            buf[n] = if i < head.len() {
                head[i]
            } else if i + 2 == self.total {
                b'"'
            } else if i + 1 == self.total {
                b'}'
            } else {
                // text with an escaped newline every 1,000 bytes, and some UTF-8
                match (i - head.len()) % 1000 {
                    0 => b'\\',
                    1 => b'n',
                    2 => 0xc3,
                    3 => 0xa9,
                    k => b'a' + (k % 26) as u8,
                }
            };
            n += 1;
            self.sent += 1;
        }
        n
    }
}

#[test]
fn a_64_mib_string_streams_in_64_kib_chunks_without_heap_growth() {
    let mut buf = vec![0u8; CHUNK];
    let mut tok = Tokenizer::new(Config::default());
    let mut src = Source {
        sent: 0,
        total: 9 + STRING + 2,
    };
    let before = ALLOCATOR.allocated();
    let mut peak = before;
    let (mut text_bytes, mut longest, mut fragments) = (0usize, 0usize, 0usize);
    'outer: loop {
        let n = src.fill(&mut buf);
        let eof = n == 0;
        let mut chunk = &buf[..n];
        loop {
            let (event, used) = tok.next(chunk, eof).unwrap();
            match event {
                Event::String(t) => {
                    text_bytes += t.fragment.len();
                    longest = longest.max(t.fragment.len());
                    fragments += 1;
                }
                Event::NeedMore => break,
                Event::Eof => break 'outer,
                _ => {}
            }
            chunk = &chunk[used..];
        }
        peak = peak.max(ALLOCATOR.allocated());
    }
    // every 1,000 source bytes hold `\n` (2 bytes, decoded to 1) and `é`
    assert_eq!(text_bytes, STRING - STRING.div_ceil(1000));
    assert!(longest <= CHUNK, "longest fragment {longest}");
    assert!(fragments > STRING / CHUNK);
    let growth = peak - before;
    assert!(growth <= CHUNK, "heap grew {growth} bytes while tokenizing");
}
