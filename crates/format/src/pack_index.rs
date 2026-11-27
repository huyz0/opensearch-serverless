//! The pack index: which nodes may hold live packs, so a collector reads the
//! ledgers of only those.
//!
//! Rust only. The index is 16 registers, `packs/index/<slot>`, where the slot
//! is a hash of the node id: a writer adds its node before its first pack and
//! the collector removes a node that holds none, each by compare-and-swap. A
//! value is a version byte (1), an entry count (u32) and per entry the node id
//! (length byte, bytes) and a stamp (u64), big-endian, ids sorted.

use crate::error::{FormatError, corrupt};
use crate::members::{MAX_NODE_ID_BYTES, check_node_id};

const WHAT: &str = "node pack index";
const VERSION: u8 = 1;
const PREFIX_BYTES: usize = 5;

/// How many registers the index is spread over.
pub const INDEX_SLOTS: u8 = 16;

/// The most bytes a register holds: the 256 KiB a register read keeps.
const MAX_BYTES: usize = 256 << 10;

/// The slot of `node_id`: FNV-1a (64 bits) of its bytes, mod [`INDEX_SLOTS`].
/// Not Rust's hasher, which may change between versions.
pub fn pack_index_slot(node_id: &str) -> u8 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in node_id.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    u8::try_from(hash.checked_rem(u64::from(INDEX_SLOTS)).unwrap_or(0)).unwrap_or(0)
}

/// The key of register `slot` under `base` (empty, or ending in `/`).
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a slot past [`INDEX_SLOTS`].
pub fn pack_index_key(base: &str, slot: u8) -> Result<String, FormatError> {
    if slot >= INDEX_SLOTS {
        return Err(corrupt(WHAT, "a slot past the last"));
    }
    // ALLOC: one key, a base and a one or two digit slot.
    Ok([base, "packs/index/", &slot.to_string()].concat())
}

/// The nodes one register names, sorted by id.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PackIndex {
    entries: Vec<(String, u64)>,
}

impl PackIndex {
    /// An index naming no node.
    pub const fn new() -> Self {
        Self {
            // ALLOC: an empty list, no allocation until a node is added.
            entries: Vec::new(),
        }
    }

    /// Whether `node` is named.
    pub fn contains(&self, node: &str) -> bool {
        self.stamp(node).is_some()
    }

    /// The stamp of `node`'s entry.
    pub fn stamp(&self, node: &str) -> Option<u64> {
        self.entries
            .binary_search_by(|(id, _)| id.as_str().cmp(node))
            .ok()
            .and_then(|i| self.entries.get(i))
            .map(|(_, stamp)| *stamp)
    }

    /// The named nodes, sorted.
    pub fn nodes(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(id, _)| id.as_str())
    }

    /// Names `node` with a stamp of at least `stamp` and past its old one,
    /// so the value always changes.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for an id the members index would refuse,
    /// [`FormatError::TooLarge`] when the register would pass its cap; the
    /// index is then unchanged.
    pub fn upsert(&mut self, node: &str, stamp: u64) -> Result<(), FormatError> {
        check_node_id(node)?;
        match self
            .entries
            .binary_search_by(|(id, _)| id.as_str().cmp(node))
        {
            Ok(i) => {
                if let Some(entry) = self.entries.get_mut(i) {
                    entry.1 = stamp.max(entry.1.saturating_add(1));
                }
            }
            Err(i) => {
                if self.encoded_len().saturating_add(entry_len(node)) > MAX_BYTES {
                    return Err(FormatError::TooLarge {
                        what: WHAT,
                        limit: MAX_BYTES as u64,
                        actual: self.encoded_len().saturating_add(entry_len(node)) as u64,
                    });
                }
                // ALLOC: one entry a node, bounded by the register cap checked above.
                self.entries.insert(i, (node.to_owned(), stamp));
            }
        }
        Ok(())
    }

    /// Drops `node`; whether it was named.
    pub fn remove(&mut self, node: &str) -> bool {
        match self
            .entries
            .binary_search_by(|(id, _)| id.as_str().cmp(node))
        {
            Ok(i) => {
                self.entries.remove(i);
                true
            }
            Err(_) => false,
        }
    }

    fn encoded_len(&self) -> usize {
        self.entries
            .iter()
            .fold(PREFIX_BYTES, |n, (id, _)| n.saturating_add(entry_len(id)))
    }

    /// Parses a register's value.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for a short, over-long or unsorted value,
    /// [`FormatError::Unsupported`] for another version.
    pub fn parse(bytes: &[u8]) -> Result<Self, FormatError> {
        let Some((&version, rest)) = bytes.split_first() else {
            return Err(corrupt(WHAT, "empty"));
        };
        if version != VERSION {
            return Err(FormatError::Unsupported {
                what: WHAT,
                version: version.into(),
            });
        }
        let (count, mut body) = rest
            .split_first_chunk::<4>()
            .ok_or(corrupt(WHAT, "no entry count"))?;
        let count = usize::try_from(u32::from_be_bytes(*count))
            .map_err(|_| corrupt(WHAT, "an entry count too large"))?;
        // ALLOC: an empty list; one entry a node, at most the value's length / 10.
        let mut entries: Vec<(String, u64)> = Vec::new();
        for _ in 0..count {
            let (&len, after) = body.split_first().ok_or(corrupt(WHAT, "a short entry"))?;
            let len = usize::from(len);
            if len == 0 || len > MAX_NODE_ID_BYTES || after.len() < len.saturating_add(8) {
                return Err(corrupt(WHAT, "a bad entry"));
            }
            let (id, after) = after.split_at(len);
            let (stamp, after) = after
                .split_first_chunk::<8>()
                .ok_or(corrupt(WHAT, "a short stamp"))?;
            let id = std::str::from_utf8(id).map_err(|_| corrupt(WHAT, "an id not UTF-8"))?;
            if entries.last().is_some_and(|(last, _)| last.as_str() >= id) {
                return Err(corrupt(WHAT, "ids not sorted"));
            }
            // ALLOC: one entry a node, at most the value's length / 10 of a register read.
            entries.push((id.to_owned(), u64::from_be_bytes(*stamp)));
            body = after;
        }
        if !body.is_empty() {
            return Err(corrupt(WHAT, "bytes left over"));
        }
        Ok(Self { entries })
    }

    /// The register value.
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] past the cap.
    pub fn encode(&self) -> Result<Vec<u8>, FormatError> {
        let len = self.encoded_len();
        if len > MAX_BYTES {
            return Err(FormatError::TooLarge {
                what: WHAT,
                limit: MAX_BYTES as u64,
                actual: len as u64,
            });
        }
        let count =
            u32::try_from(self.entries.len()).map_err(|_| corrupt(WHAT, "too many entries"))?;
        // ALLOC: one register value, at most the 256 KiB cap.
        let mut out = Vec::with_capacity(len);
        out.push(VERSION);
        out.extend_from_slice(&count.to_be_bytes());
        for (id, stamp) in &self.entries {
            out.push(u8::try_from(id.len()).map_err(|_| corrupt(WHAT, "an id too long"))?);
            out.extend_from_slice(id.as_bytes());
            out.extend_from_slice(&stamp.to_be_bytes());
        }
        Ok(out)
    }
}

/// Bytes an entry takes: the length byte, the id and the stamp.
fn entry_len(id: &str) -> usize {
    id.len().saturating_add(9)
}
