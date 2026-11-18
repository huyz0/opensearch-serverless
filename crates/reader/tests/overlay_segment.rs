//! The overlay segment built from pushed writes (docs/specs/reader-overlay-segment).

// ARITH: test sizes are small; an overflow panics the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use oss_doc::lucene::uid;
use oss_doc::mapping::Mapping;
use oss_format::WalRecord;
use oss_reader::overlay::{BuildError, MAX_IDS, Overlay, estimate_bytes};

fn mapping() -> Mapping {
    Mapping::parse(br#"{"properties":{"name":{"type":"keyword"},"n":{"type":"long"}}}"#)
        .expect("mapping")
}

fn index(id: &str, name: &str, seq: i64) -> WalRecord<'static> {
    WalRecord::index(
        id.to_owned(),
        format!(r#"{{"name":"{name}","n":{seq}}}"#),
        seq,
        1,
        seq + 1,
    )
}

fn deletion(id: &str, seq: i64) -> WalRecord<'static> {
    WalRecord::deletion(id.to_owned(), seq, 1, seq + 1)
}

/// Documents whose `_id` is `id`.
fn with_id(overlay: &Overlay, id: &str) -> i32 {
    let term = uid(id).expect("uid");
    overlay
        .reader()
        .expect("documents")
        .segment_readers()
        .iter()
        .map(|s| s.doc_freq("_id", &term).expect("doc freq"))
        .sum()
}

/// Documents whose `name` is `value`.
fn with_name(overlay: &Overlay, value: &str) -> i32 {
    overlay
        .reader()
        .expect("documents")
        .segment_readers()
        .iter()
        .map(|s| s.doc_freq("name", value.as_bytes()).expect("doc freq"))
        .sum()
}

#[test]
fn records_at_or_below_the_checkpoint_are_dropped_and_the_highest_sequence_number_wins() {
    let records = [
        index("a", "old", 1),
        index("b", "kept", 6),
        index("a", "first", 5),
        index("a", "second", 7),
        index("c", "low", 3),
    ];
    let overlay = Overlay::build(&mapping(), &records, 4).expect("build");
    assert_eq!(overlay.doc_count(), 2, "a (seq 7) and b (seq 6)");
    assert_eq!(with_name(&overlay, "second"), 1);
    assert_eq!(with_name(&overlay, "first"), 0, "a lower number never wins");
    assert_eq!(with_name(&overlay, "kept"), 1);
    assert_eq!(with_name(&overlay, "low"), 0, "at or below the checkpoint");
    // The later record wins when the numbers are equal.
    let equal = [index("a", "x", 9), index("a", "y", 9)];
    let overlay = Overlay::build(&mapping(), &equal, 0).expect("build");
    assert_eq!(with_name(&overlay, "y"), 1);
    assert_eq!(with_name(&overlay, "x"), 0);
}

#[test]
fn an_index_record_is_a_document_and_a_deletion_is_none() {
    let records = [index("a", "x", 5), deletion("b", 6), index("c", "z", 7)];
    let overlay = Overlay::build(&mapping(), &records, 0).expect("build");
    assert_eq!(overlay.doc_count(), 2);
    assert_eq!(with_id(&overlay, "a"), 1);
    assert_eq!(with_id(&overlay, "b"), 0, "a deletion is no document");
    assert_eq!(with_id(&overlay, "c"), 1);
    assert!(overlay.ids().iter().any(|i| i == "b"));
    // A deletion above an index record of the same id leaves no document.
    let records = [index("a", "x", 5), deletion("a", 6)];
    let overlay = Overlay::build(&mapping(), &records, 0).expect("build");
    assert_eq!(overlay.doc_count(), 0);
    assert_eq!(overlay.ids(), ["a"]);
}

#[test]
fn ids_are_each_replaced_or_deleted_id_once_in_first_seen_order() {
    let records = [
        index("b", "x", 5),
        index("a", "x", 6),
        index("b", "y", 7),
        deletion("c", 8),
        deletion("a", 9),
    ];
    let overlay = Overlay::build(&mapping(), &records, 0).expect("build");
    assert_eq!(overlay.ids(), ["b", "a", "c"]);
}

#[test]
fn more_than_the_most_ids_is_refused_and_exactly_the_most_builds() {
    let at_most: Vec<_> = (0..MAX_IDS)
        .map(|i| index(&format!("id{i}"), "x", i as i64 + 1))
        .collect();
    let overlay = Overlay::build(&mapping(), &at_most, 0).expect("exactly the most builds");
    assert_eq!(overlay.doc_count() as usize, MAX_IDS);
    let mut too_many = at_most;
    too_many.push(index("one-more", "x", MAX_IDS as i64 + 1));
    let err = Overlay::build(&mapping(), &too_many, 0).expect_err("one over");
    assert!(matches!(err, BuildError::TooManyIds), "{err:?}");
    // Dropped records do not count: ids at or below the checkpoint are free.
    let overlay = Overlay::build(&mapping(), &too_many, 1).expect("one dropped");
    assert_eq!(overlay.doc_count() as usize, MAX_IDS);
}

#[test]
fn a_source_the_mapping_refuses_names_its_id() {
    let records = [
        index("fine", "x", 5),
        WalRecord::index(
            "bad".to_owned(),
            r#"{"n":"not a number"}"#.to_owned(),
            6,
            1,
            7,
        ),
    ];
    let err = Overlay::build(&mapping(), &records, 0).expect_err("refused");
    assert!(
        matches!(&err, BuildError::Record { id, .. } if id == "bad"),
        "{err:?}"
    );
}

#[test]
fn nothing_above_the_checkpoint_is_an_empty_overlay() {
    let overlay = Overlay::build(&mapping(), &[index("a", "x", 3)], 3).expect("build");
    assert_eq!(overlay.doc_count(), 0);
    assert!(overlay.ids().is_empty());
    let overlay = Overlay::build(&mapping(), &[], 0).expect("build");
    assert_eq!(overlay.doc_count(), 0);
}

#[test]
fn the_estimate_covers_the_built_heap_and_grows_with_the_records() {
    let small: Vec<_> = (0..50)
        .map(|i| index(&format!("id{i}"), "x", i + 1))
        .collect();
    let big: Vec<_> = (0..500)
        .map(|i| index(&format!("id{i}"), "x", i + 1))
        .collect();
    assert!(estimate_bytes(&big) > estimate_bytes(&small));
    let overlay = Overlay::build(&mapping(), &big, 0).expect("build");
    assert!(overlay.heap_bytes() > 0, "a built segment has files");
    assert!(
        estimate_bytes(&big) >= overlay.heap_bytes(),
        "estimate {} below measured {}",
        estimate_bytes(&big),
        overlay.heap_bytes()
    );
}

#[test]
fn errors_and_the_overlay_say_what_they_are() {
    assert_eq!(BuildError::TooManyIds.to_string(), "more than 10000 ids");
    let record = BuildError::Record {
        id: "a".to_owned(),
        reason: "bad".to_owned(),
    };
    assert_eq!(record.to_string(), "record a: bad");
    assert_eq!(
        BuildError::Engine("boom".to_owned()).to_string(),
        "in-memory index: boom"
    );
    let overlay = Overlay::build(&mapping(), &[index("a", "x", 5)], 0).expect("build");
    let shown = format!("{overlay:?}");
    assert!(
        shown.contains("doc_count: 1") && shown.contains("ids: 1"),
        "{shown}"
    );
}
