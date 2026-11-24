//! docs/specs/shared-mappings: one parsed mapping for equal descriptor text.

use std::sync::Arc;
use std::time::Instant;

use oss_doc::mapping::{Mapping, MappingInterner};

fn mapping_of(fields: usize, tag: usize) -> String {
    let mut text = String::from(r#"{"properties":{"#);
    for n in 0..fields {
        if n > 0 {
            text.push(',');
        }
        let kind = ["keyword", "text", "long", "date"][n % 4];
        text.push_str(&format!(r#""f{tag}_{n}":{{"type":"{kind}"}}"#));
    }
    text.push_str("}}");
    text
}

// AC-1
#[test]
fn equal_text_is_one_mapping_and_unequal_text_is_two() {
    let table = MappingInterner::new();
    let a = table.get(Some(&mapping_of(3, 0))).expect("parses");
    let b = table.get(Some(&mapping_of(3, 0))).expect("parses");
    let c = table.get(Some(&mapping_of(3, 1))).expect("parses");
    assert!(Arc::ptr_eq(&a, &b));
    assert!(!Arc::ptr_eq(&a, &c));
    assert_eq!(
        *a,
        Mapping::from_descriptor(Some(&mapping_of(3, 0))).unwrap()
    );
    assert!(table.get(None).is_ok());
}

// AC-1
#[test]
fn a_text_that_does_not_parse_is_an_error_and_is_not_stored() {
    let table = MappingInterner::new();
    assert!(
        table
            .get(Some(r#"{"properties":{"a":{"type":"nonsense"}}}"#))
            .is_err()
    );
    assert_eq!(table.entries(), 0);
}

// AC-2
#[test]
fn a_dropped_mapping_is_freed_and_churn_does_not_grow_the_table() {
    let table = MappingInterner::new();
    let kept = table.get(Some(&mapping_of(2, 99_999))).expect("parses");
    for tag in 0..10_000 {
        drop(table.get(Some(&mapping_of(2, tag))).expect("parses"));
    }
    assert_eq!(table.live(), 1);
    // 16 lock shards, each swept at its own threshold: far below 10,000.
    assert!(table.entries() < 16 * 128 + 1, "{}", table.entries());
    drop(kept);
    assert_eq!(table.live(), 0);
}

// AC-5
#[test]
fn many_threads_share_one_mapping_and_distinct_texts_do_not_wait() {
    let table = Arc::new(MappingInterner::new());
    let text = mapping_of(150, 0);
    let found: Vec<Arc<Mapping>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..16)
            .map(|_| scope.spawn(|| table.get(Some(&text)).expect("parses")))
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("joins"))
            .collect()
    });
    assert!(found.iter().all(|m| Arc::ptr_eq(m, &found[0])));
    let distinct: Vec<Arc<Mapping>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (1..=16)
            .map(|tag| {
                let table = Arc::clone(&table);
                scope.spawn(move || table.get(Some(&mapping_of(150, tag))).expect("parses"))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("joins"))
            .collect()
    });
    assert_eq!(distinct.len(), 16);
}

// AC-7
#[test]
fn a_hit_costs_far_less_than_a_parse() {
    let table = MappingInterner::new();
    let text = mapping_of(150, 0);
    let parse = {
        let start = Instant::now();
        let _first = table.get(Some(&text)).expect("parses");
        start.elapsed()
    };
    let held = table.get(Some(&text)).expect("hit");
    let hits = 100u32;
    let start = Instant::now();
    for _ in 0..hits {
        let _ = table.get(Some(&text)).expect("hit");
    }
    let hit = start.elapsed() / hits;
    drop(held);
    assert!(hit * 10 < parse, "hit {hit:?}, parse {parse:?}");
}
