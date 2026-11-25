//! The shard a search serves when an overlay source has finished an overlay.

use std::sync::Arc;

use lucene_search::directory_reader::DirectoryReader;
use oss_blob::BlobStore;
use oss_reader::OpenShard;
use oss_reader::failure::{Failure, SearchError, ShardFailure};
use oss_reader::overlay::{CommitAccess, OverlaySource, Ready, local_checkpoint};

use super::{Held, SearchService};

/// A shard held and the view a search of it runs on.
pub(super) type FreshHeld<S> = (Arc<Held<S>>, Option<Arc<OpenShard<S>>>);

/// How many times a shard refused as behind is opened afresh in one search.
const REOPENS: u32 = 3;

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

    /// Installs the check that a shard may answer only while it holds every
    /// write its writer answered as refreshed. False when one is installed
    /// already.
    pub fn set_tail_fresh(&self, fresh: Arc<crate::tail_fresh::TailFresh>) -> bool {
        self.fresh.set(fresh).is_ok()
    }

    /// The view [`Self::overlay_view`] gives, once the shard is known to
    /// hold what its writer answered as refreshed.
    ///
    /// # Errors
    ///
    /// A 503 naming the shard when it may not answer: the search asks the
    /// next copy.
    pub(super) fn fresh_view(
        &self,
        held: &Arc<Held<S>>,
    ) -> Result<Option<Arc<OpenShard<S>>>, Box<SearchError>> {
        self.fresh_check(held)?;
        Ok(self.overlay_view(held))
    }

    /// As [`Self::fresh_view`], and when the shard refuses because it is
    /// behind (it was open on a commit that a publish has since passed, and
    /// is let go), again over the shard opened afresh on the newest commit:
    /// the open the next search would make, made now, so the caller is not
    /// answered a 503 for a state the node mends by itself. A publish can
    /// pass the new open too, so it is tried [`REOPENS`] times; a shard still
    /// behind then answers the refusal.
    pub(super) fn fresh_held(
        &self,
        held: Arc<Held<S>>,
        single: bool,
    ) -> Result<FreshHeld<S>, super::Answer> {
        let mut held = held;
        let mut reopened = 0;
        loop {
            match self.fresh_view(&held) {
                Ok(view) => return Ok((held, view)),
                Err(e)
                    if e.status == 503
                        && e.failure.kind == "shard_behind_exception"
                        && reopened < REOPENS =>
                {
                    reopened = reopened.saturating_add(1);
                    // ALLOC: the index name, once a reopen.
                    let (index, shard) = (held.index.clone(), held.shard_no);
                    drop(held);
                    held = self.open_shard(&index, shard, single)?;
                }
                Err(e) => return Err(super::Answer::Search(e)),
            }
        }
    }

    fn fresh_check(&self, held: &Arc<Held<S>>) -> Result<(), Box<SearchError>> {
        let Some(fresh) = self.fresh.get() else {
            return Ok(());
        };
        let Some(writer) = held.stage.writer() else {
            return Ok(());
        };
        let uuid = &held.shard.meta.uuid;
        // The engine is known to the source before the writer is asked for
        // a subscription: a shard opened after refreshed writes would
        // otherwise ask only how far they went, and never hold them.
        if let Some(source) = self.overlay.get() {
            let commit: Arc<dyn CommitAccess> = Arc::clone(held) as _;
            let _ = source.ready(uuid, held.shard_no, &held.stage, &commit, self.now_millis());
        }
        let take = || {
            if let Some(source) = self.overlay.get() {
                let commit: Arc<dyn CommitAccess> = Arc::clone(held) as _;
                source.apply_pending(uuid, held.shard_no, &held.stage, &commit);
            }
        };
        if fresh.ensure_fresh(
            &held.index,
            uuid,
            held.shard_no,
            writer,
            local_checkpoint(&held.shard.reader),
            &take,
        ) && (!held.unpublished || fresh.holds_tail(uuid, held.shard_no, writer))
        {
            // An empty commit stands for a first publish still to come: with
            // no complete tail from the writer (it did not answer, or its tail
            // has a gap because it published, so the register is lost) the
            // shard is behind, not empty.
            return Ok(());
        }
        // A shard that cannot hold its writer's tail (it ran out of room, or
        // the engine refused a record) would refuse every search until its
        // commit changed: letting it go makes the next search open it afresh,
        // on the newest commit the node knows, and ask for the tail again.
        let key = crate::search::open_shard::set_key(&held.index, held.shard_no);
        let released = self.lock().release(&key);
        drop(released);
        // A module that failed the build is why the shard is behind: say so.
        if let Some(source) = self.overlay.get()
            && let Some(refusal) = source.extension_refusal(uuid, held.shard_no, &held.stage)
        {
            let (status, ty) = refusal_answer(refusal.ty);
            // ALLOC: error path; one box and the names.
            return Err(Box::new(SearchError::on_shard(
                status,
                Failure::plain(ty, refusal.reason),
                ShardFailure {
                    index: held.index.clone(),
                    shard: held.shard_no,
                    node: self.node_id.clone(),
                },
            )));
        }
        // ALLOC: error path; one box and the names.
        Err(Box::new(SearchError::on_shard(
            503,
            Failure::plain(
                "shard_behind_exception",
                "the shard is behind the writes its writer answered as refreshed",
            ),
            ShardFailure {
                index: held.index.clone(),
                shard: held.shard_no,
                node: self.node_id.clone(),
            },
        )))
    }

    /// `held`'s shard with the overlay the source has finished on top, or
    /// `None` to serve the commit alone: no source, no finished overlay, or a
    /// view the engine refused.
    pub(super) fn overlay_view(&self, held: &Arc<Held<S>>) -> Option<Arc<OpenShard<S>>> {
        let source = self.overlay.get()?;
        // An index that turned the overlay off is served from its commit.
        if !crate::tail_overlay::index_wants_overlay(&held.shard.meta.settings) {
            return None;
        }
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
        self.shard.meta.mapping.json()
    }

    fn index_name(&self) -> &str {
        &self.index
    }

    fn modules(&self) -> Vec<(String, Arc<dyn oss_ext::analyzer::Analyze>)> {
        self.pinned.as_ref().map_or_else(Vec::new, |p| p.modules())
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

/// The status and type a search answers when a module failed the overlay
/// build: capacity is a 429, anything else a retryable 503.
fn refusal_answer(ty: &str) -> (u16, &'static str) {
    if ty == "extension_busy" {
        (429, "extension_busy")
    } else {
        (503, "extension_unavailable")
    }
}

#[cfg(test)]
mod refusal_tests {
    use super::refusal_answer;

    #[test]
    fn capacity_is_a_429_and_any_other_module_failure_a_retryable_503() {
        assert_eq!(refusal_answer("extension_busy"), (429, "extension_busy"));
        for ty in [
            "extension_failed",
            "extension_malformed",
            "extension_unavailable",
        ] {
            assert_eq!(refusal_answer(ty), (503, "extension_unavailable"), "{ty}");
        }
    }
}
