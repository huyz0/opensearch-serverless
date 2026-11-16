//! Shared helpers: the fixture blobs the Java shell wrote.

// ARITH: sizes and offsets here are fixture lengths and small constants; an overflow
// panics the test in a debug build, which fails it as it should.
#![allow(clippy::arithmetic_side_effects)]
#![allow(dead_code, clippy::disallowed_methods)]

use std::path::PathBuf;

/// The deployment base the fixtures were written under (`BlobPath.cleanPath()`).
pub const BASE: &str = "";
pub const INDEX: &str = "fixture";
pub const UUID: &str = "fixture-uuid-0000000";
pub const FIRST_NODE: &str = "tZ9iGQAAQACwLK7Z_____w";
pub const SECOND_NODE: &str = "gnPAlf__T_-qxystAAAAAA";
pub const PACK_EPOCH: u64 = 1_794_710_559_389;

/// The bytes of the fixture blob at `key`.
pub fn blob(key: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/shell-blobs")
        .join(key);
    std::fs::read(&path).unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()))
}

/// The bytes of a Jackson recording (`fixtures/jackson`).
pub fn jackson(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/jackson")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()))
}

/// The bytes of a change-stripe recording (`fixtures/change-stripes`).
pub fn stripe_fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/change-stripes")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()))
}

pub fn manifest_key() -> String {
    format!("segments/{INDEX}#{UUID}#0/manifest")
}

pub fn segment_blob_key() -> String {
    format!("segments/{INDEX}#{UUID}#0/t=2/_1.seg")
}

pub fn pack_key() -> String {
    format!("packs/{FIRST_NODE}/e={PACK_EPOCH:020}/{:020}", 0)
}

/// A Lucene file starts with its codec header magic and ends with a footer
/// whose first four bytes are the footer magic, sixteen bytes from its end.
pub fn assert_lucene_file(bytes: &[u8], name: &str) {
    assert!(bytes.len() >= 16 + 4, "{name} is {} bytes", bytes.len());
    assert_eq!(
        bytes[..4],
        [0x3F, 0xD7, 0x6C, 0x17],
        "{name} starts with the codec magic"
    );
    assert_eq!(
        bytes[bytes.len() - 16..bytes.len() - 12],
        [0xC0, 0x28, 0x93, 0xE8],
        "{name} ends with a footer"
    );
}

/// The lines of a cluster-config recording (`fixtures/cluster-config`).
pub fn settings_fixture(name: &str) -> Vec<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/cluster-config")
        .join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()));
    text.lines().map(str::to_owned).collect()
}
