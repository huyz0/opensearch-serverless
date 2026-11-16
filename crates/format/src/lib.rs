//! The Java serverless shell's blob formats.
//!
//! Readers for the blobs a shard's storage is made of, ported from the shell
//! so Rust and Java nodes read the same bucket. From a few register values,
//! every segment file of a shard's latest commit maps to a blob key and a
//! range, with no LIST:
//!
//! 1. the descriptor register `indices/<index>` gives the index's uuid
//!    ([`DescriptorRecord`]);
//! 2. the commit register `segments/<index>#<uuid>#<shard>/manifest` gives the
//!    commit ([`CommitManifest`]);
//! 3. [`ShardRef::resolve`] turns each file's recorded location into a key
//!    and range ([`FileExtent`]).
//!
//! The headers of segment blobs ([`SegmentBlobHeader`]) and node packs
//! ([`NodePackHeader`]) describe the same ranges again, for checking a blob or
//! opening one without its manifest. Pure parsing over byte slices: no I/O,
//! and nothing borrowed from the input is copied unless it holds an escape.
//! Metadata writers write what they read: [`IndexRecord`] and [`Tombstone`]
//! are a name register's whole value, written back byte for byte as the
//! shell's Jackson writes it, into the caller's buffer; so is an
//! [`AliasRecord`]. A delete's [`ReclaimIntent`] is written as the Java
//! writes it and read as leniently. [`ClusterSettings`]
//! reads the cluster config register as the Java's `Settings` reads it, and
//! writes the flat form that reads back the same. A node's [`NodeLease`],
//! the members index and the [`LeaseDigest`] are read and written byte for
//! byte as the shell's `NodeLease` and `LeaseDigest` (`members`).
//!
//! A node's write log is read and written as the shell's `NodeLogBlob`
//! and `WalRecord` write it: [`node_log_format::encode`] lays a window's
//! [`WalRecord`]s out as one blob, [`node_log_format::decode`] reads a
//! slot's header or fence, and [`slice_records`] reads one shard's records.
//!
//! Bucketed indices, whose commits are in a bucket's manifest, are not read
//! yet ([`FormatError::NotFollowable`]).

mod alias_record;
mod change_stripe;
mod cluster_settings;
mod descriptor;
mod error;
mod index_record;
mod json;
mod keys;
mod location;
mod manifest;
mod members;
mod mutf8;
mod node_lease;
mod node_log;
mod node_pack;
mod reclaim_intent;
mod register;
mod segment_blob;
mod tombstone;
mod transport_secret;
mod unit_layout;
mod wal_record;

pub use alias_record::{ALIAS_DISCRIMINATOR, AliasMutation, AliasRecord};
pub use change_stripe::{
    ChangeKind, ChangeStripe, KEPT, KEPT_PENDING, MAX_CHANGE_STRIPE_BYTES, MAX_ENTRY_NAME_BYTES,
    STRIPES, StripeEntry, stripe_of,
};
pub use cluster_settings::{ClusterSettings, MAX_CLUSTER_SETTINGS_BYTES, Setting, SettingsRefused};
pub use descriptor::{DescriptorRecord, IndexDescriptor, TOMBSTONE_PREFIX};
pub use error::FormatError;
pub use index_record::{EMPTY_SETTINGS, IndexRecord, MAX_SHARDS, NameRecord};
pub use keys::{
    CHANGES_PREFIX, CLUSTER_CONFIG_KEY, DESCRIPTORS_PREFIX, FileExtent, MANIFEST, MARKERS_PREFIX,
    NODE_LOG_PREFIX, RECLAIM_PREFIX, ShardRef, TRANSPORT_SECRET_KEY, change_stripe_key,
    cluster_config_key, descriptor_key, node_log_key, node_pack_key, reclaim_intent_key,
    tombstone_marker_key, transport_secret_key,
};
pub use location::{FileLocation, NODE_PACK_PREFIX};
pub use manifest::{CommitManifest, ManifestFile};
pub use members::{
    LEASE_PREFIX, LeaseDigest, MAX_NODE_ID_BYTES, MEMBERS_PREFIX, check_node_id, decode_digest,
    decode_members, encode_digest, encode_members, lease_digest_key, lease_key, members_index_key,
};
pub use node_lease::{MAX_LEASE_BYTES, NodeLease, NodeLogEpoch, java_order};
pub use node_log::{
    EncodedNodeLog, NodeLogEntry, NodeLogFence, NodeLogHeader, OrdinalRange, ShardBatch, ShardKey,
    Slice, SliceRecords, slice_records, trim_below, with_ordinal,
};
pub use node_pack::{Block, NodePackHeader};
pub use reclaim_intent::{
    MAX_RECLAIM_INTENT_BYTES, ReclaimIntent, reclaim_bucket, reclaim_bucket_start,
};
pub use register::{GENERATION_BYTES, Register};
pub use segment_blob::{SegmentBlobHeader, SegmentFile};
pub use tombstone::{MAX_TOMBSTONE_BYTES, Tombstone, tombstone_bucket, tombstone_bucket_start};

/// Node pack format constants and the header-length probe.
pub mod node_pack_format {
    pub use crate::node_pack::{
        MAGIC, MAX_HEADER_BYTES, PREFIX_BYTES, VERSION, VERSION_UUID_ONLY, header_end,
    };
}

/// Node write-log blob format constants, the header-length probe, and the
/// encoders and decoder.
pub mod node_log_format {
    pub use crate::node_log::{
        FENCE_MAGIC, MAX_HEADER_BYTES, MAX_SLICE_BYTES, PREFIX_BYTES, RECORDS_MAGIC, VERSION,
        decode, encode, encode_fence, header_end,
    };
}

/// Segment blob format constants and the header-length probe.
pub mod segment_blob_format {
    pub use crate::segment_blob::{MAGIC, MAX_FILES, MAX_HEADER_BYTES, PREFIX_BYTES, header_end};
}

/// Commit manifest limits.
pub mod manifest_format {
    pub use crate::manifest::MAX_FILES;
}
pub use transport_secret::{MAX_TRANSPORT_SECRET_BYTES, TransportSecret};
pub use unit_layout::{MAX_UNIT_LAYOUT_BYTES, ShardMode, UnitLayoutRecord, UnitRecord};
pub use wal_record::{
    EncodedWalRecord, MATCH_ANY, UNASSIGNED_PRIMARY_TERM, UNASSIGNED_SEQ_NO, WalRecord,
};
