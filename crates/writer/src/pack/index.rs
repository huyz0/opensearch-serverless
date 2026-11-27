//! The pack index registers: the nodes that may hold live packs, so a
//! collector reads the ledgers of only those ([`oss_format::PackIndex`]).
//!
//! A writer adds its node before its first pack of an epoch and the add
//! always writes, so the register's generation moves even when the node is
//! already listed; the collector removes a node by compare-and-swap against
//! the generation it read before it read that node's sidecar and ledger, so
//! an add made in between makes the removal lose. Both retry at most
//! [`MAX_ATTEMPTS`] times with a short jitter.

use std::sync::Arc;
use std::time::Duration;

use oss_blob::register::ABSENT_GENERATION;
use oss_blob::{BlobError, BlobStore, SwapOutcome};
use oss_format::{PackIndex, pack_index_key, pack_index_slot};

use super::PackError;

/// How many times an add re-reads after losing a swap.
const MAX_ATTEMPTS: usize = 16;

/// The most a register read keeps.
const READ_BYTES: usize = 256 << 10;

/// One index register as read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexState {
    /// The register's generation, to swap against.
    pub generation: u64,
    /// The nodes it names.
    pub index: PackIndex,
}

/// What a removal did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Removal {
    /// The node was dropped.
    Removed,
    /// The node was not named; nothing was written.
    Absent,
    /// The register moved since it was read; try again at the next pass.
    Lost,
}

/// The 16 index registers of a deployment.
pub struct PackIndexStore {
    store: Arc<dyn BlobStore>,
    base: String,
}

impl std::fmt::Debug for PackIndexStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackIndexStore")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl PackIndexStore {
    /// The index under `base` (empty, or ending in `/`).
    pub fn new(store: Arc<dyn BlobStore>, base: &str) -> Self {
        Self {
            store,
            // ALLOC: one base, once a store.
            base: base.to_owned(),
        }
    }

    /// Reads register `slot`: empty, at [`ABSENT_GENERATION`], if never
    /// written.
    ///
    /// # Errors
    ///
    /// The store's failures; [`PackError::Format`] for a register that is not
    /// an index or is past the cap.
    pub fn read(&self, slot: u8) -> Result<IndexState, PackError> {
        let key = pack_index_key(&self.base, slot)?;
        // ALLOC: one buffer a register read, at most READ_BYTES.
        let mut buf = vec![0u8; READ_BYTES];
        match self.store.read_register(&key, &mut buf) {
            Ok(Some(read)) => Ok(IndexState {
                generation: read.generation,
                index: PackIndex::parse(buf.get(..read.len).unwrap_or_default())?,
            }),
            Ok(None) => Ok(IndexState {
                generation: ABSENT_GENERATION,
                index: PackIndex::new(),
            }),
            Err(BlobError::InvalidArgument(_)) => {
                Err(PackError::Format(oss_format::FormatError::TooLarge {
                    what: "node pack index",
                    limit: READ_BYTES as u64,
                    actual: 0,
                }))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Names `node`, with a stamp of at least `stamp`, before the writer's
    /// first pack. Always writes: an already listed node's entry is
    /// rewritten, so the generation moves.
    ///
    /// # Errors
    ///
    /// The store's failures; [`PackError::IndexBusy`] when the register moved
    /// under every attempt.
    pub fn add(&self, node: &str, stamp: u64) -> Result<(), PackError> {
        let slot = pack_index_slot(node);
        let key = pack_index_key(&self.base, slot)?;
        for attempt in 0..MAX_ATTEMPTS {
            let mut state = self.read(slot)?;
            state.index.upsert(node, stamp)?;
            let bytes = state.index.encode()?;
            let outcome = if state.generation == ABSENT_GENERATION {
                self.store.create_register(&key, &bytes)?
            } else {
                self.store.swap_register(&key, state.generation, &bytes)?
            };
            if matches!(outcome, SwapOutcome::Applied(_)) {
                return Ok(());
            }
            std::thread::sleep(jitter(node, attempt));
        }
        Err(PackError::IndexBusy)
    }

    /// Drops `node` from register `slot` if the register is still at the
    /// generation of `read`, which the caller took before reading the node's
    /// sidecar and ledger. One attempt: a lost swap is [`Removal::Lost`], to
    /// be retried at the next pass.
    ///
    /// # Errors
    ///
    /// The store's failures.
    pub fn remove(&self, slot: u8, read: &IndexState, node: &str) -> Result<Removal, PackError> {
        let mut next = read.index.clone();
        if !next.remove(node) {
            return Ok(Removal::Absent);
        }
        let key = pack_index_key(&self.base, slot)?;
        let bytes = next.encode()?;
        match self.store.swap_register(&key, read.generation, &bytes)? {
            SwapOutcome::Applied(_) => Ok(Removal::Removed),
            _ => Ok(Removal::Lost),
        }
    }
}

/// A pause of 0 to 4 ms that differs by node and attempt, so writers that
/// lost together do not retry together.
fn jitter(node: &str, attempt: usize) -> Duration {
    let spread =
        u64::from(pack_index_slot(node)).saturating_add((attempt as u64).saturating_mul(3));
    Duration::from_millis(spread.checked_rem(5).unwrap_or(0))
}
