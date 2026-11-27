//! The pack index registers: which nodes may hold live packs, spread over
//! 16 registers by a hash of the node id.

use oss_format::{
    FormatError, INDEX_SLOTS, PackIndex, check_node_id, pack_index_key, pack_index_slot,
};

/// The slot is a stable function of the node id (FNV-1a, 64 bits, mod 16):
/// not Rust's hasher, which may change between versions.
#[test]
fn the_slot_is_a_pinned_function_of_the_node_id() {
    assert_eq!(INDEX_SLOTS, 16);
    assert_eq!(pack_index_slot("node-1"), 3);
    assert_eq!(pack_index_slot("node-2"), 6);
    assert_eq!(pack_index_slot("a"), 12);
    assert_eq!(pack_index_key("", 3).expect("key"), "packs/index/3");
    assert_eq!(
        pack_index_key("base/", 15).expect("key"),
        "base/packs/index/15"
    );
    assert!(pack_index_key("", 16).is_err());
}

/// An add rewrites the entry with a stamp past the old one, so the register
/// changes even when the node is already listed, and entries stay sorted.
#[test]
fn an_upsert_always_changes_the_register() {
    let mut index = PackIndex::new();
    index.upsert("node-b", 100).expect("adds");
    index.upsert("node-a", 100).expect("adds");
    let before = index.clone();
    index.upsert("node-a", 100).expect("again");
    assert_ne!(index, before, "a stamp no later still moves forward");
    assert_eq!(index.stamp("node-a"), Some(101));
    index.upsert("node-a", 500).expect("later");
    assert_eq!(index.stamp("node-a"), Some(500));
    let nodes: Vec<&str> = index.nodes().collect();
    assert_eq!(nodes, vec!["node-a", "node-b"]);
}

#[test]
fn a_removal_names_whether_the_node_was_there() {
    let mut index = PackIndex::new();
    index.upsert("n", 1).expect("adds");
    assert!(index.remove("n"));
    assert!(!index.remove("n"));
    assert!(!index.contains("n"));
}

/// A value round-trips; a bad one is refused; a node id is checked as the
/// members index checks it.
#[test]
fn a_register_round_trips_and_corrupt_values_are_refused() {
    let mut index = PackIndex::new();
    index.upsert("node-1", 7).expect("adds");
    index.upsert("node-2", 9).expect("adds");
    let bytes = index.encode().expect("encodes");
    assert_eq!(PackIndex::parse(&bytes).expect("parses"), index);
    assert!(matches!(
        PackIndex::parse(&[]),
        Err(FormatError::Corrupt { .. })
    ));
    let mut other = bytes.clone();
    other[0] = 9;
    assert!(matches!(
        PackIndex::parse(&other),
        Err(FormatError::Unsupported { .. })
    ));
    assert!(matches!(
        PackIndex::parse(&bytes[..bytes.len() - 1]),
        Err(FormatError::Corrupt { .. })
    ));
    let mut extra = bytes.clone();
    extra.push(0);
    assert!(matches!(
        PackIndex::parse(&extra),
        Err(FormatError::Corrupt { .. })
    ));
    assert!(PackIndex::new().upsert("", 1).is_err());
    assert!(PackIndex::new().upsert("a/b", 1).is_err());
    check_node_id("node-1").expect("valid");
}

/// The register stays under the 256 KiB a register read keeps: an upsert
/// that would pass it is refused, never written.
#[test]
fn the_register_is_bounded_at_256_kib() {
    let mut index = PackIndex::new();
    let mut n = 0u64;
    let refused = loop {
        let id = format!("node-{n:0>120}");
        match index.upsert(&id, n) {
            Ok(()) => n += 1,
            Err(e) => break e,
        }
        assert!(n < 5_000, "no bound");
    };
    assert!(matches!(refused, FormatError::TooLarge { .. }), "{refused}");
    assert!(index.encode().expect("fits").len() <= 256 << 10);
}
