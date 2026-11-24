//! The three point-in-time stores a node builds over one store: one for the
//! point-in-time routes, one for the janitor and one for the search service.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_meta::{PitClock, PointInTimeStore};

use crate::assemble::AssembleError;

/// Three stores over `store` under `base`, sharing `clock`.
pub(crate) fn three(
    store: &Arc<dyn BlobStore>,
    base: &str,
    clock: &PitClock,
) -> Result<[PointInTimeStore; 3], AssembleError> {
    let one = || {
        PointInTimeStore::new(Arc::clone(store), base, Arc::clone(clock))
            .map_err(|e| AssembleError::Store(e.to_string()))
    };
    Ok([one()?, one()?, one()?])
}
