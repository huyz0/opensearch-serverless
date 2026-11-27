//! Node packs: one blob holding the segment blobs that many of a node's
//! shards publish together, so the node pays one PUT for all of them.
//!
//! Ported from `NodePackStore`, `NodePackWriter` and `NodePackLedger`
//! (huyz0/OpenSearch `feature/serverlessshell`). The publisher's use of a
//! pack, and the collector that deletes unreferenced ones, are separate.

mod commit_refs;
mod election;
mod error;
mod index;
mod judge;
mod ledger;
mod sidecar;
mod store;
mod writer;

pub use commit_refs::{CommitReferences, IndexHolds};
pub use election::{WINDOW_MILLIS, collector_for, window_of};
pub use error::PackError;
pub use index::{IndexState, PackIndexStore, Removal};
pub use judge::{JUDGE_AGE_MILLIS, Judged, PackId, PackJudge, PackReferences, SIGHTING_GAP_MILLIS};
pub use ledger::{LedgerState, PackLedger};
pub use sidecar::{PackSidecarStore, SidecarState};
pub use store::{Landed, PackStore, Part};
pub use writer::{COLLECTABLE_AFTER_MILLIS, PackWriter, RESERVE_AHEAD, SWAP_DEADLINE_MILLIS};
