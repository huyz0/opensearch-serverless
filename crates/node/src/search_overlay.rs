//! The shard a search serves when an overlay source has finished an overlay.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_reader::OpenShard;
use oss_reader::overlay::Ready;

use super::{Held, SearchService};

/// An overlay and the shard view made from it.
pub(super) type OverlayView<S> = (Arc<Ready>, Arc<OpenShard<S>>);

impl<S: BlobStore + 'static> SearchService<S> {
    /// `held`'s shard with the overlay the source has finished on top, or
    /// `None` to serve the commit alone: no source, no finished overlay, or a
    /// view the engine refused.
    pub(super) fn overlay_view(
        &self,
        held: &Held<S>,
        meta_uuid: &str,
    ) -> Option<Arc<OpenShard<S>>> {
        let source = self.overlay.get()?;
        let ready = source.ready(
            meta_uuid,
            0,
            &held.stage,
            &held.shard.reader,
            self.now_millis(),
        )?;
        let mut cached = held
            .view
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((made_from, view)) = cached.as_ref()
            && Arc::ptr_eq(made_from, &ready)
        {
            return Some(Arc::clone(view));
        }
        let view = Arc::new(ready.view(&held.shard).ok()?);
        *cached = Some((ready, Arc::clone(&view)));
        Some(view)
    }
}
