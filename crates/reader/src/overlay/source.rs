//! What a search asks for the overlay of a shard it serves.

use std::sync::Arc;

use lucene_search::directory_reader::DirectoryReader;

use super::{BuildError, Copies, Overlay};
use crate::OpenShard;
use crate::commit_push::Stage;

/// A finished overlay with the commit's copies of the ids it replaces: all
/// a search needs to see the shard with the overlay on top.
#[derive(Debug)]
pub struct Ready {
    overlay: Overlay,
    copies: Copies,
}

impl Ready {
    /// `overlay` with `copies`, found in the commit it was built over.
    #[must_use]
    pub fn new(overlay: Overlay, copies: Copies) -> Self {
        Self { overlay, copies }
    }

    /// The overlay.
    #[must_use]
    pub fn overlay(&self) -> &Overlay {
        &self.overlay
    }

    /// The shard as a search sees it with this overlay on top.
    ///
    /// # Errors
    ///
    /// As [`Overlay::view_of`].
    pub fn view<S: oss_blob::BlobStore + 'static>(
        &self,
        shard: &OpenShard<S>,
    ) -> Result<OpenShard<S>, BuildError> {
        shard.with_overlay(&self.overlay, &self.copies)
    }
}

/// Where a node's searches get the overlay of a shard. A search never
/// starts a build: the source answers with what is finished, or nothing.
pub trait OverlaySource: Send + Sync {
    /// The finished overlay for shard `shard` of the index `index_uuid`,
    /// built over the commit `commit` (at `stage`), as of `now_millis`.
    fn ready(
        &self,
        index_uuid: &str,
        shard: u32,
        stage: &Stage,
        commit: &DirectoryReader,
        now_millis: u64,
    ) -> Option<Arc<Ready>>;
}
