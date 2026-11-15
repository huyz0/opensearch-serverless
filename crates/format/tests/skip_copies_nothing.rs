//! Skipping a field copies nothing: a descriptor whose mapping is a large
//! escaped JSON string (as every shell descriptor's is) parses with no heap
//! allocation at all. Its own binary, because the counting allocator is
//! process-wide.

use oss_format::{CommitManifest, DescriptorRecord};
use oss_testkit::{CountingAllocator, counting_allocator};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

#[test]
fn skipped_escaped_strings_are_not_decoded() {
    let mut mapping = String::from(r#"{\"properties\":{"#);
    while mapping.len() < 256 * 1024 {
        mapping.push_str(r#"\"field\":{\"type\":\"keyword\"},"#);
    }
    mapping.push_str(r#"\"last\":{\"type\":\"long\"}}}"#);
    let descriptor = format!(
        r#"{{"name":"logs","uuid":"u1","number_of_shards":3,"mapping":"{mapping}","settings":{{"index.x":"a\"b"}},"aliased_by":["aA"]}}"#
    );
    let manifest = format!(
        r#"{{"term":1,"note":"{mapping}","files":{{"_0.si":"t=1"}},"digest":{{"f":"{mapping}"}}}}"#
    );

    let before = ALLOC.total_allocated();
    let parsed = DescriptorRecord::parse(descriptor.as_bytes())
        .map(|r| matches!(r, DescriptorRecord::Index(_)));
    let after_descriptor = ALLOC.total_allocated();
    assert_eq!(parsed, Ok(true));
    assert_eq!(after_descriptor - before, 0, "a descriptor parse allocated");

    let before = ALLOC.total_allocated();
    let files = CommitManifest::parse(manifest.as_bytes()).map(|m| m.files().len());
    let allocated = ALLOC.total_allocated() - before;
    assert_eq!(files, Ok(1));
    // One file entry; the skipped note and digest strings cost nothing.
    assert!(
        allocated < 1024,
        "a manifest parse allocated {allocated} bytes"
    );
}
