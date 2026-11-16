//! Document routing: which physical shard a document lives in, computed by
//! Rust nodes exactly as Java nodes compute it, so a document written by
//! one is found by the other.
//!
//! Ported from the Java shell's `routing/RoutingHash`, `UnitLayout`,
//! `ShardMode` (with `UnitLayoutFormat` in `oss-format`) and
//! `ops/DocumentRouting`, with core's `OperationRouting.generateShardId`
//! for an index created without a shard mode. Nothing here reads or writes
//! the store: the layout comes from the index descriptor every request
//! already reads.

mod document;
mod hash;
mod layout;

use std::fmt;

use oss_format::FormatError;

pub use document::{DocumentRouting, normalize, search_routings};
pub use hash::{declared_shard, first_hash_of, last_hash_of, murmur3_x64_h1, routing_hash};
pub use layout::{Split, Unit, UnitLayout};
pub use oss_format::ShardMode;

/// Why a layout or a routing request was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingError {
    /// The layout bytes are not a layout record.
    Format(FormatError),
    /// The layout or the request breaks a rule: the reason.
    Invalid(&'static str),
    /// A split is already pending (the Java's `IllegalStateException`).
    SplitPending,
    /// A partitioned index needs a routing value.
    RoutingMissing,
}

impl fmt::Display for RoutingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Format(e) => e.fmt(f),
            Self::Invalid(reason) => write!(f, "invalid unit layout or routing: {reason}"),
            Self::SplitPending => f.write_str("a split is already pending"),
            Self::RoutingMissing => {
                f.write_str("a routing value is required for a partitioned index")
            }
        }
    }
}

impl std::error::Error for RoutingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Format(e) => Some(e),
            _ => None,
        }
    }
}

impl From<FormatError> for RoutingError {
    fn from(e: FormatError) -> Self {
        Self::Format(e)
    }
}
