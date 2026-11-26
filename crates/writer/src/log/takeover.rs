//! What a successor does first to a predecessor's write log: fence every
//! epoch its lease names, once, so the predecessor can land nothing after
//! what is read, and read each epoch's last header below its fence.
//!
//! Every shard taken from one predecessor shares those fences, and a
//! second successor taking another of its shards shares them too, through
//! [`NodeLogStore::fence`]. Nothing lists the store: the epochs come from
//! the lease and the slots are searched from its landed hint.
//!
//! The predecessor deletes behind a trim point only once a lease naming it
//! has landed, but the lease a successor started from may be older. Its
//! landed hint can then point into what was deleted, and a fence searching
//! from there lands in a hole below the writer's tail, which the writer
//! never reaches. Any deletion that made such a hole followed a lease
//! naming a trim point above it, so reading the lease again after the
//! fence finds a trim point above the fence exactly when the fence may be
//! in a hole, and the fence is searched for again from that lease's hint.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use oss_format::{NodeLogEpoch, NodeLogHeader, ShardKey};

use crate::log::{LogError, NodeLogStore, Refusal, lease_slot};

mod read;

pub use read::BodyCache;

/// How many times a fence is searched for again after the predecessor's
/// lease moved past it (the Java's `MAX_REFENCES`).
pub const MAX_REFENCES: u32 = 8;

/// Reads the predecessor's lease again, for its epochs as they stand now.
pub trait LeaseReader: Send + Sync {
    /// The epochs the lease names now; empty if it is gone.
    ///
    /// # Errors
    ///
    /// When the lease cannot be read.
    fn epochs(&self) -> Result<Vec<NodeLogEpoch>, Refusal>;
}

impl<F> LeaseReader for F
where
    F: Fn() -> Result<Vec<NodeLogEpoch>, Refusal> + Send + Sync,
{
    fn epochs(&self) -> Result<Vec<NodeLogEpoch>, Refusal> {
        self()
    }
}

/// Why fencing a predecessor's log failed; the next call tries again.
#[derive(Debug, Clone)]
pub enum TakeoverError {
    /// A fence or a read of the log failed.
    Log(LogError),
    /// The predecessor's lease could not be read again.
    Lease(Refusal),
}

impl fmt::Display for TakeoverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Log(e) => write!(f, "the predecessor's log: {e}"),
            Self::Lease(e) => write!(f, "the predecessor's lease: {e}"),
        }
    }
}

impl std::error::Error for TakeoverError {}

impl From<LogError> for TakeoverError {
    fn from(e: LogError) -> Self {
        Self::Log(e)
    }
}

/// What fencing found, read once and shared by every shard taken.
#[derive(Debug)]
pub(crate) struct Fenced {
    /// Per epoch, the slot its fence took, or its first free slot for a
    /// reader that fences nothing.
    pub(crate) ends: BTreeMap<u64, u64>,
    /// Per epoch, the last header below its end and at or above its trim
    /// point; `None` when it held no records there.
    pub(crate) last_headers: BTreeMap<u64, Option<Arc<NodeLogHeader<'static>>>>,
}

/// A successor's hold on one predecessor's log.
pub struct NodeLogTakeover {
    log: NodeLogStore,
    epochs: Vec<NodeLogEpoch>,
    fenced_by: String,
    term: i64,
    reread: Option<Box<dyn LeaseReader>>,
    fence: bool,
    fenced: Mutex<Option<Arc<Fenced>>>,
    /// Headers read for one shard, kept for the others in the same blob.
    headers: Mutex<BTreeMap<(u64, u64), Arc<NodeLogHeader<'static>>>>,
    /// Bodies read whole, kept for the other shards in the same blob.
    bodies: Mutex<read::Bodies>,
}

impl NodeLogTakeover {
    /// A reader of `log` that fences the `epochs` its lease named, as
    /// `fenced_by` at `term` (recorded in each fence for diagnosis), and
    /// trusts them: for this node's own log after a restart.
    #[must_use]
    pub fn new(log: NodeLogStore, epochs: Vec<NodeLogEpoch>, fenced_by: &str, term: i64) -> Self {
        Self::build(log, epochs, fenced_by, term, None, true)
    }

    /// As [`NodeLogTakeover::new`], checking each fence against the
    /// predecessor's lease as `reread` finds it after the fence.
    #[must_use]
    pub fn rereading(
        log: NodeLogStore,
        epochs: Vec<NodeLogEpoch>,
        fenced_by: &str,
        term: i64,
        reread: Box<dyn LeaseReader>,
    ) -> Self {
        Self::build(log, epochs, fenced_by, term, Some(reread), true)
    }

    /// A reader of a log whose writer has let go of the shards it will be
    /// asked about: it reads up to each epoch's first free slot and fences
    /// nothing, so the writer's other shards go on writing. Each end is
    /// checked against the lease as `reread` finds it afterwards, as a
    /// fence is.
    #[must_use]
    pub fn unfenced(
        log: NodeLogStore,
        epochs: Vec<NodeLogEpoch>,
        reread: Box<dyn LeaseReader>,
    ) -> Self {
        // ALLOC: one node id, once a reader.
        let node = log.node_id().to_owned();
        Self::build(log, epochs, &node, 0, Some(reread), false)
    }

    fn build(
        log: NodeLogStore,
        epochs: Vec<NodeLogEpoch>,
        fenced_by: &str,
        term: i64,
        reread: Option<Box<dyn LeaseReader>>,
        fence: bool,
    ) -> Self {
        Self {
            log,
            epochs,
            // ALLOC: one node id, once a reader.
            fenced_by: fenced_by.to_owned(),
            term,
            reread,
            fence,
            fenced: Mutex::new(None),
            // ALLOC: one header a blob holding a shard asked about, bounded by the predecessor's unpublished blobs.
            headers: Mutex::new(BTreeMap::new()),
            bodies: Mutex::new(read::Bodies::new(read::BodyCache::default())),
        }
    }

