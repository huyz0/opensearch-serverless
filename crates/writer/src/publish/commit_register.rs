//! Where a shard's published commit is recorded.
//!
//! Ported from `CommitRegister` (huyz0/OpenSearch
//! `feature/serverlessshell`): the shard's own manifest register, or a bucket
//! member's entry in its bucket's manifest. A publisher reads and swaps its
//! commit only through this, so a bucket member and a shard of its own
//! publish alike. The generation is whatever tells this shard's commit apart
//! from the one before: the register's generation for a shard of its own, the
//! member entry's version for a bucket member, so a sibling's swap never looks
//! like this shard's commit moving.

use std::sync::Arc;

use oss_blob::{BlobError, BlobStore, SwapOutcome};

use super::PublishError;

/// What a register read first has room for: well over a thousand files.
const MANIFEST_READ_FIRST: usize = 256 << 10;

/// The most a manifest read keeps: 16,384 files at well under 200 bytes.
const MANIFEST_READ_BYTES: usize = 4 << 20;

/// A commit as a register holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterValue {
    /// The generation: [`oss_blob::register::ABSENT_GENERATION`] is none.
    pub generation: u64,
    /// The commit manifest's bytes.
    pub bytes: Vec<u8>,
}

/// Where a shard's commit is recorded.
pub trait CommitRegister: Send + Sync {
    /// The commit, or `None` when none is published.
    ///
    /// # Errors
    ///
    /// The store's failures, or a commit too large to read.
    fn read(&self) -> Result<Option<RegisterValue>, PublishError>;

    /// Replaces the commit, only if it is still at `expected_generation`
    /// ([`oss_blob::register::ABSENT_GENERATION`] for none).
    ///
    /// # Errors
    ///
    /// The store's failures.
    fn swap(&self, expected_generation: u64, value: &[u8]) -> Result<SwapOutcome, PublishError>;
}

/// The register a shard of its own keeps under its data path.
pub(super) struct OwnRegister {
    store: Arc<dyn BlobStore>,
    key: String,
}

impl OwnRegister {
    pub(super) fn new(store: Arc<dyn BlobStore>, key: String) -> Self {
        Self { store, key }
    }
}

impl CommitRegister for OwnRegister {
    fn read(&self) -> Result<Option<RegisterValue>, PublishError> {
        for size in [MANIFEST_READ_FIRST, MANIFEST_READ_BYTES] {
            // ALLOC: one buffer a register read, at most manifest-sized: a publisher's first, or after a lost swap.
            let mut buf = vec![0u8; size];
            match self.store.read_register(&self.key, &mut buf) {
                Ok(Some(read)) => {
                    buf.truncate(read.len);
                    return Ok(Some(RegisterValue {
                        generation: read.generation,
                        bytes: buf,
                    }));
                }
                Ok(None) => return Ok(None),
                // A larger manifest than this buffer: once more with room for the most.
                Err(BlobError::InvalidArgument(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Err(PublishError::Unusable("a manifest too large to read"))
    }

    fn swap(&self, expected_generation: u64, value: &[u8]) -> Result<SwapOutcome, PublishError> {
        Ok(self
            .store
            .swap_register(&self.key, expected_generation, value)?)
    }
}
