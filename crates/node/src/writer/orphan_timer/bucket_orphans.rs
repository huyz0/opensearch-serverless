//! The sweep of bucket members no index owns.
//!
//! An index joins its bucket before its descriptor is written, and a delete
//! takes the member out after the descriptor is gone, so a node that dies
//! between the two leaves a member whose descriptor is absent or names
//! another incarnation. [`BucketMemberSweep::slice`] lists a few buckets,
//! reads each manifest and the descriptor of each member's index (one GET a
//! distinct member name), and takes a member out only when it was a
//! candidate at two sightings at least the grace apart: an index that is
//! joining right now has no descriptor yet, and would otherwise lose its
//! member to the sweep. The sightings live in memory, so a sweep that moves
//! to another node starts its grace over.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use oss_blob::BlobStore;
use oss_format::BUCKETS_PREFIX;
use oss_meta::DescriptorStore;

use crate::writer::BucketWiring;

/// The buckets one slice reads: at most this many manifests, so a slice
/// reads at most this times 1,024 descriptors.
pub const BUCKETS_PER_SLICE: usize = 4;

/// The most candidates remembered; one past it is seen again later.
const MAX_SEEN: usize = 4_096;

/// The longest descriptor a slice reads.
const DESCRIPTOR_BYTES: usize = 1 << 16;

/// What one slice did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BucketSlice {
    /// Buckets whose manifest was read.
    pub buckets: usize,
    /// Members seen in them.
    pub members: usize,
    /// Members whose index is gone or has another uuid.
    pub candidates: usize,
    /// Members taken out (counted, not taken, under dry run).
    pub removed: usize,
}

#[derive(Default)]
struct State {
    cursor: Option<String>,
    seen: BTreeMap<(String, String), i64>,
}

/// The sweep over one deployment's buckets.
pub struct BucketMemberSweep {
    wiring: Arc<BucketWiring>,
    store: Arc<dyn BlobStore>,
    descriptors: DescriptorStore,
    base: String,
    grace_millis: i64,
    dry_run: bool,
    state: Mutex<State>,
}

impl BucketMemberSweep {
    /// A sweep that takes a member out after `grace_millis` as a candidate.
    #[must_use]
    pub fn new(
        wiring: Arc<BucketWiring>,
        store: Arc<dyn BlobStore>,
        descriptors: DescriptorStore,
        base: &str,
        grace_millis: i64,
        dry_run: bool,
    ) -> Self {
        Self {
            wiring,
            store,
            descriptors,
            // ALLOC: the base, once a sweep.
            base: base.to_owned(),
            grace_millis,
            dry_run,
            state: Mutex::new(State::default()),
        }
    }

    /// Judges the next `BUCKETS_PER_SLICE` buckets at `now_millis`, one
    /// LIST of one key a bucket, one GET of each manifest and one of each
    /// distinct member's descriptor, and one manifest swap a member taken out.
    ///
    /// # Errors
    ///
    /// The failure of a store read, as text; the cursor stays so the
    /// slice is tried again.
    pub fn slice(&self, now_millis: i64) -> Result<BucketSlice, String> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let (ids, more) = self.next_ids(state.cursor.as_deref())?;
        let mut done = BucketSlice::default();
        // ALLOC: one descriptor buffer a slice, DESCRIPTOR_BYTES.
        let mut buf = vec![0u8; DESCRIPTOR_BYTES];
        for id in &ids {
            let Some(members) = self.wiring.members(id)? else {
                continue;
            };
            done.buckets = done.buckets.saturating_add(1);
            done.members = done.members.saturating_add(members.len());
            let mut orphans = Vec::new();
            for (uuid, name) in members {
                let live = self
                    .descriptors
                    .get(&name, &mut buf)
                    .map_err(|e| e.to_string())?
                    .is_some_and(|record| record.uuid == uuid.as_str());
                if !live {
                    orphans.push(uuid);
                }
            }
            done.candidates = done.candidates.saturating_add(orphans.len());
            // What is no longer a candidate here is forgotten.
            state
                .seen
                .retain(|(bucket, uuid), _| bucket != id || orphans.contains(uuid));
            for uuid in orphans {
                let key = (id.clone(), uuid);
                let first = match state.seen.get(&key) {
                    Some(first) => *first,
                    None if state.seen.len() < MAX_SEEN => {
                        state.seen.insert(key, now_millis);
                        continue;
                    }
                    None => continue,
                };
                if now_millis.saturating_sub(first) < self.grace_millis {
                    continue;
                }
                if !self.dry_run {
                    self.wiring.leave(&key.0, &key.1)?;
                }
                state.seen.remove(&key);
                done.removed = done.removed.saturating_add(1);
            }
        }
        state.cursor = if more { ids.last().cloned() } else { None };
        Ok(done)
    }

    fn next_ids(&self, cursor: Option<&str>) -> Result<(Vec<String>, bool), String> {
        let prefix = format!("{}{BUCKETS_PREFIX}", self.base);
        let start = cursor.map(|id| format!("{prefix}{id}/"));
        // ALLOC: one id a bucket of the page, at most BUCKETS_PER_SLICE.
        let mut ids = Vec::new();
        // LIST: one key a bucket, at most BUCKETS_PER_SLICE, resuming
        // strictly after the cursor; a sweep of buckets is not a per-index
        // or per-request cost.
        let page = self
            .store
            .list_children(&prefix, start.as_deref(), BUCKETS_PER_SLICE, &mut |c| {
                if let Some(id) = c
                    .strip_prefix(prefix.as_str())
                    .map(|rest| rest.strip_suffix('/').unwrap_or(rest))
                {
                    // ALLOC: one id, as above.
                    ids.push(id.to_owned());
                }
            })
            .map_err(|e| e.to_string())?;
        Ok((ids, page.truncated))
    }
}
