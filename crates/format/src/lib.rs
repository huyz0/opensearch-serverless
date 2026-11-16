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
//! shell's Jackson writes it, into the caller's buffer.
//!
//! Bucketed indices, whose commits are in a bucket's manifest, are not read
//! yet ([`FormatError::NotFollowable`]).

mod change_stripe;
mod descriptor;
mod error;
mod index_record;
mod json;
mod keys;
mod location;
mod manifest;
mod mutf8;
mod node_pack;
mod register;
mod segment_blob;
mod tombstone;

pub use change_stripe::{
    ChangeKind, ChangeStripe, KEPT, KEPT_PENDING, MAX_CHANGE_STRIPE_BYTES, MAX_ENTRY_NAME_BYTES,
    STRIPES, StripeEntry, stripe_of,
};
pub use descriptor::{DescriptorRecord, IndexDescriptor, TOMBSTONE_PREFIX};
pub use error::FormatError;
pub use index_record::{EMPTY_SETTINGS, IndexRecord, MAX_SHARDS, NameRecord};
pub use keys::{
    CHANGES_PREFIX, DESCRIPTORS_PREFIX, FileExtent, MANIFEST, MARKERS_PREFIX, ShardRef,
    change_stripe_key, descriptor_key, node_pack_key, tombstone_marker_key,
};
pub use location::{FileLocation, NODE_PACK_PREFIX};
pub use manifest::{CommitManifest, ManifestFile};
pub use node_pack::{Block, NodePackHeader};
pub use register::{GENERATION_BYTES, Register};
pub use segment_blob::{SegmentBlobHeader, SegmentFile};
pub use tombstone::{MAX_TOMBSTONE_BYTES, Tombstone, tombstone_bucket, tombstone_bucket_start};

/// Node pack format constants and the header-length probe.
pub mod node_pack_format {
    pub use crate::node_pack::{
        MAGIC, MAX_HEADER_BYTES, PREFIX_BYTES, VERSION, VERSION_UUID_ONLY, header_end,
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
