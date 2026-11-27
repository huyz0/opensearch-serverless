//! One call from a commit and records to a finished overlay
//! (docs/specs/node-overlay-builder, AC-8).

// ARITH: test sizes are small constants.
#![allow(clippy::arithmetic_side_effects)]

use crate::common;

use oss_doc::mapping::Mapping;
use oss_format::WalRecord;
use oss_reader::overlay::build_ready;

const MAPPING: &str = r#"{"properties":{"name":{"type":"keyword"}}}"#;

const DOCS: &[&[(&str, &str, i64)]] = &[&[
    ("a", r#"{"name":"old"}"#, 1),
    ("b", r#"{"name":"old"}"#, 2),
    ("c", r#"{"name":"old"}"#, 3),
]];

fn index(id: &str, name: &str, seq: i64) -> WalRecord<'static> {
    WalRecord::index(
        id.to_owned(),
        format!(r#"{{"name":"{name}"}}"#),
        seq,
        1,
        seq + 1,
    )
}

#[test]
fn records_the_commit_holds_are_left_out_and_newer_ones_kept() {
    let mapping = Mapping::parse(MAPPING.as_bytes()).expect("mapping");
    // The commit's checkpoint is 3: it holds seq 1 to 3.
    let (_dir, commit) =
        common::commit_with_data(&mapping, DOCS, &[], &[("local_checkpoint", "3")]);
    let records = [
        index("a", "stale", 2),
        index("b", "new", 10),
        WalRecord::deletion("c".to_owned(), 11, 1, 12),
        index("d", "added", 12),
    ];
    let ready = build_ready(&commit, MAPPING, &records, 4 << 20).expect("builds");
    // a (seq 2) is below the checkpoint; b, c and d are kept.
    assert_eq!(ready.overlay().ids(), ["b", "c", "d"]);
    assert_eq!(ready.overlay().doc_count(), 2);
}

#[test]
fn a_mapping_that_is_not_json_is_refused() {
    let mapping = Mapping::parse(MAPPING.as_bytes()).expect("mapping");
    let (_dir, commit) = common::commit_of(&mapping, DOCS);
    assert!(build_ready(&commit, "not json", &[index("d", "x", 5)], 4 << 20).is_err());
}

#[test]
fn a_build_over_the_bytes_reserved_is_refused() {
    let mapping = Mapping::parse(MAPPING.as_bytes()).expect("mapping");
    let (_dir, commit) = common::commit_of(&mapping, DOCS);
    assert!(build_ready(&commit, MAPPING, &[index("d", "x", 5)], 1).is_err());
}
