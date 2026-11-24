//! The keys a snapshot repository and its snapshots are stored under, as the
//! shell's `SnapshotPaths` and `SnapshotStore` name them.

use crate::error::{FormatError, corrupt};
use crate::keys::check_base;

const HEX: &[u8; 16] = b"0123456789abcdef";

/// The key of a snapshot repository's record (`repositories/<name>`).
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a base that is not empty and does not end in
/// `/`.
pub fn repository_key(base: &str, name: &str) -> Result<String, FormatError> {
    check_base(base)?;
    // ALLOC: one key, the length of its parts.
    Ok([base, "repositories/", name].concat())
}

/// The key of a snapshot name's state register
/// (`snapshot-states/<sha256 hex of "<repo>#<name>">`).
///
/// # Errors
///
/// As [`repository_key`].
pub fn snapshot_state_key(base: &str, repo: &str, name: &str) -> Result<String, FormatError> {
    // ALLOC: one key, the length of its parts.
    snapshot_state_key_of_hash(base, &sha256_hex(&[repo, "#", name].concat()))
}

/// The key of the state register whose name hash is `hash`
/// (`snapshot-states/<hash>`), as a discovery marker names it.
///
/// # Errors
///
/// As [`repository_key`], and [`FormatError::Corrupt`] for a hash that is
/// not 64 lowercase hex digits.
pub fn snapshot_state_key_of_hash(base: &str, hash: &str) -> Result<String, FormatError> {
    check_base(base)?;
    if hash.len() != 64 || !hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(corrupt(
            "snapshot key",
            "a state hash that is not 64 hex digits",
        ));
    }
    // ALLOC: one key, the length of its parts.
    Ok([base, "snapshot-states/", hash].concat())
}

/// The lowercase hex SHA-256 of `text`'s UTF-8 bytes, as the shell's
/// `SnapshotStore.hash` writes it.
#[must_use]
pub fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(text.as_bytes());
    // ALLOC: one 64-character hash.
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

/// The two key prefixes under which a repository's snapshots have discovery
/// markers: `snapshots/<repo>#` (the legacy layout) and
/// `snapshots/v2-<sha256 of repo>#` (the current one).
///
/// # Errors
///
/// As [`repository_key`].
pub fn snapshot_marker_prefixes(base: &str, repo: &str) -> Result<[String; 2], FormatError> {
    check_base(base)?;
    // ALLOC: two keys, the length of their parts.
    Ok([
        [base, "snapshots/", repo, "#"].concat(),
        [base, "snapshots/v2-", &sha256_hex(repo), "#"].concat(),
    ])
}

/// The key of a snapshot's legacy record (`snapshots/<repo>#<name>`), read
/// when its name has no state register.
///
/// # Errors
///
/// As [`repository_key`].
pub fn snapshot_legacy_key(base: &str, repo: &str, name: &str) -> Result<String, FormatError> {
    check_base(base)?;
    // ALLOC: one key, the length of its parts.
    Ok([base, "snapshots/", repo, "#", name].concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_prefixes_name_both_layouts_and_hashes_are_checked() {
        let [legacy, v2] = snapshot_marker_prefixes("d/", "backups").unwrap();
        assert_eq!(legacy, "d/snapshots/backups#");
        assert_eq!(v2, format!("d/snapshots/v2-{}#", sha256_hex("backups")));
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(snapshot_state_key_of_hash("", &sha256_hex("x")).is_ok());
        for bad in ["", "ABC", &"g".repeat(64), &"a".repeat(63)] {
            assert!(snapshot_state_key_of_hash("", bad).is_err(), "{bad}");
        }
        assert!(snapshot_marker_prefixes("d", "r").is_err());
    }

    #[test]
    fn repository_and_legacy_snapshot_keys_join_their_parts() {
        assert_eq!(repository_key("", "r").expect("key"), "repositories/r");
        assert_eq!(
            snapshot_legacy_key("d/", "r", "s").expect("key"),
            "d/snapshots/r#s"
        );
        assert!(repository_key("d", "r").is_err());
    }

    #[test]
    fn the_state_register_is_named_by_the_hash_of_repo_hash_name() {
        // sha256("r#s"), as the Java's `HexFormat.formatHex` writes it.
        assert_eq!(
            snapshot_state_key("", "r", "s").expect("key"),
            "snapshot-states/b551ef751e101d6416af3e2ed4eb3f40f15d3b2224ead0b234d914b4419fa644"
        );
        assert_eq!(
            snapshot_state_key("d/", "r", "s").expect("key"),
            "d/snapshot-states/b551ef751e101d6416af3e2ed4eb3f40f15d3b2224ead0b234d914b4419fa644"
        );
    }
}
