//! `DELETE /{index}` and the reclaim drain, over `oss-meta`'s lifecycle with
//! the real shard heads and pins (ADR 0012).
//!
//! Ported from `IndexAdminHandler`'s delete route. An index of few shards no
//! writer held is purged inside the request; a larger one, or a shard a writer held, waits for its
//! reclaim intent, and so does a shard a point in time pinned, which [`DeleteService::drain`] takes once due.

mod heads;
mod pins;
mod timer;

use std::sync::Arc;
use std::time::Duration;

use oss_blob::BlobStore;
use oss_coordinator::indices::admin::{check_name, delete_ack, delete_failed, no_such_index};
use oss_coordinator::route::resolve::Refusal;
use oss_meta::{
    AliasClock, AliasMutations, DescriptorStore, IndexLifecycle, MetaError, PitClock,
    PointInTimeStore, ReclaimQueue,
};
use oss_rest::{Exception, JSON, Method, Request, Response, RestError, RouteError, Router};

pub use heads::NodeHeads;
pub use pins::{NoSnapshots, NodePins, SnapshotPins, StoreSnapshotPins};
pub use timer::DrainTimer;

/// The lifetime of an alias reservation a delete's detach takes.
const ALIAS_TTL_MILLIS: i64 = 30_000;

/// Buffers a delete holds: the lifecycle's, its alias mutations' three, the
/// pins' one and the snapshot pins' one.
const BUFFERS: usize = 6;

/// The query parameters a delete accepts and has nothing to do for.
const PARAMS: &[&str] = &["timeout", "master_timeout", "cluster_manager_timeout"];

fn refuse(status: u16, kind: &'static str, reason: impl Into<String>) -> RestError {
    RestError::exception(status, Exception::new(kind, reason))
}

fn refusal(r: Refusal) -> RestError {
    refuse(r.status, r.kind, r.reason)
}

/// Deletes indexes and finishes the deletes whose intents fell due.
pub struct DeleteService {
    descriptors: DescriptorStore,
    queue: ReclaimQueue,
    heads: NodeHeads,
    pins: NodePins,
    clock: PitClock,
    buffer_bytes: usize,
    inline_purge_max_shards: u32,
}

impl DeleteService {
    /// A service over `store` under `base`. Each request holds
    /// six buffers of `buffer_bytes`, charged to the request-body pool.
    ///
    /// # Errors
    ///
    /// [`MetaError`] when the store refuses the base.
    pub fn new(
        store: &Arc<dyn BlobStore>,
        base: &str,
        mode: oss_meta::DeleteMode,
        clock: PitClock,
        snapshots: Arc<dyn SnapshotPins>,
        buffer_bytes: usize,
        inline_purge_max_shards: u32,
    ) -> Result<Self, MetaError> {
        let pits = PointInTimeStore::new(Arc::clone(store), base, Arc::clone(&clock))?;
        Ok(Self {
            // A delete is recorded in the change stripes, so a writer that
            // kept the index stops serving it within a window.
            descriptors: DescriptorStore::new(Arc::clone(store), base, mode)?
                .record_changes_in(oss_meta::ChangeStripes::new(Arc::clone(store), base)?),
            queue: ReclaimQueue::new(Arc::clone(store), base)?,
            heads: NodeHeads::new(Arc::clone(store), base)?,
            pins: NodePins::new(pits, snapshots, buffer_bytes),
            clock,
            buffer_bytes,
            inline_purge_max_shards,
        })
    }

    fn now(&self) -> i64 {
        i64::try_from((self.clock)()).unwrap_or(i64::MAX)
    }

    fn lifecycle<'a>(
        &'a self,
        now: &'a dyn Fn() -> i64,
        pause: &'a dyn Fn(Duration),
    ) -> IndexLifecycle<'a> {
        let aliases = AliasMutations::new(
            &self.descriptors,
            AliasClock {
                now_millis: now,
                pause,
            },
            ALIAS_TTL_MILLIS,
            self.buffer_bytes,
        );
        IndexLifecycle::new(
            &self.descriptors,
            aliases,
            &self.queue,
            &self.heads,
            &self.pins,
            self.buffer_bytes,
        )
        .with_inline_purge_limit(self.inline_purge_max_shards)
        .with_repurge_pinned(true)
    }

    /// `DELETE /{index}`.
    pub fn delete(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let index = req
            .path_param("index")
            .ok_or_else(|| RestError::illegal_argument("an index is required"))?;
        check_name(&index, false).map_err(refusal)?;
        if index.contains(['*', ',']) || index == "_all" {
            return Err(refuse(
                400,
                "illegal_argument_exception",
                "deleting more than one index in a request is not supported: name one index",
            ));
        }
        req.body()
            .reserve(self.buffer_bytes.saturating_mul(BUFFERS))?;
        let now = || self.now();
        let pause = |d: Duration| std::thread::sleep(d);
        let deleted = self
            .lifecycle(&now, &pause)
            .delete_index(&index, None, self.now())
            .map_err(|e| refuse(500, "index_delete_failure", e.to_string()))?;
        match deleted {
            Some(_) => resp.send(200, JSON, delete_ack(&index).as_bytes()),
            // The pass found no index, or a change to it won the race.
            None => Err(refusal(if self.exists(&index) {
                delete_failed(&index, true)
            } else {
                no_such_index(&index)
            })),
        }
    }

    /// What [`DeleteService::remove`] holds while it runs, for a caller to
    /// reserve before it may need one.
    #[must_use]
    pub fn remove_bytes(&self) -> usize {
        self.buffer_bytes.saturating_mul(BUFFERS)
    }

    /// Deletes `index` only while it is still the incarnation `uuid`: what a
    /// restore that failed undoes of what it created, without touching an
    /// index another request created under the name since. The caller
    /// reserves [`DeleteService::remove_bytes`].
    ///
    /// # Errors
    ///
    /// [`MetaError`] from any step of the delete.
    pub fn remove(&self, index: &str, uuid: &str) -> Result<bool, MetaError> {
        let now = || self.now();
        let pause = |d: Duration| std::thread::sleep(d);
        Ok(self
            .lifecycle(&now, &pause)
            .delete_index(index, Some(uuid), self.now())?
            .is_some())
    }

    fn exists(&self, index: &str) -> bool {
        // ALLOC: one buffer, only on a delete that found nothing to delete.
        let mut buf = vec![0u8; self.buffer_bytes];
        self.descriptors
            .get(index, &mut buf)
            .is_ok_and(|r| r.is_some())
    }

    /// Takes the reclaim intents that are due and finishes their deletes,
    /// answering how many it took. One LIST when nothing is due.
    ///
    /// # Errors
    ///
    /// [`MetaError`] when the queue cannot be listed.
    pub fn drain(&self) -> Result<usize, MetaError> {
        let now = || self.now();
        let pause = |d: Duration| std::thread::sleep(d);
        let mut lifecycle = self.lifecycle(&now, &pause);
        let at = self.now();
        self.queue.drain(at, &|_| true, &mut |intent| {
            lifecycle.reclaim_at(intent, at)
        })
    }
}

/// The delete route added to `router`.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: Arc<DeleteService>) -> Result<Router, RouteError> {
    router.route(
        Method::Delete,
        "/{index}",
        PARAMS,
        move |req: &mut Request<'_>, resp: &mut Response<'_>| service.delete(req, resp),
    )
}
