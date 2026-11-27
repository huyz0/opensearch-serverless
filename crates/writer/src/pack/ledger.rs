//! Which of a node's packs may still exist, so collection can name every one
//! without listing.
//!
//! Ported from `NodePackLedger` (huyz0/OpenSearch `feature/serverlessshell`).
//! One register per node at `packs/<node>/ledger`; a writer records its epoch
//! in it before its first pack, so no pack is written in an epoch collection
//! does not know about. Written when a writer starts and when a collection
//! pass moves something on, never per pack.

use std::sync::Arc;

use oss_blob::register::ABSENT_GENERATION;
use oss_blob::{BlobError, BlobStore, SwapOutcome};
use oss_format::{NodePackLedger, PackEpoch, encode_pack_ledger};

use super::PackError;

/// How many times an update re-reads after losing a swap.
const MAX_ATTEMPTS: usize = 16;

/// The most live epochs a node keeps; a writer starts no epoch past it.
pub(super) const MAX_LIVE_EPOCHS: usize = 1_500;

/// The most a ledger read keeps: some 10,000 epochs.
const READ_BYTES: usize = 256 << 10;

/// The ledger as read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerState {
    /// The register's generation, to swap against.
    pub generation: u64,
    /// Every epoch, oldest first.
    pub epochs: Vec<PackEpoch>,
}

/// One node's ledger register.
pub struct PackLedger {
    store: Arc<dyn BlobStore>,
    key: String,
}

impl std::fmt::Debug for PackLedger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackLedger")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl PackLedger {
    /// The ledger of `node` under `base` (empty, or ending in `/`).
    ///
    /// # Errors
    ///
    /// [`PackError::Format`] when the node id cannot be part of a key.
    pub fn new(store: Arc<dyn BlobStore>, base: &str, node: &str) -> Result<Self, PackError> {
        let key = oss_format::node_pack_key(base, node, 0, 0)?;
        // The key of any pack of the node is `packs/<node>/e=.../...`; the
        // ledger is `packs/<node>/ledger`.
        let dir = key
            .rsplit_once("/e=")
            .map_or(key.as_str(), |(dir, _)| dir)
            // ALLOC: the epochs of one ledger, 24 bytes each of a register.
            .to_owned();
        Ok(Self {
            store,
            // ALLOC: one key, once a ledger.
            key: format!("{dir}/ledger"),
        })
    }

    /// Reads the ledger: no epochs, at [`ABSENT_GENERATION`], if it was never
    /// written.
    ///
    /// # Errors
    ///
    /// The store's failures; [`PackError::Format`] for a register that is
    /// not a ledger or is larger than a ledger read keeps.
    pub fn read(&self) -> Result<LedgerState, PackError> {
        // ALLOC: one buffer a ledger read, at most READ_BYTES.
        let mut buf = vec![0u8; READ_BYTES];
        match self.store.read_register(&self.key, &mut buf) {
            Ok(Some(read)) => {
                let value = buf.get(..read.len).unwrap_or_default();
                Ok(LedgerState {
                    generation: read.generation,
                    // ALLOC: the epochs of one ledger, 24 bytes each of a register.
                    epochs: NodePackLedger::parse(value)?.epochs().to_vec(),
                })
            }
            Ok(None) => Ok(LedgerState {
                generation: ABSENT_GENERATION,
                // ALLOC: the epochs of one ledger, 24 bytes each of a register.
                epochs: Vec::new(),
            }),
            Err(BlobError::InvalidArgument(_)) => {
                Err(PackError::Format(oss_format::FormatError::TooLarge {
                    what: "node pack ledger",
                    limit: READ_BYTES as u64,
                    actual: 0,
                }))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// The epoch a writer starting now should use: `clock_millis`, or one
    /// past the highest epoch the ledger holds if that is later, so a clock
    /// that stepped back across a restart cannot reuse a name.
    ///
    /// # Errors
    ///
    /// As [`PackLedger::read`].
    pub fn next_epoch(&self, clock_millis: u64) -> Result<u64, PackError> {
        let highest = self.read()?.epochs.iter().map(|e| e.epoch).max();
        Ok(highest.map_or(clock_millis, |h| clock_millis.max(h.saturating_add(1))))
    }

    /// Records an epoch a writer is about to write packs in. Must return
    /// before the writer writes its first pack.
    ///
    /// # Errors
    ///
    /// The store's failures; [`PackError::LedgerBusy`] when the ledger moved
    /// under every attempt.
    pub fn add_epoch(&self, epoch: u64) -> Result<LedgerState, PackError> {
        self.update(self.read()?, |epochs| {
            if epochs.iter().any(|e| e.epoch == epoch) {
                // ALLOC: the epochs of one ledger, 24 bytes each of a register.
                return epochs.to_vec();
            }
            // ALLOC: the ledger's epochs, 24 bytes each of the register.
            let mut next = epochs.to_vec();
            next.push(PackEpoch {
                epoch,
                collected_below: 0,
                high: 0,
            });
            next
        })
    }

    /// Reserves ordinals of `epoch` below `high`, adding the epoch if the
    /// ledger lacks it, so `high` is never below an ordinal in use. Starting
    /// an epoch past the 1,500 live epochs a node keeps is refused.
    ///
    /// # Errors
    ///
    /// As [`PackLedger::add_epoch`], and [`PackError::TooManyEpochs`].
    pub fn reserve(&self, epoch: u64, high: u64) -> Result<LedgerState, PackError> {
        let state = self.read()?;
        if state.epochs.len() >= MAX_LIVE_EPOCHS && !state.epochs.iter().any(|e| e.epoch == epoch) {
            return Err(PackError::TooManyEpochs);
        }
        self.update(state, |epochs| {
            // ALLOC: the ledger's epochs, 24 bytes each of the register.
            let mut next = epochs.to_vec();
            match next.iter_mut().find(|e| e.epoch == epoch) {
                Some(e) => e.high = e.high.max(high),
                None => next.push(PackEpoch {
                    epoch,
                    collected_below: 0,
                    high,
                }),
            }
            next
        })
    }

    /// Applies `change` to the ledger by compare-and-swap, re-reading and
    /// re-applying on a conflict. Writes nothing when the change leaves the
    /// epochs as they are.
    ///
    /// # Errors
    ///
    /// As [`PackLedger::add_epoch`].
    pub fn update(
        &self,
        from: LedgerState,
        change: impl Fn(&[PackEpoch]) -> Vec<PackEpoch>,
    ) -> Result<LedgerState, PackError> {
        let mut current = from;
        for _ in 0..MAX_ATTEMPTS {
            let next = change(&current.epochs);
            if next == current.epochs {
                return Ok(current);
            }
            let bytes = encode_pack_ledger(&next)?;
            let outcome = if current.generation == ABSENT_GENERATION {
                self.store.create_register(&self.key, &bytes)?
            } else {
                self.store
                    .swap_register(&self.key, current.generation, &bytes)?
            };
            if let SwapOutcome::Applied(generation) = outcome {
                return Ok(LedgerState {
                    generation,
                    epochs: next,
                });
            }
            current = self.read()?;
        }
        Err(PackError::LedgerBusy)
    }
}
