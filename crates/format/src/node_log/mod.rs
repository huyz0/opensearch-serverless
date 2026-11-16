//! A blob of a node's write log: the records of every shard the node wrote
//! in one window, and an index of where each shard's records are.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/nodelog/NodeLogBlob.java`
//! (`encode`, `encodeFence`, `headerEnd`, `decode`, `parseSlice`,
//! `withOrdinal`, `trimBelow`); the key from `NodeLogStore.java` (`name`,
//! `epochPath`): `nodelog/<node>/e=<epoch:020>/<ordinal:020>`.
//!
//! | bytes | field |
//! |---|---|
//! | 4 | magic `0x4E4C4752` (`NLGR`) for records, `0x4E4C4746` (`NLGF`) for a fence, big-endian |
//! | 1 | version: 2 |
//! | 4 | header length, big-endian signed (records only) |
//! | header | node id, epoch (i64), ordinal (i64), trim point (i64); index names (i32 count, then uuid and name each); slices (i32 count, then per slice: name position (i32), shard (i32), term (i64), offset (i64), length (i32), records (i32), max seq no (i64)); running index (i32 count, then per shard: name position, shard, run count (i32), then from and to (i64 each) per run) |
//! | body | one slice per shard, its records one a line ([`WalRecord`]) |
//!
//! A fence is magic, version, the fencing node (a string) and its term.
//! Strings are `DataOutput.writeUTF`. Each index is named once in the
//! header, in the order its shards first appear (slices, then the index);
//! an empty name stands for a name not known. The running index lists, for
//! every shard with unpublished records, the ordinals of this epoch that
//! hold them, so the last blob before a fence tells a successor where every
//! unpublished record is.
//!
//! Stricter than the Java, which writes none of these: a negative epoch,
//! ordinal, trim point, shard, slice length or record count; a trim point
//! past the blob's ordinal; slices that overlap or go back; runs that are
//! empty, overlap or go back; a shard listed twice in the index; and bytes
//! left over in the header.

use std::borrow::Cow;
use std::cmp::Ordering;

use crate::error::{FormatError, corrupt};
use crate::node_lease::java_order;
use crate::wal_record::{EncodedWalRecord, WalRecord};

mod decode;
mod encode;

pub use decode::{decode, header_end};
pub use encode::{encode, encode_fence};

pub(super) const WHAT: &str = "node-log blob";

/// Starts every blob of records.
pub const RECORDS_MAGIC: u32 = 0x4E4C_4752;
/// Starts every fence.
pub const FENCE_MAGIC: u32 = 0x4E4C_4746;
/// The format written, and the only one read.
pub const VERSION: u8 = 2;
/// Magic, version and header length.
pub const PREFIX_BYTES: usize = 4 + 1 + 4;
/// The longest header accepted. At about 70 bytes a shard (its name once,
/// a slice and an index entry) this is some 240,000 shards in one blob; a
/// node holds a few thousand.
pub const MAX_HEADER_BYTES: u32 = 16 * 1024 * 1024;
/// The longest slice: the Java's `int` length.
pub const MAX_SLICE_BYTES: u64 = i32::MAX as u64;

pub(super) const MIN_NAME_BYTES: usize = 2 + 2;
pub(super) const SLICE_BYTES: usize = 4 + 4 + 8 + 8 + 4 + 4 + 8;
pub(super) const MIN_INDEX_BYTES: usize = 4 + 4 + 4;
pub(super) const RUN_BYTES: usize = 8 + 8;

/// One shard of one index incarnation. Equal, ordered and hashed by uuid
/// and shard alone, as the Java's `ShardKey`: the name travels with it so a
/// successor holding only a dead node's log can find the shard's head.
#[derive(Debug, Clone)]
pub struct ShardKey<'a> {
    /// The index's uuid.
    pub index_uuid: Cow<'a, str>,
    /// The shard number, at most `i32::MAX`.
    pub shard: u32,
    /// The index's name, `None` where not known.
    pub index_name: Option<Cow<'a, str>>,
}

impl<'a> ShardKey<'a> {
    /// A key.
    pub fn new(
        index_uuid: impl Into<Cow<'a, str>>,
        shard: u32,
        index_name: Option<impl Into<Cow<'a, str>>>,
    ) -> Self {
        Self {
            index_uuid: index_uuid.into(),
            shard,
            index_name: index_name.map(Into::into),
        }
    }

