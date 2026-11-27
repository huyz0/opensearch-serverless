//! The collector's tick: the pass one elected node runs a window over the
//! nodes the pack index names.
//!
//! A pass reads the 16 index registers, and for each node judges its live
//! packs ([`PackJudge`]). A node whose sidecar holds no epoch has no live
//! pack: it is dropped from the index by compare-and-swap against the
//! generation read before the sidecar, so a writer that named itself in
//! between is not lost. An open epoch keeps its node listed, which costs
//! one sidecar read a pass. Every schedule is in memory.

use std::collections::BTreeMap;
use std::sync::Arc;

use oss_blob::BlobStore;
use oss_format::INDEX_SLOTS;

use super::election::{collector_for, window_of};
use super::index::Removal;
use super::{
    PackError, PackIndexStore, PackJudge, PackLedger, PackReferences, PackSidecarStore, sync,
};

/// What one tick did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Collected {
    /// Whether this node ran the window's pass.
    pub ran: bool,
    /// Nodes whose packs were judged.
    pub nodes: usize,
    /// Packs deleted.
    pub deleted: usize,
    /// Packs found named.
    pub referenced: usize,
    /// Packs found too young to judge.
    pub young: usize,
    /// Nodes dropped from the index.
    pub dropped: usize,
    /// Whether a node's tracking stopped at its cap.
    pub at_cap: bool,
}

/// One node's collector: runs the passes it is elected for.
pub struct PackCollector {
    store: Arc<dyn BlobStore>,
    base: String,
    index: PackIndexStore,
    max_tracked: usize,
    judges: BTreeMap<String, PackJudge>,
    last_window: Option<u64>,
}

impl PackCollector {
    /// A collector over the packs under `base`, tracking at most
    /// `max_tracked` packs a node.
    pub fn new(store: Arc<dyn BlobStore>, base: &str, max_tracked: usize) -> Self {
        Self {
            index: PackIndexStore::new(Arc::clone(&store), base),
            store,
            // ALLOC: the base, once a collector.
            base: base.to_owned(),
            max_tracked,
            // ALLOC: an empty map, one judge a node with a live epoch.
            judges: BTreeMap::new(),
            last_window: None,
        }
    }

    /// Runs the pass of `now_millis`'s window if `me` is the member elected
    /// for it among `members` and has not run it. Nothing is requested
    /// otherwise.
    ///
    /// # Errors
    ///
    /// The store's failures; a reference source that could not be read. The
    /// window is not counted as run, so the next tick tries it again.
    pub fn tick(
        &mut self,
        me: &str,
        members: &[String],
        now_millis: u64,
        refs: &dyn PackReferences,
    ) -> Result<Collected, PackError> {
        let window = window_of(now_millis);
        let mut out = Collected::default();
        if self.last_window == Some(window) || collector_for(window, members) != Some(me) {
            return Ok(out);
        }
        out.ran = true;
        for slot in 0..INDEX_SLOTS {
            let state = self.index.read(slot)?;
            for node in state.index.nodes() {
                let alive = members.iter().any(|m| m == node);
                let live = self.judge_node(node, alive, refs, now_millis, &mut out)?;
                // A node with nothing left to judge and no lease leaves the
                // index; the swap fails if a writer added it since the read.
                if !live && !alive && self.index.remove(slot, &state, node)? == Removal::Removed {
                    self.judges.remove(node);
                    out.dropped = out.dropped.saturating_add(1);
                }
            }
        }
        self.last_window = Some(window);
        Ok(out)
    }

    /// Builds the node's sidecar from its ledger, judges its packs, and
    /// advances its ledger; whether it still has packs that may exist.
    fn judge_node(
        &mut self,
        node: &str,
        alive: bool,
        refs: &dyn PackReferences,
        now: u64,
        out: &mut Collected,
    ) -> Result<bool, PackError> {
        let sidecar = PackSidecarStore::new(Arc::clone(&self.store), &self.base, node)?;
        let ledger = PackLedger::new(Arc::clone(&self.store), &self.base, node)?;
        let from = ledger.read()?;
        let state = sidecar.update(sidecar.read()?, |current| {
            sync::synced(&from.epochs, current, alive)
        })?;
        if !self.judges.contains_key(node) {
            let judge =
                PackJudge::new(Arc::clone(&self.store), &self.base, node, self.max_tracked)?;
            // ALLOC: one judge a node with a live epoch, at most the nodes the index names.
            self.judges.insert(node.to_owned(), judge);
        }
        let Some(judge) = self.judges.get_mut(node) else {
            return Ok(false);
        };
        let (judged, state) = judge.pass_with(&sidecar, state, refs, now)?;
        out.nodes = out.nodes.saturating_add(1);
        out.deleted = out.deleted.saturating_add(judged.deleted);
        out.referenced = out.referenced.saturating_add(judged.referenced);
        out.young = out.young.saturating_add(judged.young);
        out.at_cap |= judged.at_cap;
        ledger.update(from, |epochs| sync::advanced(epochs, &state.sidecar))?;
        Ok(sync::has_live(&state.sidecar))
    }
}
