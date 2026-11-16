//! Change stripes: a few registers every descriptor update and delete
//! bumps, so a writer confirms that many indexes are unchanged by reading
//! one register rather than a descriptor each.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/ChangeStripes.java`.
//! An index's name hashes to one of [`crate::STRIPES`] stripes; a stripe that
//! names no change to an index since its descriptor was last read confirms
//! it as well as reading it again would. A delete is bumped twice, a
//! pending entry before and an applied one after, so a reader that read the
//! stripe and then a still-live descriptor between the two is never
//! confirmed by the stripe; a pending entry nothing finishes is kept.
//!
//! Each bump is a read and a compare-and-swap of one register, retried
//! while other bumps move it: no lock and no leader.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use oss_blob::{BlobStore, SwapOutcome};
use oss_format::{
    ChangeKind, ChangeStripe, MAX_CHANGE_STRIPE_BYTES, MAX_ENTRY_NAME_BYTES, change_stripe_key,
    stripe_of,
};

use crate::error::MetaError;

/// How many times a bump reads and swaps before giving up to contention.
const ATTEMPTS: u32 = 64;

/// The change stripes of one deployment.
pub struct ChangeStripes {
    store: Arc<dyn BlobStore>,
    base: String,
}

/// One read of a stripe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The stripe's epoch.
    pub epoch: i64,
    /// Its generation.
    pub generation: i64,
    /// The generation of the oldest applied change it keeps, or
    /// `generation + 1` when it keeps none.
    pub oldest_kept: i64,
    /// Every index an entry names, with the highest generation naming it.
    pub changed: HashMap<String, i64>,
    /// Every index a delete no applied change has finished names.
    pub pending: HashSet<String>,
    /// When the read began.
    pub read_started: Instant,
}

impl Snapshot {
    /// Whether this read proves `name`'s descriptor unchanged since a read
    /// of the stripe at `since_epoch` and `since`: same epoch, no pending
    /// delete of it, and every change after `since` still kept and none
    /// naming it.
    pub fn unchanged_since(&self, name: &str, since_epoch: i64, since: i64) -> bool {
        if since_epoch != self.epoch || self.pending.contains(name) || since > self.generation {
            return false;
        }
        if since == self.generation {
            return true;
        }
        if self.oldest_kept > since.saturating_add(1) {
            // Entries after the earlier read were dropped: one may have
            // named it.
            return false;
        }
        self.changed.get(name).is_none_or(|last| *last <= since)
    }
}

impl ChangeStripes {
    /// The stripes under `base` (empty, or ending in `/`) of `store`.
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`] for a base that is not empty and does not end in
    /// `/`.
    pub fn new(store: Arc<dyn BlobStore>, base: &str) -> Result<Self, MetaError> {
        change_stripe_key(base, 0)?;
        Ok(Self {
            store,
            // ALLOC: once a deployment, its base.
            base: base.to_owned(),
        })
    }

    /// The stripe `name` hashes to, as every Java and Rust node hashes it.
    pub fn stripe_of(name: &str) -> usize {
        stripe_of(name)
    }

    /// Records a change to `name`'s descriptor in its stripe, returning
    /// once it has landed: a read and a swap, retried while other bumps
    /// move the stripe.
    ///
    /// # Errors
    ///
    /// [`MetaError::InvalidArgument`] for a name longer than an index name
    /// may be or holding a newline, before any request;
    /// [`MetaError::Contended`] when the stripe moved under every attempt;
    /// [`MetaError::Format`] for a malformed stripe; [`MetaError::Blob`]
    /// when the store fails.
    pub fn bump(&self, name: &str, kind: ChangeKind) -> Result<(), MetaError> {
        if !Self::holds(name) {
            return Err(MetaError::InvalidArgument(
                "a change stripe names indexes of at most 255 bytes, on one line",
            ));
        }
        let key = change_stripe_key(&self.base, stripe_of(name))?;
        // ALLOC: one stripe read and one written, MAX_CHANGE_STRIPE_BYTES
        // each, at the rate of descriptor updates and deletes.
        let mut read = vec![0u8; MAX_CHANGE_STRIPE_BYTES];
        let mut write = vec![0u8; MAX_CHANGE_STRIPE_BYTES];
        for attempt in 0..ATTEMPTS {
            let current = self.store.read_register(&key, &mut read)?;
            let mut stripe = match current {
                Some(r) => ChangeStripe::parse(read.get(..r.len).unwrap_or_default())?,
                None => ChangeStripe::default(),
            };
            stripe.add(name, kind);
            let len = stripe.write(&mut write)?;
            let value = write.get(..len).unwrap_or_default();
            let outcome = match current {
                Some(r) => self.store.swap_register(&key, r.generation, value)?,
                None => self.store.create_register(&key, value)?,
            };
            if matches!(outcome, SwapOutcome::Applied(_)) {
                return Ok(());
            }
            // Another bump landed first: back off 1, 2, 4 ... 32 ms, as the
            // Java's.
            std::thread::sleep(Duration::from_millis(1 << attempt.min(5)));
        }
        Err(MetaError::Contended)
    }

    /// Whether a stripe can name `name`: at most an index name's 255 bytes,
    /// and on one line. No index created through the name rules is longer.
    pub(crate) fn holds(name: &str) -> bool {
        name.len() <= MAX_ENTRY_NAME_BYTES && !name.contains('\n')
    }

    /// Reads stripe `stripe`: one request.
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`] for a stripe of [`crate::STRIPES`] or more or a
    /// malformed stripe; [`MetaError::Blob`] when the store fails.
    pub fn read(&self, stripe: usize) -> Result<Snapshot, MetaError> {
        let read_started = Instant::now();
        let key = change_stripe_key(&self.base, stripe)?;
        // ALLOC: one stripe read, MAX_CHANGE_STRIPE_BYTES, a read a stripe.
        let mut buf = vec![0u8; MAX_CHANGE_STRIPE_BYTES];
        let stripe = match self.store.read_register(&key, &mut buf)? {
            Some(r) => ChangeStripe::parse(buf.get(..r.len).unwrap_or_default())?,
            None => ChangeStripe::default(),
        };
        let mut snapshot = Snapshot {
            epoch: stripe.epoch,
            generation: stripe.generation,
            oldest_kept: stripe.generation.saturating_add(1),
            // ALLOC: at most KEPT + KEPT_PENDING names, owned so a writer
            // can keep the read.
            changed: HashMap::with_capacity(stripe.entries.len()),
            pending: HashSet::new(),
            read_started,
        };
        for entry in &stripe.entries {
            if entry.pending {
                // ALLOC: one name, bounded as above.
                snapshot.pending.insert(entry.name.to_owned());
            } else {
                snapshot.oldest_kept = snapshot.oldest_kept.min(entry.generation);
            }
            // ALLOC: one name, bounded as above.
            let last = snapshot
                .changed
                .entry(entry.name.to_owned())
                .or_insert(entry.generation);
            *last = (*last).max(entry.generation);
        }
        Ok(snapshot)
    }
}
