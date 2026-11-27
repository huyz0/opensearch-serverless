//! The collector's tick: the pass one elected node runs a window over the
//! nodes the pack index names.
//!
//! A pass reads the 16 index registers, and for each node judges its live
//! packs ([`PackJudge`]). A node whose sidecar holds no epoch has no live
//! pack: it is dropped from the index by compare-and-swap against the
//! generation read before the sidecar, so a writer that named itself in
//! between is not lost. An open epoch keeps its node listed, which costs
//! one sidecar read a pass. Every schedule is in memory.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use oss_blob::BlobStore;
use oss_format::{INDEX_SLOTS, check_node_id, pack_index_slot};

use super::election::{collector_for, window_of};
use super::index::{IndexState, Removal};
use super::{
    PackError, PackIndexStore, PackJudge, PackLedger, PackReferences, PackSeeder, PackSidecarStore,
    Seeded, sync,
};

/// How long a live member with no ledger is left alone before its ledger is
/// read again.
const IDLE_RECHECK_MILLIS: u64 = 60 * 60_000;

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
    seeder: Option<PackSeeder>,
    adopt_members: bool,
    idle: BTreeMap<String, u64>,
    settled: BTreeMap<String, u64>,
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
            seeder: None,
            adopt_members: false,
            // ALLOC: an empty map, one entry a live member with no ledger.
            idle: BTreeMap::new(),
            // ALLOC: an empty map, one entry a live node with nothing to judge.
            settled: BTreeMap::new(),
            last_window: None,
        }
    }

    /// Finds the nodes no index names, which is off by default: walks
    /// `packs/` once a cluster, `page` children a pass, for the nodes that
    /// left before any collector ran, and names each live member the index
    /// does not (a Java node never names itself) at first sight.
    #[must_use]
    pub fn with_seeding(mut self, page: usize) -> Self {
        self.seeder = Some(PackSeeder::new(Arc::clone(&self.store), &self.base, page));
        self.adopt_members = true;
        self
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
        self.seed(now_millis)?;
        // ALLOC: the 16 index registers as read, one pass.
        let mut states = Vec::with_capacity(usize::from(INDEX_SLOTS));
        for slot in 0..INDEX_SLOTS {
            states.push(self.index.read(slot)?);
        }
        self.adopt(members, &states, now_millis)?;
        // ALLOC: the live members as a set, one pass, at most the members.
        let live_members: HashSet<&str> = members.iter().map(String::as_str).collect();
        for (slot, state) in (0..INDEX_SLOTS).zip(&states) {
            for node in state.index.nodes() {
                let alive = live_members.contains(node);
                let live = self.judge_node(node, alive, refs, now_millis, &mut out)?;
                // A node with nothing left to judge and no lease leaves the
                // index; the swap fails if a writer added it since the read.
                if !live && !alive && self.index.remove(slot, state, node)? == Removal::Removed {
                    self.judges.remove(node);
                    self.idle.remove(node);
                    self.settled.remove(node);
                    out.dropped = out.dropped.saturating_add(1);
                }
            }
        }
        self.last_window = Some(window);
        Ok(out)
    }

    /// Names each live member that the registers just read do not, once: a
    /// member already named costs no write. It is judged from the next pass.
    fn adopt(&self, members: &[String], states: &[IndexState], now: u64) -> Result<(), PackError> {
        if !self.adopt_members {
            return Ok(());
        }
        for member in members {
            if check_node_id(member).is_err() {
                continue;
            }
            let named = states
                .get(usize::from(pack_index_slot(member)))
                .is_some_and(|s| s.index.contains(member));
            if !named {
                self.index.add(member, now)?;
            }
        }
        Ok(())
    }

    /// One step of the seeding walk, if it is on and not done. A done walk
    /// is remembered, so it costs nothing after.
    fn seed(&mut self, now: u64) -> Result<(), PackError> {
        let Some(seeder) = &self.seeder else {
            return Ok(());
        };
        let step = seeder.step(now)?;
        if matches!(step, Seeded::Done | Seeded::Page { done: true, .. }) {
            self.seeder = None;
        }
        Ok(())
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
        if alive && self.idle.get(node).is_some_and(|until| now < *until) {
            return Ok(true);
        }
        let ledger = PackLedger::new(Arc::clone(&self.store), &self.base, node)?;
        let from = ledger.read()?;
        if alive && from.epochs.is_empty() {
            // A live member that has written nothing: named, but not worth
            // a sidecar read, and asked about again in an hour.
            // ALLOC: one entry a live member with no ledger, at most the members.
            self.idle
                .insert(node.to_owned(), now.saturating_add(IDLE_RECHECK_MILLIS));
            return Ok(true);
        }
        self.idle.remove(node);
        if alive && self.settled.get(node) == Some(&from.generation) {
            // Nothing was left to judge under this very ledger, and only a
            // ledger that moves can bring something: no sidecar read.
            return Ok(true);
        }
        let sidecar = PackSidecarStore::new(Arc::clone(&self.store), &self.base, node)?;
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
        let after = ledger.update(from, |epochs| sync::advanced(epochs, &state.sidecar))?;
        let live = sync::has_live(&state.sidecar);
        if !live {
            // ALLOC: one entry a node with nothing to judge, at most the members.
            self.settled.insert(node.to_owned(), after.generation);
        } else {
            self.settled.remove(node);
        }
        Ok(live)
    }
}
