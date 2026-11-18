//! The overlay segment built from pushed writes (docs/specs/reader-overlay-segment).

// ARITH: test sizes are small; an overflow panics the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use lucene_codecs::stored_fields::FieldValue;
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

/// Every stored value of every document, as text where it is text.
fn stored_texts(overlay: &Overlay) -> Vec<String> {
    let mut out = Vec::new();
    for segment in overlay.reader().expect("documents").segment_readers() {
        for doc in 0..segment.num_docs() {
            let stored = segment
                .stored_document(doc)
                .expect("stored")
                .expect("fields");
            for field in stored.fields {
                match field.value {
                    FieldValue::String(t) => out.push(t),
                    FieldValue::Binary(b) => out.push(String::from_utf8_lossy(&b).into_owned()),
                    _ => {}
                }
            }
        }
    }
    out
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
    let overlay = Overlay::build(&mapping(), &records, 4, usize::MAX).expect("build");
    assert_eq!(overlay.doc_count(), 2, "a (seq 7) and b (seq 6)");
    assert_eq!(with_name(&overlay, "second"), 1);
    assert_eq!(with_name(&overlay, "first"), 0, "a lower number never wins");
    assert_eq!(with_name(&overlay, "kept"), 1);
    assert_eq!(with_name(&overlay, "low"), 0, "at or below the checkpoint");
    // The document held is the winner's: its stored source and id.
    let stored = stored_texts(&overlay);
    assert!(
        stored.iter().any(|t| t.contains(r#""name":"second""#)),
        "{stored:?}"
    );
    assert!(
        !stored.iter().any(|t| t.contains(r#""name":"first""#)),
        "{stored:?}"
    );
    // The later record wins when the numbers are equal.
    let equal = [index("a", "x", 9), index("a", "y", 9)];
    let overlay = Overlay::build(&mapping(), &equal, 0, usize::MAX).expect("build");
    assert_eq!(with_name(&overlay, "y"), 1);
    assert_eq!(with_name(&overlay, "x"), 0);
}

#[test]
fn an_index_record_is_a_document_and_a_deletion_is_none() {
    let records = [index("a", "x", 5), deletion("b", 6), index("c", "z", 7)];
    let overlay = Overlay::build(&mapping(), &records, 0, usize::MAX).expect("build");
    assert_eq!(overlay.doc_count(), 2);
    assert_eq!(with_id(&overlay, "a"), 1);
    assert_eq!(with_id(&overlay, "b"), 0, "a deletion is no document");
    assert_eq!(with_id(&overlay, "c"), 1);
    assert!(overlay.ids().iter().any(|i| i == "b"));
    // A deletion above an index record of the same id leaves no document.
    let records = [index("a", "x", 5), deletion("a", 6)];
    let overlay = Overlay::build(&mapping(), &records, 0, usize::MAX).expect("build");
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
    let overlay = Overlay::build(&mapping(), &records, 0, usize::MAX).expect("build");
    assert_eq!(overlay.ids(), ["b", "a", "c"]);
}

#[test]
fn more_than_the_most_ids_is_refused_and_exactly_the_most_builds() {
    let at_most: Vec<_> = (0..MAX_IDS)
        .map(|i| index(&format!("id{i}"), "x", i as i64 + 1))
        .collect();
    let overlay =
        Overlay::build(&mapping(), &at_most, 0, usize::MAX).expect("exactly the most builds");
    assert_eq!(overlay.doc_count() as usize, MAX_IDS);
    let mut too_many = at_most;
    too_many.push(index("one-more", "x", MAX_IDS as i64 + 1));
    let err = Overlay::build(&mapping(), &too_many, 0, usize::MAX).expect_err("one over");
    assert!(matches!(err, BuildError::TooManyIds), "{err:?}");
    // Dropped records do not count: ids at or below the checkpoint are free.
    let overlay = Overlay::build(&mapping(), &too_many, 1, usize::MAX).expect("one dropped");
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
    let err = Overlay::build(&mapping(), &records, 0, usize::MAX).expect_err("refused");
    assert!(
        matches!(&err, BuildError::Record { id, .. } if id == "bad"),
        "{err:?}"
    );
}

#[test]
fn nothing_above_the_checkpoint_is_an_empty_overlay() {
    let overlay = Overlay::build(&mapping(), &[index("a", "x", 3)], 3, usize::MAX).expect("build");
    assert_eq!(overlay.doc_count(), 0);
    assert!(overlay.ids().is_empty());
    let overlay = Overlay::build(&mapping(), &[], 0, usize::MAX).expect("build");
    assert_eq!(overlay.doc_count(), 0);
}

#[test]
fn the_estimate_grows_with_the_records_and_bounds_the_build() {
    let small: Vec<_> = (0..50)
        .map(|i| index(&format!("id{i}"), "x", i + 1))
        .collect();
    let big: Vec<_> = (0..500)
        .map(|i| index(&format!("id{i}"), "x", i + 1))
        .collect();
    assert!(estimate_bytes(&big) > estimate_bytes(&small));
    // The kept records are what the bound is held to: a build reserved for
    // exactly its estimate runs, one byte less is refused.
    let exact = estimate_bytes(&small);
    Overlay::build(&mapping(), &small, 0, exact).expect("exactly the estimate builds");
    let err = Overlay::build(&mapping(), &small, 0, exact - 1).expect_err("one byte short");
    assert!(
        matches!(err, BuildError::TooManyBytes { estimate, max_bytes } if estimate == exact && max_bytes == exact - 1),
        "{err:?}"
    );
    // Superseded and dropped records are not charged.
    let with_dropped: Vec<_> = small.iter().cloned().chain(small.iter().cloned()).collect();
    Overlay::build(&mapping(), &with_dropped, 0, exact).expect("a repeat adds nothing");
    Overlay::build(&mapping(), &big, 450, estimate_bytes(&big[450..]))
        .expect("records below the checkpoint add nothing");
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
    let overlay = Overlay::build(&mapping(), &[index("a", "x", 5)], 0, usize::MAX).expect("build");
    let shown = format!("{overlay:?}");
    assert!(
        shown.contains("doc_count: 1") && shown.contains("ids: 1"),
        "{shown}"
    );
}
