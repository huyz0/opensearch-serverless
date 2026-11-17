//! A shard's index and delete operations as `InternalEngine` runs them on
//! a primary: versions, version conflicts, sequence numbers and soft
//! deletes, with no local translog (the node's write log is the
//! translog, as `DiscardingTranslogManager` leaves it).
//!
//! Ported from `IndexingStrategyPlanner`, `DeletionStrategyPlanner`,
//! `VersionType`, `LiveVersionMap` and `VersionsAndSeqNoResolver` (server,
//! OpenSearch 3.10).

mod lookup;
mod plan;
mod shard;
mod version;
mod versions;

pub use plan::{
    Current, Expect, Plan, UNASSIGNED_SEQ_NO, plan, validate, validation_failed, version_errors,
};
pub use shard::{NO_OPS_PERFORMED, Outcome, Refused, Shard, Written};
pub use version::{MATCH_ANY, MATCH_DELETED, NOT_FOUND, VersionType};
pub use versions::{GC_DELETES_MILLIS, VersionMap};

impl Default for Expect {
    fn default() -> Self {
        Expect {
            version: MATCH_ANY,
            version_type: VersionType::Internal,
            if_seq_no: UNASSIGNED_SEQ_NO,
            if_primary_term: 0,
        }
    }
}
