//! The node's collect pass: the writer's [`PackCollector`] over the live
//! membership, with the references the node can read.
//!
//! A pack is named when a shard's latest commit records it or a live point
//! in time or snapshot holds the incarnation ([`PassHolds`]). A fresh view
//! of the points in time and snapshots is made for each run, and read only
//! when a pack is due, so an idle run reads nothing.

use std::sync::{Arc, Mutex, PoisonError};

use oss_blob::BlobStore;
use oss_meta::PointInTimeStore;
use oss_writer::pack::{Collected, CommitReferences, PackCollector, PackError};

use crate::pack_holds::{HoldLimits, PassHolds};

/// What a [`PackPass`] is made of.
pub struct PackPassParts {
    /// The deployment's store.
    pub store: Arc<dyn BlobStore>,
    /// The deployment's base key.
    pub base: String,
    /// This node's id.
    pub node_id: String,
    /// The points in time.
    pub pits: Arc<PointInTimeStore>,
    /// The bounds on what a run reads of the views.
    pub limits: HoldLimits,
    /// The most packs the collector tracks, across every node it judges.
    pub max_tracked: usize,
    /// The live members' ids, sorted.
    pub members: Box<dyn Fn() -> Vec<String> + Send + Sync>,
    /// The time in milliseconds.
    pub now: Box<dyn Fn() -> u64 + Send + Sync>,
}

/// Children of `packs/` the seeding walk reads a pass.
const SEED_PAGE: usize = 64;

/// One node's collect pass.
pub struct PackPass {
    parts: PackPassParts,
    collector: Mutex<PackCollector>,
}

impl PackPass {
    /// A pass with nothing tracked yet.
    pub fn new(parts: PackPassParts) -> Self {
        let collector =
            PackCollector::new(Arc::clone(&parts.store), &parts.base, parts.max_tracked)
                .with_seeding(SEED_PAGE);
        Self {
            parts,
            collector: Mutex::new(collector),
        }
    }

    /// Runs the window's pass if this node is elected for it.
    ///
    /// # Errors
    ///
    /// The store's failures, and a reference source that could not be read:
    /// the pass stops and the window is tried again at the next run.
    pub fn run(&self) -> Result<Collected, PackError> {
        let holds = PassHolds::new(
            Arc::clone(&self.parts.pits),
            Arc::clone(&self.parts.store),
            &self.parts.base,
            self.parts.limits,
        );
        let refs = CommitReferences::new(
            Arc::clone(&self.parts.store),
            &self.parts.base,
            Arc::new(holds),
        )?;
        let mut members = (self.parts.members)();
        members.sort();
        let mut collector = self
            .collector
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        collector.tick(&self.parts.node_id, &members, (self.parts.now)(), &refs)
    }
}
