//! The plan's size bound, what a fast document weighs, recompiling, and text with no token.

// ARITH: test sizes are small; an overflow panics the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

mod fast_common;

use fast_common::*;
use lucene_store::ByteBuffersDirectory;
use oss_doc::fast::FastIngest;
use oss_doc::lucene::uid;

#[test]
fn a_source_over_the_size_bound_stays_with_the_complete_path() {
    let words = |n: usize| "word ".repeat(n / 5);
    let small = format!(r#"{{"message":"{}"}}"#, words(900_000));
    let large = format!(r#"{{"message":"{}"}}"#, words(1_100_000));
    let mapping = r#"{"properties":{"message":{"type":"text"}}}"#;
    let taken = run(mapping, &[&small, &small, &large, &large]);
    // The first numbers the field; the second is under the bound and read by the plan; the
    // large ones are too big for the plan, whose memory is about twice the source's.
    assert_eq!(taken, [false, true, false, false]);
}

#[test]
fn a_source_of_exactly_the_size_bound_is_read_by_the_plan_and_one_byte_more_is_not() {
    // `{"message":"` and `"}` are 14 bytes; the rest is one-letter words.
    let of_len = |n: usize| {
        let pad = n - 14;
        format!(
            r#"{{"message":"{}{}"}}"#,
            "a ".repeat(pad / 2),
            "a".repeat(pad % 2)
        )
    };
    let (at, over) = (of_len(1 << 20), of_len((1 << 20) + 1));
    assert_eq!((at.len(), over.len()), (1 << 20, (1 << 20) + 1));
    let mapping = r#"{"properties":{"message":{"type":"text"}}}"#;
    let taken = run(mapping, &[&at, &at, &over]);
    assert_eq!(taken, [false, true, false]);
}

#[test]
fn a_fast_document_weighs_what_it_holds_and_invalidating_recompiles_the_plan() {
    let (d1, d2) = (ByteBuffersDirectory::new(), ByteBuffersDirectory::new());
    let (mut slow, fast) = (side(&d1, LOGS), side(&d2, LOGS));
    let _ = fast;
    let first = log(0);
    slow.slow("id0", 0, first.as_bytes());
    let mut path = FastIngest::new();
    let key = uid("id1").expect("uid");
    let small = path
        .prepare(&slow.indexer, &slow.mapping, log(1).as_bytes(), &key)
        .expect("read by the plan");
    let long = format!(
        r#"{{"@timestamp":"2026-11-20T08:00:00.000Z","level":"info","host":"h","status":200,"bytes":1,"message":"{}"}}"#,
        "word ".repeat(400)
    );
    let big = path
        .prepare(&slow.indexer, &slow.mapping, long.as_bytes(), &key)
        .expect("read by the plan");
    assert!(small.weight() > 1, "{}", small.weight());
    assert!(
        big.weight() > small.weight() + 1000,
        "{} {}",
        big.weight(),
        small.weight()
    );
    let compiles = path.compiles();
    path.prepare(&slow.indexer, &slow.mapping, log(2).as_bytes(), &key)
        .expect("read by the plan");
    assert_eq!(path.compiles(), compiles);
    path.invalidate();
    path.prepare(&slow.indexer, &slow.mapping, log(3).as_bytes(), &key)
        .expect("read by the plan");
    assert_eq!(path.compiles(), compiles + 1);
}

#[test]
fn a_text_with_no_token_is_numbered_but_written_nowhere_alone_or_beside_others() {
    let mapping = r#"{"properties":{"t":{"type":"text"},"k":{"type":"keyword"}}}"#;
    let docs = [
        r#"{"t":"a b","k":"x"}"#,
        r#"{"t":"","k":"x"}"#,
        r#"{"t":""}"#,
        r#"{"t":"  "}"#,
        r#"{"t":"c","k":"y"}"#,
        r#"{"t":"","k":""}"#,
    ];
    let taken = run(mapping, &docs);
    assert!(taken[1..].iter().filter(|&&t| t).count() >= 3, "{taken:?}");
}

// docs/specs/extension-analyzer AC-10: a pinned field is never read by the
// plan, which would analyze it with the built-in analyzer.
#[test]
fn a_document_with_a_pinned_analyzer_is_left_to_the_complete_path() {
    let dir = ByteBuffersDirectory::new();
    let mut side = side(&dir, r#"{"properties":{"t":{"type":"text"}}}"#);
    let key = uid("id1").expect("uid");
    let source = br#"{"t":"hello world"}"#;
    // The plan reads the fields the engine has numbered.
    side.slow("id0", 0, source);
    assert!(
        FastIngest::new()
            .prepare(&side.indexer, &side.mapping, source, &key)
            .is_some(),
        "the same field with no pin is read by the plan"
    );
    side.mapping = oss_doc::mapping::Mapping::parse_pinned(
        br#"{"properties":{"t":{"type":"text","analyzer":"ext_shout"}}}"#,
        oss_doc::mapping::ParseOptions::default(),
    )
    .expect("mapping");
    assert!(
        FastIngest::new()
            .prepare(&side.indexer, &side.mapping, source, &key)
            .is_none()
    );
}
