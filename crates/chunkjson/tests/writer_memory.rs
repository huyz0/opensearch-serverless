//! One huge string is written in fragments with no heap growth. Its own test
//! binary, so no test allocates while it measures; the harness's own threads
//! still can, a few bytes, so a run counts as clean when the writer's count is
//! zero in any of a few attempts (a writer that allocates fails every one).
// ARITH: test inputs are small and fixed; an overflow would be a test bug, and debug builds panic on it.
#![allow(missing_docs, clippy::arithmetic_side_effects)]

use std::alloc;
use std::convert::Infallible;

use cap::Cap;
use chunkjson::{Config, Event, Output, Text, Writer};

#[global_allocator]
static ALLOCATOR: Cap<alloc::System> = Cap::new(alloc::System, usize::MAX);

const FRAGMENT: usize = 64 << 10;
const STRING: usize = 128 << 20;

/// Counts bytes and checks escapes, keeping nothing.
#[derive(Default)]
struct Count {
    bytes: usize,
    escapes: usize,
}

impl Output for Count {
    type Error = Infallible;
    fn write(&mut self, bytes: &[u8]) -> Result<(), Infallible> {
        self.bytes += bytes.len();
        self.escapes += bytes.iter().filter(|&&b| b == b'\\').count();
        Ok(())
    }
}

/// Writes the whole string once; returns the bytes the allocator handed out
/// meanwhile, and the output's counts.
fn write_once(fragment: &str) -> (usize, Count) {
    let mut w = Writer::new(Config::default());
    let mut out = Count::default();
    let before = ALLOCATOR.total_allocated();
    w.event(Event::BeginArray, &mut out).unwrap();
    let pieces = STRING / FRAGMENT;
    for i in 0..pieces {
        let t = Text {
            fragment,
            first: i == 0,
            last: i + 1 == pieces,
        };
        w.event(Event::String(t), &mut out).unwrap();
    }
    w.event(Event::EndArray, &mut out).unwrap();
    w.check_complete().unwrap();
    (ALLOCATOR.total_allocated() - before, out)
}

/// AC-5
#[test]
fn a_128_mib_string_is_written_in_64_kib_fragments_without_heap_growth() {
    // plain text with a newline every 1,000 bytes, which is escaped
    let fragment: String = (0..FRAGMENT)
        .map(|i| {
            if i % 1000 == 999 {
                '\n'
            } else {
                char::from(b'a' + (i % 26) as u8)
            }
        })
        .collect();
    let mut grew = Vec::new();
    let mut last = None;
    for _ in 0..5 {
        let (allocated, out) = write_once(&fragment);
        last = Some(out);
        if allocated == 0 {
            grew.clear();
            break;
        }
        grew.push(allocated);
    }
    assert!(
        grew.is_empty(),
        "the writer allocated, even if it freed it again, in every attempt: {grew:?}"
    );
    let out = last.unwrap();
    let pieces = STRING / FRAGMENT;
    let newlines = pieces * (FRAGMENT / 1000);
    assert_eq!(out.escapes, newlines);
    assert_eq!(out.bytes, 2 + 2 + STRING + newlines);
}
