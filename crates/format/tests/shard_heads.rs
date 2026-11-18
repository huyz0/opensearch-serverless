//! Shard heads read and written as the Java's `ShardHead` does
//! (docs/specs/shard-head/SPEC.md, AC-1 and AC-2).

#![allow(clippy::disallowed_methods)]

use std::path::PathBuf;

use oss_format::{FormatError, ShardHead};

fn fixture(name: &str) -> Vec<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/shard-heads")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()))
        .lines()
        .map(str::to_owned)
        .collect()
}

fn written(head: &ShardHead) -> String {
    String::from_utf8(head.to_bytes()).expect("utf-8")
}

#[test]
fn every_case_reads_and_writes_as_the_javas() {
    let cases = fixture("cases.txt");
    let parsed = fixture("parsed.txt");
    assert_eq!(parsed.len(), cases.len());
    let mut refused = 0;
    for (case, java) in cases.iter().zip(&parsed) {
        let rust = ShardHead::parse(case.as_bytes());
        if java == "refused" {
            refused += 1;
            assert!(
                matches!(rust, Err(FormatError::Corrupt { .. })),
                "{case}: {rust:?}"
            );
            continue;
        }
        let rust = rust.unwrap_or_else(|e| panic!("{case}: {e}"));
        assert_eq!(written(&rust), *java, "{case}");
        assert_eq!(ShardHead::parse(java.as_bytes()).expect("again"), rust);
    }
    assert_eq!(refused, 6);
}

#[test]
fn a_head_held_by_a_process_names_it_and_its_expiry() {
    let head = ShardHead::parse(
        br#"{"index":"logs","shard":3,"term":7,"owner_node_id":"node-2","owner_ephemeral_id":"e2","lease_expires_at_millis":100,"index_uuid":"u-1"}"#,
    )
    .expect("parses");
    assert_eq!(head.term, 7);
    assert!(head.is_held_at(99));
    assert!(!head.is_held_at(100));
    assert!(head.owned_by("node-2", "e2"));
    assert!(!head.owned_by("node-2", "e3"));
    assert!(!head.owned_by("node-1", "e2"));
}

#[test]
fn truncated_and_corrupt_heads_are_refused() {
    let whole = br#"{"index":"logs","shard":0,"term":1,"owner_node_id":"n","writers":["a","b"]}"#;
    for n in 0..whole.len() {
        let cut = &whole[..n];
        assert!(ShardHead::parse(cut).is_err(), "{n} bytes");
    }
    for bad in [
        &b"[]"[..],
        b"{\"index\":\"x\",\"shard\":\"0\",\"term\":1}",
        b"{\"index\":\"x\",\"shard\":0,\"term\":1.5}",
        b"{\"index\":\"x\",\"shard\":0,\"term\":1}x",
        b"{\"index\":\"x\",\"shard\":0,\"term\":1,\"writers\":[1]}",
        b"",
    ] {
        assert!(ShardHead::parse(bad).is_err(), "{bad:?}");
    }
}