    /// The key with every string owned.
    pub fn into_owned(self) -> ShardKey<'static> {
        ShardKey {
            index_uuid: Cow::Owned(self.index_uuid.into_owned()),
            shard: self.shard,
            index_name: self.index_name.map(|n| Cow::Owned(n.into_owned())),
        }
    }
}

impl PartialEq for ShardKey<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.index_uuid == other.index_uuid && self.shard == other.shard
    }
}

impl Eq for ShardKey<'_> {}

impl std::hash::Hash for ShardKey<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.index_uuid.hash(state);
        self.shard.hash(state);
    }
}

impl PartialOrd for ShardKey<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ShardKey<'_> {
    /// The Java's order: the uuid as `String.compareTo` (UTF-16 units), then
    /// the shard.
    fn cmp(&self, other: &Self) -> Ordering {
        java_order(&self.index_uuid, &other.index_uuid).then(self.shard.cmp(&other.shard))
    }
}

/// An inclusive run of ordinals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrdinalRange {
    /// The first ordinal.
    pub from: u64,
    /// The last ordinal, at least `from`.
    pub to: u64,
}

/// Where one shard's records are in a blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slice<'a> {
    /// The shard.
    pub shard: ShardKey<'a>,
    /// The writer's term for the shard.
    pub term: i64,
    /// Where the slice starts, from the body's first byte.
    pub offset: u64,
    /// The slice's length.
    pub length: u32,
    /// How many records it holds.
    pub records: u32,
    /// The highest sequence number among them, or [`UNASSIGNED_SEQ_NO`](crate::UNASSIGNED_SEQ_NO).
    pub max_seq_no: i64,
}

/// The header of a blob of records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeLogHeader<'a> {
    /// The node that wrote it.
    pub node_id: Cow<'a, str>,
    /// Its epoch.
    pub epoch: u64,
    /// Its slot within the epoch.
    pub ordinal: u64,
    /// The lowest slot the node still needed; every slot below was deletable.
    pub trim_point: u64,
    /// Where the body starts, from the blob's first byte.
    pub body_offset: u64,
    /// The slices, in body order.
    pub slices: Vec<Slice<'a>>,
    /// For every shard with unpublished records, the runs of slots of this
    /// epoch holding them, this blob's included, in the Java's shard order.
    pub index: Vec<(ShardKey<'a>, Vec<OrdinalRange>)>,
}

impl NodeLogHeader<'_> {
    /// This blob's slices of one shard, in body order.
    pub fn slices_of<'s>(&'s self, shard: &'s ShardKey<'_>) -> impl Iterator<Item = &'s Slice<'s>> {
        self.slices
            .iter()
            .filter(move |s| s.shard.index_uuid == shard.index_uuid && s.shard.shard == shard.shard)
    }

    /// The runs of slots holding a shard's unpublished records; empty when
    /// it has none.
    pub fn ordinals_of(&self, shard: &ShardKey<'_>) -> &[OrdinalRange] {
        self.index
            .iter()
            .find(|(k, _)| k.index_uuid == shard.index_uuid && k.shard == shard.shard)
            .map_or(&[], |(_, runs)| runs.as_slice())
    }

    /// The body's length: the end of its last slice (`NodeLogStore.bodyLength`).
    pub fn body_length(&self) -> u64 {
        self.slices
            .iter()
            .map(|s| s.offset.saturating_add(s.length.into()))
            .max()
            .unwrap_or(0)
    }
}

/// The slot a successor took so the writer can append nothing after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeLogFence<'a> {
    /// The node that put it there.
    pub fenced_by: Cow<'a, str>,
    /// The term that node had just won, for diagnosis only.
    pub term: i64,
}

/// What a slot of an epoch holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeLogEntry<'a> {
    /// A blob of records.
    Records(NodeLogHeader<'a>),
    /// A fence.
    Fence(NodeLogFence<'a>),
}

/// The records one shard wrote at one term in one window.
#[derive(Debug, Clone, Copy)]
pub struct ShardBatch<'b> {
    /// The shard.
    pub shard: &'b ShardKey<'b>,
    /// The writer's term for the shard.
    pub term: i64,
    /// The records, in replay order.
    pub records: &'b [EncodedWalRecord],
}

/// An encoded blob of records and the header that describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedNodeLog {
    /// The blob.
    pub bytes: Vec<u8>,
    /// Its header.
    pub header: NodeLogHeader<'static>,
}

