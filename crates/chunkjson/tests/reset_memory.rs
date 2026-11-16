//! `reset` keeps the tokenizer's allocations. Its own test binary, so no
//! other test allocates while it counts.
// ARITH: test inputs are small and fixed; an overflow would be a test bug, and debug builds panic on it.
#![allow(missing_docs, clippy::arithmetic_side_effects)]

use std::alloc;

use cap::Cap;
use chunkjson::{Config, Event, Tokenizer};

#[global_allocator]
static ALLOCATOR: Cap<alloc::System> = Cap::new(alloc::System, usize::MAX);

/// AC-5
#[test]
fn a_line_per_document_with_reset_allocates_nothing() {
    let mut tok = Tokenizer::new(Config::default());
    let before = ALLOCATOR.total_allocated();
    for _ in 0..1000 {
        let mut line = &b"{\"a\":[1,2,{\"b\":-1.5e3}],\"c\":\"text\"}"[..];
        loop {
            let (event, used) = tok.next(line, true).unwrap();
            line = &line[used..];
            if event == Event::Eof {
                break;
            }
        }
        tok.reset();
    }
    assert_eq!(ALLOCATOR.total_allocated(), before);
}
