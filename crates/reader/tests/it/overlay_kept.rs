//! The shape of the kept-size estimate: its terms, read off one case the
//! build estimate's cap does not bind. (The heap it must bound is measured in
//! `overlay_heap.rs`, whose allocator counts the whole process, so this test
//! lives apart.)

use oss_format::WalRecord;
use oss_reader::overlay::{estimate_bytes, kept_bytes};

#[test]
fn the_kept_estimate_is_a_fixed_part_the_stored_bytes_and_a_quarter_of_the_source() {
    // One large record against a small segment: far under the build estimate,
    // so the cap does not bind and each term is read off.
    let body = "x".repeat(1 << 20);
    let recs = vec![WalRecord::index(
        "id0".to_owned(),
        format!(r#"{{"body":"{body}"}}"#),
        1,
        1,
        1,
    )];
    let source: usize = recs.iter().map(|r| r.id().len() + r.source().len()).sum();
    assert!(source > 1 << 20);
    let none = kept_bytes(&recs, 0);
    assert_eq!(none, (64 << 10) + source / 4);
    let some = kept_bytes(&recs, 1000);
    assert_eq!(some - none, 3000);
    assert!(some < estimate_bytes(&recs));
}
