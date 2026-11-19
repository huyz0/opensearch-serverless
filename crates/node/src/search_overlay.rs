//! The shard a search serves when an overlay source has finished an overlay.

use std::sync::Arc;

use lucene_search::directory_reader::DirectoryReader;
use oss_blob::BlobStore;
use oss_reader::OpenShard;
use oss_reader::overlay::{CommitAccess, OverlaySource, Ready};

use super::{Held, SearchService};

/// An overlay and the shard view made from it; `None` when the engine
/// refused the view, so the refusal is not repeated on every search.
pub(super) type OverlayView<S> = (Arc<Ready>, Option<Arc<OpenShard<S>>>);

impl<S: BlobStore + 'static> SearchService<S> {
    /// Whether a source of overlays is installed.
    pub fn has_overlay_source(&self) -> bool {
        self.overlay.get().is_some()
    }

    /// Installs where a search finds the overlay of the shard it serves.
    /// False when one is installed already.
    pub fn set_overlay_source(&self, source: Arc<dyn OverlaySource>) -> bool {
        self.overlay.set(source).is_ok()
    }

    /// `held`'s shard with the overlay the source has finished on top, or
    /// `None` to serve the commit alone: no source, no finished overlay, or a
    /// view the engine refused.
    pub(super) fn overlay_view(&self, held: &Arc<Held<S>>) -> Option<Arc<OpenShard<S>>> {
        let source = self.overlay.get()?;
        let commit: Arc<dyn CommitAccess> = Arc::clone(held) as _;
        let ready = source.ready(
            &held.shard.meta.uuid,
            held.shard_no,
            &held.stage,
            &commit,
            self.now_millis(),
        )?;
        let mut cached = held
            .view
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((made_from, view)) = cached.as_ref()
            && Arc::ptr_eq(made_from, &ready)
        {
            return view.clone();
        }
        // The build reads nothing from the store, so holding the lock
        // through it costs the searches of one shard a short wait, once an
        // overlay.
        let view = ready.view(&held.shard).ok().map(Arc::new);
        *cached = Some((ready, view.clone()));
        view
    }
}

impl<S: BlobStore + 'static> CommitAccess for Held<S> {
    fn reader(&self) -> &DirectoryReader {
        &self.shard.reader
    }

    fn mapping_json(&self) -> &str {
        &self.shard.meta.mapping_json
    }

    fn index_name(&self) -> &str {
        &self.index
    }
}

impl<S: BlobStore + 'static> Drop for Held<S> {
    fn drop(&mut self) {
        if let Some(source) = &self.source {
            let now = u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX);
            source.closed(&self.shard.meta.uuid, self.shard_no, &self.stage, now);
        }
    }
}
