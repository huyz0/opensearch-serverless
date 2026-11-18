//! The commit's local checkpoint (docs/specs/reader-overlay-checkpoint).

// ARITH: test sizes are small constants.
#![allow(clippy::arithmetic_side_effects)]

mod common;

use oss_doc::mapping::Mapping;
use oss_reader::overlay::local_checkpoint;

fn mapping() -> Mapping {
    Mapping::parse(br#"{"properties":{"name":{"type":"keyword"}}}"#).expect("mapping")
}

const DOCS: &[&[(&str, &str, i64)]] = &[&[("a", r#"{"name":"x"}"#, 1)]];

#[test]
fn a_commit_gives_the_checkpoint_its_writer_stored() {
    let (_dir, reader) =
        common::commit_with_data(&mapping(), DOCS, &[], &[("local_checkpoint", "41")]);
    assert_eq!(local_checkpoint(&reader), 41);
    let wrapped = reader.soft_deletes_wrapper("__soft_deletes").expect("wrap");
    assert_eq!(local_checkpoint(&wrapped), 41);
}

#[test]
fn a_commit_without_a_number_gives_minus_one() {
    let (_dir, none) = common::commit_of(&mapping(), DOCS);
    assert_eq!(local_checkpoint(&none), -1);
    let (_dir, text) =
        common::commit_with_data(&mapping(), DOCS, &[], &[("local_checkpoint", "soon")]);
    assert_eq!(local_checkpoint(&text), -1);
    let (_dir, other) = common::commit_with_data(&mapping(), DOCS, &[], &[("max_seq_no", "9")]);
    assert_eq!(local_checkpoint(&other), -1);
}
