//! The collector's judgement of one node's packs.
//!
//! A pack is judged only past [`JUDGE_AGE_MILLIS`] of the store's age, and
//! deleted only after two sightings with nothing naming it, a
//! [`SIGHTING_GAP_MILLIS`] apart. A pack found named is asked about again
//! after the age it was judged at, then twice that, and so on, so a pack
//! costs a logarithmic number of questions over its life and none between.
//! Every schedule is in memory: a restart judges each live pack again.

use std::collections::BTreeMap;
use std::sync::Arc;

use oss_blob::BlobStore;
use oss_format::node_pack_key;

use super::{PackError, PackSidecarStore, SidecarState};

/// How old a pack must be, by the store's clock, before it is judged.
pub const JUDGE_AGE_MILLIS: u64 = 15 * 60_000;

/// How long after a pack is first seen unreferenced it is seen again.
pub const SIGHTING_GAP_MILLIS: u64 = 60_000;

/// One pack's name: whose it is, and where in the node's epochs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackId<'a> {
    /// The node that wrote the pack.
    pub node: &'a str,
    /// The pack's epoch.
    pub epoch: u64,
    /// The pack's slot in its epoch.
    pub ordinal: u64,
}

/// Whether anything names a pack. Every place that persists a manifest, a
/// bucket manifest or a file location is a source (see
/// `docs/pack-reference-sources.tsv`), and there is no implementation that
/// answers "no" without looking: a source that cannot be read is an error,
/// and the pass stops.
pub trait PackReferences {
    /// Whether any source names `pack`.
    ///
    /// # Errors
    ///
    /// A source that could not be read.
    fn names(&self, pack: &PackId<'_>) -> Result<bool, PackError>;
}

/// What one pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Judged {
    /// Packs deleted.
    pub deleted: usize,
    /// Packs found named, now on their backoff.
    pub referenced: usize,
    /// Packs found younger than the judging age.
    pub young: usize,
    /// Packs left untracked because the tracking cap was reached.
    pub at_cap: bool,
}

/// Where one tracked pack stands.
#[derive(Clone, Copy, Debug)]
struct Entry {
    /// The judge's clock, in milliseconds, when the pack is next due.
    due: u64,
    /// The interval after the next recheck of a named pack.
    interval: u64,
    /// When it was first seen unreferenced, if it has been.
    unseen_since: Option<u64>,
    /// Whether the store's age has been found past the judging age.
    old: bool,
}

/// The judge of one node's packs.
pub struct PackJudge {
    store: Arc<dyn BlobStore>,
    base: String,
    node: String,
    max_tracked: usize,
    tracked: BTreeMap<(u64, u64), Entry>,
}

impl std::fmt::Debug for PackJudge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackJudge")
            .field("node", &self.node)
            .field("tracked", &self.tracked.len())
            .finish_non_exhaustive()
    }
}

impl PackJudge {
    /// A judge of `node`'s packs under `base`, tracking at most `max_tracked`.
    ///
    /// # Errors
    ///
    /// [`PackError::Format`] when the node id cannot be part of a key.
    pub fn new(
        store: Arc<dyn BlobStore>,
        base: &str,
        node: &str,
        max_tracked: usize,
    ) -> Result<Self, PackError> {
        node_pack_key(base, node, 0, 0)?;
        Ok(Self {
            store,
            // ALLOC: one base, once a judge.
            base: base.to_owned(),
            // ALLOC: one node id, once a judge.
            node: node.to_owned(),
            max_tracked,
            // ALLOC: an empty map, no allocation until a pack is tracked.
            tracked: BTreeMap::new(),
        })
    }

    /// One pass at judge-clock time `now` (milliseconds on a clock that only
    /// moves forward): judges every live pack of the sidecar that is due,
    /// deletes those past their second sighting, and records them as
    /// collected. Nothing is requested for a pack that is not due.
    ///
    /// # Errors
    ///
    /// The store's failures; a reference source that could not be read.
    /// Marks made before the failure are kept.
    pub fn pass(
        &mut self,
        sidecar: &PackSidecarStore,
        refs: &dyn PackReferences,
        now: u64,
    ) -> Result<Judged, PackError> {
        let state = sidecar.read()?;
        self.pass_with(sidecar, state, refs, now)
            .map(|(out, _)| out)
    }

