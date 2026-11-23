//! `reset` keeps the tokenizer's allocations. Its own test binary, but the
//! harness's other threads allocate now and then while the count is open, so a
//! run with no growth in any of a few tries is what shows the tokenizer does not
//! allocate: a tokenizer that did would grow on every try.
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
    let mut grew = Vec::new();
    for _ in 0..10 {
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
        let after = ALLOCATOR.total_allocated();
        if after == before {
            return;
        }
        grew.push(after - before);
    }
    panic!("every try allocated: {grew:?}");
}