/// The records of one slice, one a line, in replay order; blank lines are
/// skipped, as in the Java's `parseSlice`. Reads one record at a time, so a
/// replay holds one record, not the slice's.
pub fn slice_records(slice: &[u8]) -> SliceRecords<'_> {
    SliceRecords { rest: slice }
}

/// The iterator [`slice_records`] returns.
#[derive(Debug, Clone)]
pub struct SliceRecords<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for SliceRecords<'a> {
    type Item = Result<WalRecord<'a>, FormatError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.rest.is_empty() {
                return None;
            }
            let (line, rest) = match self.rest.iter().position(|b| *b == b'\n') {
                Some(at) => (
                    self.rest.get(..at).unwrap_or_default(),
                    self.rest.get(at.saturating_add(1)..).unwrap_or_default(),
                ),
                None => (self.rest, &[][..]),
            };
            self.rest = rest;
            if !line.is_empty() {
                return Some(WalRecord::parse(line));
            }
        }
    }
}

/// A shard's runs with an ordinal past all of them added, merged into the
/// last run when adjacent (`withOrdinal`).
///
/// # Errors
///
/// [`FormatError::Corrupt`] when the ordinal is not past the last run.
pub fn with_ordinal(runs: &[OrdinalRange], ordinal: u64) -> Result<Vec<OrdinalRange>, FormatError> {
    // ALLOC: the shard's runs plus one.
    let mut out = Vec::with_capacity(runs.len().saturating_add(1));
    out.extend_from_slice(runs);
    if let Some(last) = out.last_mut() {
        if ordinal <= last.to {
            return Err(corrupt(WHAT, "an ordinal is not past the shard's last run"));
        }
        if last.to.checked_add(1) == Some(ordinal) {
            last.to = ordinal;
            return Ok(out);
        }
    }
    out.push(OrdinalRange {
        from: ordinal,
        to: ordinal,
    });
    Ok(out)
}

/// A shard's runs from an ordinal up, the run holding it clipped
/// (`trimBelow`).
pub fn trim_below(runs: &[OrdinalRange], trim_point: u64) -> Vec<OrdinalRange> {
    let kept = runs
        .iter()
        .filter(|r| r.to >= trim_point)
        .map(|r| OrdinalRange {
            from: r.from.max(trim_point),
            to: r.to,
        });
    // ALLOC: at most the shard's runs.
    kept.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(from: u64, to: u64) -> OrdinalRange {
        OrdinalRange { from, to }
    }

    #[test]
    fn with_ordinal_extends_an_adjacent_run_and_starts_a_new_one_otherwise() {
        assert_eq!(with_ordinal(&[], 4).unwrap(), [run(4, 4)]);
        assert_eq!(with_ordinal(&[run(1, 3)], 4).unwrap(), [run(1, 4)]);
        assert_eq!(
            with_ordinal(&[run(1, 2)], 4).unwrap(),
            [run(1, 2), run(4, 4)]
        );
        assert!(with_ordinal(&[run(1, 4)], 4).is_err());
        assert!(with_ordinal(&[run(1, 5)], 4).is_err());
        assert_eq!(
            with_ordinal(&[run(u64::MAX - 1, u64::MAX - 1)], u64::MAX).unwrap(),
            [run(u64::MAX - 1, u64::MAX)]
        );
    }

    #[test]
    fn trim_below_drops_runs_under_the_point_and_clips_the_one_across_it() {
        let runs = [run(0, 1), run(3, 5), run(7, 7)];
        assert_eq!(trim_below(&runs, 0), runs);
        assert_eq!(trim_below(&runs, 2), [run(3, 5), run(7, 7)]);
        assert_eq!(trim_below(&runs, 5), [run(5, 5), run(7, 7)]);
        assert_eq!(trim_below(&runs, 4), [run(4, 5), run(7, 7)]);
        assert_eq!(trim_below(&runs, 8), []);
    }

    #[test]
    fn keys_compare_by_uuid_in_java_order_then_shard_and_ignore_the_name() {
        let a = ShardKey::new("x\u{FFFD}", 0, Some("n"));
        let b = ShardKey::new("x\u{1F600}", 0, None::<&str>);
        assert_eq!(a.cmp(&b), Ordering::Greater);
        assert_eq!(
            ShardKey::new("u", 1, Some("a")),
            ShardKey::new("u", 1, None::<&str>)
        );
        assert!(ShardKey::new("u", 1, None::<&str>) < ShardKey::new("u", 2, None::<&str>));
    }
}