    /// As [`PackJudge::pass`] from a sidecar the caller has already read,
    /// returning the sidecar as it stands after the marks.
    ///
    /// # Errors
    ///
    /// As [`PackJudge::pass`].
    pub fn pass_with(
        &mut self,
        sidecar: &PackSidecarStore,
        state: SidecarState,
        refs: &dyn PackReferences,
        now: u64,
    ) -> Result<(Judged, SidecarState), PackError> {
        let mut out = Judged::default();
        // ALLOC: the packs collected in one pass, at most the tracked cap
        // plus the packs found missing, each sixteen bytes.
        let mut collected: Vec<(u64, u64)> = Vec::new();
        let mut failure = None;
        'epochs: for epoch in state.sidecar.epochs() {
            for ordinal in epoch.unmarked() {
                let id = (epoch.epoch, ordinal);
                match self.judge_one(id, refs, now, &mut out) {
                    Ok(true) => collected.push(id),
                    Ok(false) => {}
                    Err(e) => {
                        failure = Some(e);
                        break 'epochs;
                    }
                }
            }
        }
        let state = if collected.is_empty() {
            state
        } else {
            for id in &collected {
                self.tracked.remove(id);
            }
            sidecar.update(state, |current| {
                let mut next = current.clone();
                for epoch in next.epochs_mut() {
                    for (e, o) in &collected {
                        if *e == epoch.epoch {
                            epoch.mark(*o);
                        }
                    }
                }
                next
            })?
        };
        match failure {
            Some(e) => Err(e),
            None => Ok((out, state)),
        }
    }

    /// Judges one pack if it is due; whether it is now collected.
    fn judge_one(
        &mut self,
        id: (u64, u64),
        refs: &dyn PackReferences,
        now: u64,
        out: &mut Judged,
    ) -> Result<bool, PackError> {
        let entry = match self.tracked.get(&id) {
            Some(entry) if now < entry.due => return Ok(false),
            Some(entry) => *entry,
            None if self.tracked.len() >= self.max_tracked => {
                out.at_cap = true;
                return Ok(false);
            }
            None => Entry {
                due: now,
                interval: JUDGE_AGE_MILLIS,
                unseen_since: None,
                old: false,
            },
        };
        let key = node_pack_key(&self.base, &self.node, id.0, id.1)?;
        let mut entry = entry;
        if !entry.old {
            let Some(age) = self.store.age_millis(&key)? else {
                return Ok(true);
            };
            if age < JUDGE_AGE_MILLIS {
                entry.due = now.saturating_add(JUDGE_AGE_MILLIS.saturating_sub(age));
                self.tracked.insert(id, entry);
                out.young = out.young.saturating_add(1);
                return Ok(false);
            }
            entry.old = true;
        }
        let pack = PackId {
            node: &self.node,
            epoch: id.0,
            ordinal: id.1,
        };
        if refs.names(&pack)? {
            entry.unseen_since = None;
            entry.due = now.saturating_add(entry.interval);
            entry.interval = entry.interval.saturating_mul(2);
            self.tracked.insert(id, entry);
            out.referenced = out.referenced.saturating_add(1);
            return Ok(false);
        }
        match entry.unseen_since {
            Some(since) if now.saturating_sub(since) >= SIGHTING_GAP_MILLIS => {
                self.store.delete(&key)?;
                out.deleted = out.deleted.saturating_add(1);
                Ok(true)
            }
            Some(_) => {
                entry.due = now.saturating_add(SIGHTING_GAP_MILLIS);
                self.tracked.insert(id, entry);
                Ok(false)
            }
            None => {
                entry.unseen_since = Some(now);
                entry.due = now.saturating_add(SIGHTING_GAP_MILLIS);
                self.tracked.insert(id, entry);
                Ok(false)
            }
        }
    }
}
