//! The commit's local checkpoint (docs/specs/reader-overlay-checkpoint).

// ARITH: test sizes are small constants.
#![allow(clippy::arithmetic_side_effects)]

use crate::common;

use oss_doc::mapping::Mapping;
use oss_reader::overlay::{local_checkpoint, mask_bytes};

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

fn segment(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|n| (format!("d{n}"), r#"{"name":"x"}"#.to_owned()))
        .collect()
}

#[test]
fn the_masks_of_a_commit_take_a_bit_a_document_in_whole_words_a_segment() {
    // docs: the segments' sizes; bytes: 8 a started word of 64 documents.
    for (sizes, bytes) in [
        (vec![1], 8),
        (vec![64], 8),
        (vec![65], 16),
        (vec![1, 64, 65], 32),
    ] {
        let owned: Vec<Vec<(String, String)>> = sizes.iter().map(|n| segment(*n)).collect();
        let docs: Vec<Vec<(&str, &str, i64)>> = owned
            .iter()
            .map(|s| s.iter().map(|(i, d)| (i.as_str(), d.as_str(), 1)).collect())
            .collect();
        let slices: Vec<&[(&str, &str, i64)]> = docs.iter().map(Vec::as_slice).collect();
        let (_dir, reader) = common::commit_of(&mapping(), &slices);
        assert_eq!(mask_bytes(&reader), bytes, "segments of {sizes:?}");
    }
}
