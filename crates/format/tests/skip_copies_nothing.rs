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

    // The allocator counts the whole process, and the test harness's own
    // threads allocate now and then; the parse's own allocation is the same
    // every time, so the least over several runs is the parse's alone.
    let mut least_descriptor = usize::MAX;
    let mut parsed = Ok(false);
    for _ in 0..8 {
        let before = ALLOC.total_allocated();
        parsed = DescriptorRecord::parse(descriptor.as_bytes())
            .map(|r| matches!(r, DescriptorRecord::Index(_)));
        least_descriptor = least_descriptor.min(ALLOC.total_allocated() - before);
    }
    assert_eq!(parsed, Ok(true));
    assert_eq!(least_descriptor, 0, "a descriptor parse allocated");

    let mut files = Ok(0);
    let mut least_manifest = usize::MAX;
    for _ in 0..8 {
        let before = ALLOC.total_allocated();
        files = CommitManifest::parse(manifest.as_bytes()).map(|m| m.files().len());
        least_manifest = least_manifest.min(ALLOC.total_allocated() - before);
    }
    assert_eq!(files, Ok(1));
    // One file entry; the skipped note and digest strings cost nothing.
    assert!(
        least_manifest < 1024,
        "a manifest parse allocated {least_manifest} bytes"
    );
}
