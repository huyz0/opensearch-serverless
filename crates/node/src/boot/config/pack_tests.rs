//! The node pack variables.
use super::super::tests::{FS, lookup, with};
use super::super::*;

// AC-2
#[test]
fn node_packs_are_off_by_default_and_on_with_the_documented_sizes() {
    let off = NodeConfig::from_lookup(lookup(FS)).expect("valid");
    assert_eq!(off.node_packs, None);
    let sized = NodeConfig::from_lookup(lookup(&with(&[
        ("OSS_NODE_PACK_MAX_BLOB_BYTES", "1024"),
        ("OSS_NODE_PACK_LINGER_MS", "7"),
    ])))
    .expect("valid");
    assert_eq!(sized.node_packs, None, "the sizes alone change nothing");
    let on = NodeConfig::from_lookup(lookup(&with(&[("OSS_NODE_PACKS", "true")]))).expect("valid");
    assert_eq!(on.node_packs, Some(crate::writer::PackConfig::default()));
}

// AC-2
#[test]
fn a_pack_size_outside_its_bounds_is_refused() {
    for (name, value) in [
        ("OSS_NODE_PACK_MAX_BLOB_BYTES", "0"),
        ("OSS_NODE_PACK_MAX_BLOB_BYTES", "67108865"),
        ("OSS_NODE_PACK_LINGER_MS", "0"),
        ("OSS_NODE_PACK_LINGER_MS", "5001"),
    ] {
        assert!(
            NodeConfig::from_lookup(lookup(&with(&[(name, value)]))).is_err(),
            "{name}={value}"
        );
    }
    for (name, value) in [
        ("OSS_NODE_PACK_MAX_BLOB_BYTES", "1"),
        ("OSS_NODE_PACK_MAX_BLOB_BYTES", "67108864"),
        ("OSS_NODE_PACK_LINGER_MS", "1"),
        ("OSS_NODE_PACK_LINGER_MS", "5000"),
    ] {
        assert!(
            NodeConfig::from_lookup(lookup(&with(&[(name, value)]))).is_ok(),
            "{name}={value}"
        );
    }
}

// AC-2: the switch reaches the publishing a node is built with.
#[test]
fn the_switch_reaches_the_node_configuration() {
    let on = NodeConfig::from_lookup(lookup(&with(&[("OSS_NODE_PACKS", "true")]))).expect("valid");
    let mut node = crate::assemble::Config::new("", "n1", [0; 6]);
    assert_eq!(node.node_packs, None);
    crate::boot::support::apply_janitor_and_hibernation(&mut node, &on);
    assert_eq!(node.node_packs, on.node_packs);
    assert!(node.node_packs.is_some());
}