    /// The epochs this reads, as the predecessor's lease named them.
    #[must_use]
    pub fn epochs(&self) -> &[NodeLogEpoch] {
        &self.epochs
    }

    /// The node whose log this reads.
    #[must_use]
    pub fn predecessor(&self) -> &str {
        self.log.node_id()
    }

    fn lock(&self) -> MutexGuard<'_, Option<Arc<Fenced>>> {
        self.fenced.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Fences every epoch the lease names and reads each one's last
    /// header, unless that is done already.
    ///
    /// # Errors
    ///
    /// When a fence, a read or the lease's reread fails; the next call
    /// tries again from the start, sharing the fences already placed.
    pub fn fence_all(&self) -> Result<(), TakeoverError> {
        self.fenced().map(|_| ())
    }

    pub(crate) fn fenced(&self) -> Result<Arc<Fenced>, TakeoverError> {
        self.fenced_within(None)
    }

    /// As `fenced`, refusing a last header over `cap` bytes; nothing is
    /// kept then, so a reader without the cap starts over.
    pub(crate) fn fenced_within(&self, cap: Option<u64>) -> Result<Arc<Fenced>, TakeoverError> {
        let mut held = self.lock();
        if let Some(fenced) = held.as_ref() {
            let over = cap.and_then(|cap| {
                fenced
                    .last_headers
                    .values()
                    .flatten()
                    .map(|h| h.body_offset)
                    .find(|len| *len > cap)
                    .map(|bytes| (bytes, cap))
            });
            if let Some((bytes, cap)) = over {
                return Err(LogError::HeaderTooLarge { bytes, cap }.into());
            }
            return Ok(Arc::clone(fenced));
        }
        // ALLOC: one entry an epoch the lease names, a handful.
        let mut ends = BTreeMap::new();
        // ALLOC: as above.
        let mut floors = BTreeMap::new();
        for entry in &self.epochs {
            let epoch = lease_slot(entry.epoch);
            floors.insert(epoch, lease_slot(entry.trim));
            ends.insert(epoch, self.end_of(epoch, lease_slot(entry.landed))?);
        }
        if let Some(reread) = &self.reread {
            for _ in 0..MAX_REFENCES {
                let mut moved = false;
                for now in reread.epochs().map_err(TakeoverError::Lease)? {
                    let epoch = lease_slot(now.epoch);
                    let Some(end) = ends.get(&epoch).copied() else {
                        continue;
                    };
                    let trim = lease_slot(now.trim);
                    // Nothing below a trim point the lease recorded is
                    // needed, so the header search stops there.
                    floors.entry(epoch).and_modify(|f| *f = (*f).max(trim));
                    if trim > end {
                        // Deleted past the end after the lease this
                        // started from: the fence may be in a hole.
                        ends.insert(epoch, self.end_of(epoch, lease_slot(now.landed))?);
                        moved = true;
                    }
                }
                if !moved {
                    break;
                }
            }
        }
        // ALLOC: one header an epoch the lease names, a handful.
        let mut last_headers = BTreeMap::new();
        for (&epoch, &end) in &ends {
            let floor = floors.get(&epoch).copied().unwrap_or(0);
            let last = self.log.last_header_below_within(epoch, end, floor, cap)?;
            last_headers.insert(epoch, last.map(Arc::new));
        }
        let fenced = Arc::new(Fenced { ends, last_headers });
        *held = Some(Arc::clone(&fenced));
        Ok(fenced)
    }

    fn end_of(&self, epoch: u64, from: u64) -> Result<u64, LogError> {
        if self.fence {
            self.log.fence(epoch, from, &self.fenced_by, self.term)
        } else {
            self.log.first_free(epoch, from)
        }
    }

    /// Per epoch, the slot the fence took, or the first free slot for a
    /// reader that fences nothing: every slot the writer used is below it.
    ///
    /// # Errors
    ///
    /// As [`NodeLogTakeover::fence_all`].
    pub fn ends(&self) -> Result<BTreeMap<u64, u64>, TakeoverError> {
        Ok(self.fenced()?.ends.clone())
    }

    /// Every shard an epoch's last header names as holding records not
    /// yet published.
    ///
    /// # Errors
    ///
    /// As [`NodeLogTakeover::fence_all`].
    pub fn named(&self) -> Result<BTreeSet<ShardKey>, TakeoverError> {
        self.named_within(None)
    }

    /// As [`NodeLogTakeover::named`], refusing a last header longer than
    /// `cap` bytes, which is [`LogError::HeaderTooLarge`] and leaves the
    /// shards to the takes that read the header unbounded.
    ///
    /// # Errors
    ///
    /// As [`NodeLogTakeover::fence_all`], or a header over `cap`.
    pub fn named_within(&self, cap: Option<u64>) -> Result<BTreeSet<ShardKey>, TakeoverError> {
        let fenced = self.fenced_within(cap)?;
        Ok(fenced
            .last_headers
            .values()
            .flatten()
            .flat_map(|header| header.index.iter().map(|(shard, _)| shard.clone()))
            // A commit key is a logged commit, not a shard with a head.
            .filter(|shard| !crate::log_commit::is_commit_key(shard))
            // ALLOC: the shards the predecessor's headers name, as many as it held unpublished.
            .collect())
    }
}
