//! docs/specs/reader-overlay-dynamic: the overlay builds over fields the
//! mapping does not hold yet.

// ARITH: test sizes are small constants.
#![allow(clippy::arithmetic_side_effects)]

use crate::common;

use oss_doc::mapping::Mapping;
use oss_format::WalRecord;
use oss_reader::overlay::{BuildError, Overlay, build_ready};

fn record(id: &str, source: &str, seq: i64) -> WalRecord<'static> {
    WalRecord::index(id.to_owned(), source.to_owned(), seq, 1, seq + 1)
}

// AC-1
#[test]
fn an_empty_mapping_source_builds_over_records_whose_fields_are_all_new() {
    let seed = Mapping::parse(br#"{"properties":{"name":{"type":"keyword"}}}"#).expect("mapping");
    let (_dir, commit) = common::commit_of(&seed, &[&[("a", r#"{"name":"old"}"#, 1)]]);
    let records = [record("b", r#"{"n":1}"#, 5), record("c", r#"{"n":2}"#, 6)];
    let ready = build_ready(&commit, "", &records, 4 << 20).expect("builds");
    assert_eq!(ready.overlay().ids(), ["b", "c"]);
    assert_eq!(ready.overlay().doc_count(), 2);
}

// AC-2
#[test]
fn a_later_record_uses_a_field_an_earlier_one_added() {
    let records = [
        record("a", r#"{"n":1}"#, 1),
        record("b", r#"{"n":2,"m":"t"}"#, 2),
        record("c", r#"{"m":"u"}"#, 3),
    ];
    let overlay = Overlay::build(&Mapping::default(), &records, 0, usize::MAX).expect("builds");
    assert_eq!(overlay.doc_count(), 3);
    let total: i32 = overlay
        .reader()
        .expect("documents")
        .segment_readers()
        .iter()
        .map(|s| s.doc_freq("m", b"t").expect("doc freq"))
        .sum();
    assert_eq!(total, 1, "the dynamic text field is indexed");
}

// AC-3
#[test]
fn a_dynamic_field_whose_records_disagree_on_type_names_the_record() {
    let records = [
        record("a", r#"{"n":1}"#, 1),
        record("bad", r#"{"n":"text"}"#, 2),
    ];
    let err = Overlay::build(&Mapping::default(), &records, 0, usize::MAX).expect_err("refused");
    assert!(
        matches!(&err, BuildError::Record { id, .. } if id == "bad"),
        "{err:?}"
    );
}
