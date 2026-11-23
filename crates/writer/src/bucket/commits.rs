//! Writing members' commits into their bucket's manifest.
//!
//! Ported from `BucketCommits` (`swapAll`, `member`, `forget`).

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use oss_format::CommitManifest;

use super::{BucketError, BucketRead, BucketStore};

/// Lost races on the bucket register before giving up: each is a sibling or
/// another owner landing first.
const MAX_ATTEMPTS: usize = 16;

/// One member's swap: the version it read and the commit it writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Swap {
    expected_version: u64,
    commit: Vec<u8>,
}

impl Swap {
    /// A swap of `commit` (a commit register's JSON) for the member entry at
    /// `expected_version`: 0 for a member with no commit.
    #[must_use]
    pub fn new(expected_version: u64, commit: Vec<u8>) -> Self {
        Self {
            expected_version,
            commit,
        }
    }
}

/// How one member's swap ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberOutcome {
    /// The commit landed; the member is at this version.
    Applied(u64),
    /// The member's entry is at another version (`Some`), or it is not a
    /// member (`None`).
    Conflict(Option<u64>),
}

/// One member's latest commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberCommit {
    /// The entry's version.
    pub version: u64,
    /// The commit, as a shard's commit register holds it.
    pub commit: Vec<u8>,
}

type Slot = Arc<Mutex<Option<BucketRead>>>;

/// Writes members' commits into their buckets, one swap for as many members
/// as checkpoint together.
///
/// Writes to one bucket are serialised on this node, and the manifest this
/// node last wrote is kept, so the next checkpoint swaps over it without a
/// read. Another node writing the bucket, which only happens across a
/// takeover, costs one lost swap and one read.
pub struct BucketCommits {
    store: BucketStore,
    slots: Mutex<HashMap<String, Slot>>,
    writes: AtomicU64,
}

impl std::fmt::Debug for BucketCommits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BucketCommits")
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

impl BucketCommits {
    /// Commits over a deployment's buckets.
    #[must_use]
    pub fn new(store: BucketStore) -> Self {
        Self {
            store,
            // ALLOC: one slot a bucket this node has written, bounded by the buckets it owns.
            slots: Mutex::new(HashMap::new()),
            writes: AtomicU64::new(0),
        }
    }

    /// Conditional writes made on bucket registers, applied or not.
    #[must_use]
    pub fn register_writes(&self) -> u64 {
        self.writes.load(Ordering::Relaxed)
    }

    fn slot(&self, bucket: &str) -> Slot {
        let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        // ALLOC: one slot a bucket, once.
        Arc::clone(slots.entry(bucket.to_owned()).or_default())
    }

    /// Forgets what this node last wrote to a bucket, for a caller that
    /// changed it some other way.
    pub fn forget(&self, bucket: &str) {
        *self
            .slot(bucket)
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// One member's latest commit: `None` before its first.
    ///
    /// # Errors
    ///
    /// [`BucketError::NoBucket`] or [`BucketError::NotAMember`], and the
    /// store's and the manifest's failures.
    pub fn member_commit(
        &self,
        bucket: &str,
        index_uuid: &str,
    ) -> Result<Option<MemberCommit>, BucketError> {
        let entry =
            self.store
                .read_member(bucket, index_uuid)?
                .ok_or_else(|| BucketError::NoBucket {
                    // ALLOC: one id in an error.
                    bucket: bucket.to_owned(),
                })?;
        let member = entry.member.ok_or_else(|| BucketError::NotAMember {
            // ALLOC: two ids in an error.
            bucket: bucket.to_owned(),
            uuid: index_uuid.to_owned(),
        })?;
        Ok(member.commit.map(|commit| MemberCommit {
            version: member.version,
            commit,
        }))
    }

    /// Writes many members' commits in one swap of their bucket's manifest.
    /// A member whose entry is no longer at the version it read is left as
    /// it is and told the version it is at; the others land together or not
    /// at all.
    ///
    /// # Errors
    ///
    /// [`BucketError::Format`] for a commit that is not a commit manifest
    /// (before any request), [`BucketError::NoBucket`], [`BucketError::Busy`]
    /// when the register kept moving, and the store's failures.
    pub fn swap_all(
        &self,
        bucket: &str,
        swaps: BTreeMap<String, Swap>,
    ) -> Result<BTreeMap<String, MemberOutcome>, BucketError> {
        if swaps.is_empty() {
            // ALLOC: an empty map holds nothing.
            return Ok(BTreeMap::new());
        }
        for swap in swaps.values() {
            CommitManifest::parse(&swap.commit)?;
        }
        let slot = self.slot(bucket);
        let mut cached = slot.lock().unwrap_or_else(PoisonError::into_inner);
        let mut read = cached.take();
        let mut from_cache = read.is_some();
        for _ in 0..MAX_ATTEMPTS {
            let current = match read.take() {
                Some(current) => current,
                None => self
                    .store
                    .read(bucket)?
                    .ok_or_else(|| BucketError::NoBucket {
                        // ALLOC: one id in an error.
                        bucket: bucket.to_owned(),
                    })?,
            };
            // ALLOC: the manifest being edited: at most 1,024 commits, once an attempt.
            let mut next = current.manifest.clone();
            // ALLOC: one outcome a member of this call.
            let mut results = BTreeMap::new();
            let mut advanced = false;
            for (uuid, swap) in &swaps {
                let outcome = match next.member(uuid) {
                    // Moved out of the bucket: its own read says so, and its siblings still land.
                    None => MemberOutcome::Conflict(None),
                    Some(member) if member.version != swap.expected_version => {
                        MemberOutcome::Conflict(Some(member.version))
                    }
                    Some(member) => {
                        // ALLOC: one commit copy a member of this call, at most manifest-sized.
                        let entry = member.advance(swap.commit.clone())?;
                        let version = entry.version;
                        next = next.with_member(uuid, entry)?;
                        advanced = true;
                        MemberOutcome::Applied(version)
                    }
                };
                // ALLOC: one key a member of this call.
                results.insert(uuid.clone(), outcome);
            }
            if !advanced {
                // Every member had moved: a cached read may be what is stale, so only a fresh one answers.
                if from_cache {
                    from_cache = false;
                    continue;
                }
                // The cache was dropped when it was taken: it is read again next time.
                return Ok(results);
            }
            self.writes.fetch_add(1, Ordering::Relaxed);
            if let Some(swapped) = self.store.swap(&next, current.generation)? {
                *cached = Some(BucketRead {
                    manifest: next,
                    generation: swapped.generation,
                    stored_bytes: swapped.stored_bytes,
                });
                return Ok(results);
            }
            from_cache = false;
        }
        Err(BucketError::Busy {
            attempts: MAX_ATTEMPTS,
        })
    }
}
