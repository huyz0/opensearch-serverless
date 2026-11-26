//! Node packs: one blob holding the segment blobs that many of a node's
//! shards publish together, so the node pays one PUT for all of them.
//!
//! Ported from `NodePackStore`, `NodePackWriter` and `NodePackLedger`
//! (huyz0/OpenSearch `feature/serverlessshell`). The publisher's use of a
//! pack, and the collector that deletes unreferenced ones, are separate.

mod error;
mod ledger;
mod sidecar;
mod store;
mod writer;

pub use error::PackError;
pub use ledger::{LedgerState, PackLedger};
pub use sidecar::{PackSidecarStore, SidecarState};
pub use store::{Landed, PackStore, Part};
pub use writer::{COLLECTABLE_AFTER_MILLIS, PackWriter, RESERVE_AHEAD, SWAP_DEADLINE_MILLIS};
