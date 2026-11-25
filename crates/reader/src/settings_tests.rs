//! What an index's settings do to its open: the result window it reads and
//! the read block that stops it.

// Tests; compiled only under `#[cfg(test)]` from the parent module.
#![cfg(test)]

use crate::ReaderError;
use crate::shard_files::meta_of;
use oss_format::IndexDescriptor;

fn descriptor<'a>(settings: &'a [u8]) -> IndexDescriptor<'a> {
    IndexDescriptor {
        name: "i".into(),
        uuid: "u".into(),
        number_of_shards: 1,
        bucket: None,
        has_unit_layout: false,
        mapping_escaped: Some(b"{}"),
        settings: Some(settings),
    }
}

// docs/specs/index-max-result-window AC-1, AC-4
#[test]
fn an_index_reads_its_result_window_and_a_bad_value_leaves_the_default() {
    let window = |settings: &str| {
        meta_of(&descriptor(settings.as_bytes()))
            .expect("reads")
            .max_result_window()
    };
    assert_eq!(window("{}"), 10_000);
    assert_eq!(window(r#"{"index.max_result_window":"50"}"#), 50);
    assert_eq!(window(r#"{"index.max_result_window":"10000"}"#), 10_000);
    for bad in ["0", "-1", "soon", "10001", ""] {
        let settings = format!(r#"{{"index.max_result_window":"{bad}"}}"#);
        assert_eq!(window(&settings), 10_000, "{bad}");
    }
}

// docs/specs/index-blocks-read-metadata AC-1
#[test]
fn an_index_with_a_read_block_does_not_open() {
    let open = |settings: &str| meta_of(&descriptor(settings.as_bytes()));
    let blocked = open(r#"{"index.blocks.read":"true"}"#).expect_err("blocked");
    assert!(matches!(blocked, ReaderError::ReadBlocked(ref n) if n == "i"));
    assert_eq!(
        blocked.to_string(),
        "index [i] blocked by: [FORBIDDEN/7/index read (api)];"
    );
    assert!(open(r#"{"index.blocks.read":"false"}"#).is_ok());
    assert!(open("{}").is_ok());
    // Only the read block stops reads: a write block leaves them be.
    assert!(open(r#"{"index.blocks.write":"true","index.blocks.read_only":"true"}"#).is_ok());
}
