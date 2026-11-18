//! The overlay's lookup of the commit's copies (docs/specs/reader-overlay-commit).

// ARITH: test sizes are small; an overflow panics the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use lucene_search::directory_reader::DirectoryReader;
use lucene_store::ByteBuffersDirectory;
use oss_doc::mapping::Mapping;
use oss_format::WalRecord;
mod common;

use oss_reader::overlay::{MAX_IDS, Overlay, candidate_ids, find_copies};

fn mapping() -> Mapping {
    Mapping::parse(br#"{"properties":{"name":{"type":"keyword"}}}"#).expect("mapping")
}

fn index(id: &str, name: &str, seq: i64) -> WalRecord<'static> {
    WalRecord::index(
        id.to_owned(),
        format!(r#"{{"name":"{name}"}}"#),
        seq,
        1,
        seq + 1,
    )
}

fn deletion(id: &str, seq: i64) -> WalRecord<'static> {
    WalRecord::deletion(id.to_owned(), seq, 1, seq + 1)
}

/// A commit with one segment for each batch of `(id, seq_no)`.
fn commit(batches: &[&[(&str, i64)]]) -> (ByteBuffersDirectory, DirectoryReader) {
    let sources: Vec<Vec<(&str, String, i64)>> = batches
        .iter()
        .map(|b| {
            b.iter()
                .map(|(id, seq)| (*id, format!(r#"{{"name":"{id}"}}"#), *seq))
                .collect()
        })
        .collect();
    let refs: Vec<Vec<(&str, &str, i64)>> = sources
        .iter()
        .map(|b| {
            b.iter()
                .map(|(id, s, seq)| (*id, s.as_str(), *seq))
                .collect()
        })
        .collect();
    let slices: Vec<&[(&str, &str, i64)]> = refs.iter().map(Vec::as_slice).collect();
    common::commit_of(&mapping(), &slices)
}

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn copies_are_found_across_segments_with_their_sequence_numbers() {
    let (_dir, reader) = commit(&[&[("a", 1), ("b", 2)], &[("c", 3), ("d", 4)]]);
    let copies = find_copies(&reader, &ids(&["d", "a", "missing", "c"])).expect("finds");
    assert_eq!(copies.len(), 3);
    assert!(copies.get("missing").is_none());
    let a = copies.get("a").expect("a");
    assert_eq!((a.segment, a.doc, a.seq_no), (0, 0, 1));
    let c = copies.get("c").expect("c");
    assert_eq!((c.segment, c.doc, c.seq_no), (1, 0, 3));
    let d = copies.get("d").expect("d");
    assert_eq!((d.segment, d.doc, d.seq_no), (1, 1, 4));
}

#[test]
fn candidates_are_the_ids_above_the_checkpoint_once_in_first_seen_order() {
    let records = [
        index("b", "x", 5),
        index("old", "x", 2),
        index("a", "x", 6),
        index("b", "y", 7),
        deletion("c", 8),
    ];
    assert_eq!(candidate_ids(&records, 3).expect("ids"), ["b", "a", "c"]);
    let many: Vec<_> = (0..=MAX_IDS)
        .map(|i| index(&format!("id{i}"), "x", i as i64 + 1))
        .collect();
    assert!(candidate_ids(&many, 0).is_err());
    assert_eq!(candidate_ids(&many, 1).expect("one dropped").len(), MAX_IDS);
}

#[test]
fn a_record_the_commit_already_holds_is_dropped() {
    let (_dir, reader) = commit(&[&[("a", 10), ("b", 20), ("c", 30)]]);
    let records = [
        index("a", "newer", 11),
        index("b", "equal", 20),
        index("c", "older", 25),
        index("new", "fresh", 40),
    ];
    let copies = find_copies(&reader, &ids(&["a", "b", "c", "new"])).expect("finds");
    let overlay = Overlay::build_over(&mapping(), &records, 0, usize::MAX, &copies).expect("build");
    assert_eq!(overlay.ids(), ["a", "new"]);
    assert_eq!(overlay.doc_count(), 2);
}

#[test]
fn hidden_lists_the_commit_copies_of_replaced_and_deleted_ids() {
    let (_dir, reader) = commit(&[&[("a", 1), ("b", 2)], &[("c", 3), ("d", 4)]]);
    let records = [
        index("a", "x", 10),
        deletion("c", 11),
        index("new", "x", 12),
        index("d", "old", 3),
    ];
    let copies = find_copies(&reader, &ids(&["a", "c", "new", "d"])).expect("finds");
    let overlay = Overlay::build_over(&mapping(), &records, 0, usize::MAX, &copies).expect("build");
    let mut hidden = overlay.hidden(&copies);
    hidden.sort_unstable();
    // a is segment 0 doc 0 and c is segment 1 doc 0; the new id has no copy
    // and d's older record was dropped.
    assert_eq!(hidden, [(0, 0), (1, 0)]);
}

#[test]
fn no_ids_find_nothing() {
    let (_dir, reader) = commit(&[&[("a", 1)]]);
    assert_eq!(find_copies(&reader, &[]).expect("finds").len(), 0);
}

#[test]
fn the_lookup_takes_at_most_the_most_ids() {
    let (_dir, reader) = commit(&[&[("a", 1)]]);
    let at_most: Vec<String> = (0..MAX_IDS).map(|i| format!("id{i}")).collect();
    assert_eq!(
        find_copies(&reader, &at_most)
            .expect("exactly the most")
            .len(),
        0
    );
    let mut too_many = at_most;
    too_many.push("one-more".to_owned());
    assert!(find_copies(&reader, &too_many).is_err());
}
