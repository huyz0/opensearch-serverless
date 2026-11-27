//! The collector's sidecar register at `packs/<node>/collected`: the
//! ordinals of each live epoch already collected (see
//! [`oss_format::PackSidecar`]). Read once after a restart so only the live
//! packs are re-read; written by compare-and-swap.

use std::sync::Arc;

use oss_blob::register::ABSENT_GENERATION;
use oss_blob::{BlobError, BlobStore, SwapOutcome};
use oss_format::PackSidecar;

use super::PackError;

/// How many times an update re-reads after losing a swap.
const MAX_ATTEMPTS: usize = 16;

/// The most a sidecar read keeps: the register cap.
const READ_BYTES: usize = 256 << 10;

/// The sidecar as read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidecarState {
    /// The register's generation, to swap against.
    pub generation: u64,
    /// The live epochs and their collected ordinals.
    pub sidecar: PackSidecar,
}

/// One node's sidecar register.
pub struct PackSidecarStore {
    store: Arc<dyn BlobStore>,
    key: String,
}

impl std::fmt::Debug for PackSidecarStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackSidecarStore")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl PackSidecarStore {
    /// The sidecar of `node` under `base` (empty, or ending in `/`).
    ///
    /// # Errors
    ///
    /// [`PackError::Format`] when the node id cannot be part of a key.
    pub fn new(store: Arc<dyn BlobStore>, base: &str, node: &str) -> Result<Self, PackError> {
        let key = oss_format::node_pack_key(base, node, 0, 0)?;
        let dir = key.rsplit_once("/e=").map_or(key.as_str(), |(dir, _)| dir);
        Ok(Self {
            store,
            // ALLOC: one key, once a store.
            key: format!("{dir}/collected"),
        })
    }

    /// Reads the sidecar: empty, at [`ABSENT_GENERATION`], if never written.
    ///
    /// # Errors
    ///
    /// The store's failures; [`PackError::Format`] for a register that is not
    /// a sidecar or is larger than the cap.
    pub fn read(&self) -> Result<SidecarState, PackError> {
        // ALLOC: one buffer a sidecar read, at most READ_BYTES.
        let mut buf = vec![0u8; READ_BYTES];
        match self.store.read_register(&self.key, &mut buf) {
            Ok(Some(read)) => {
                let value = buf.get(..read.len).unwrap_or_default();
                Ok(SidecarState {
                    generation: read.generation,
                    sidecar: PackSidecar::parse(value)?,
                })
            }
            Ok(None) => Ok(SidecarState {
                generation: ABSENT_GENERATION,
                // ALLOC: an empty list, no allocation until an epoch is added.
                sidecar: PackSidecar::new(Vec::new()),
            }),
            Err(BlobError::InvalidArgument(_)) => {
                Err(PackError::Format(oss_format::FormatError::TooLarge {
                    what: "node pack sidecar",
                    limit: READ_BYTES as u64,
                    actual: 0,
                }))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Applies `change` by compare-and-swap, re-reading and re-applying on a
    /// conflict. Writes nothing when the change leaves the sidecar as it is.
    ///
    /// # Errors
    ///
    /// The store's failures; [`PackError::LedgerBusy`] when the register
    /// moved under every attempt.
    pub fn update(
        &self,
        from: SidecarState,
        change: impl Fn(&PackSidecar) -> PackSidecar,
    ) -> Result<SidecarState, PackError> {
        let mut current = from;
        for _ in 0..MAX_ATTEMPTS {
            let next = change(&current.sidecar);
            if next == current.sidecar {
                return Ok(current);
            }
            let bytes = next.encode()?;
            let outcome = if current.generation == ABSENT_GENERATION {
                self.store.create_register(&self.key, &bytes)?
            } else {
                self.store
                    .swap_register(&self.key, current.generation, &bytes)?
            };
            if let SwapOutcome::Applied(generation) = outcome {
                return Ok(SidecarState {
                    generation,
                    sidecar: next,
                });
            }
            current = self.read()?;
        }
        Err(PackError::LedgerBusy)
    }
}
