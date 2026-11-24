//! What a booted node does with each commit its writer publishes: its own
//! reader takes it at once, and the readers of the shard hear of it from a
//! worker thread, so no publish waits on a send.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use oss_blob::BlobStore;
use oss_types::settings::{BoolSetting, Scope, Setting};

use crate::push_sender::{Published, PushLink, PushSender, PushTargets};
use crate::search::SearchService;
use crate::writer::CommitSink;

/// `serverless.search.commit_push.enabled`: a writer tells the readers of a
/// shard of each commit it publishes. Default `true` while the tail overlay is
/// off (a node's own reader always takes its own writer's commit); off when the
/// overlay is on, where it was measured to make readers reopen cold.
pub const ENABLED: BoolSetting = BoolSetting::new(
    "serverless.search.commit_push.enabled",
    true,
    Scope::Node,
    "Tell the nodes that search a shard of each commit its writer publishes, so an open \
     reader moves to it at once instead of keeping the commit it opened on.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[ENABLED.setting()];

/// How many commits wait for the worker before one is dropped.
pub const QUEUE: usize = 256;

/// The reader of this node, which takes a commit its own writer published.
pub trait LocalTake: Send + Sync {
    /// Takes the commit of shard `shard` of `index` (with `uuid`); whether
    /// the shard is open here.
    fn take(&self, index: &str, uuid: &str, shard: u32, commit: &[u8]) -> bool;
}

impl<S: BlobStore + 'static> LocalTake for SearchService<S> {
    fn take(&self, index: &str, uuid: &str, shard: u32, commit: &[u8]) -> bool {
        self.take_own(index, uuid, shard, commit)
    }
}

/// One commit to send.
struct Job {
    index: String,
    uuid: String,
    shard: u32,
    commit: Vec<u8>,
}

/// A node's commit sink: the local take, then the queue to the worker.
pub struct CommitPush {
    local: Arc<dyn LocalTake>,
    /// Whether every commit is sent to the other readers; else only the
    /// first one of a takeover is.
    remote: bool,
    queue: Mutex<Option<SyncSender<Job>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    dropped: AtomicU64,
}

impl CommitPush {
    /// A sink whose worker sends through `sender`, and whose own reader is
    /// `local`.
    ///
    /// # Errors
    ///
    /// The worker thread cannot be started.
    pub fn start<T, L>(
        sender: PushSender<T, L>,
        local: Arc<dyn LocalTake>,
        remote: bool,
    ) -> std::io::Result<Arc<Self>>
    where
        T: PushTargets + Send + 'static,
        L: PushLink + Send + 'static,
    {
        let (tx, rx) = sync_channel::<Job>(QUEUE);
        let worker = std::thread::Builder::new()
            .name("commit-push".to_owned())
            .spawn(move || Self::work(&sender, &rx))?;
        Ok(Arc::new(Self {
            local,
            remote,
            queue: Mutex::new(Some(tx)),
            worker: Mutex::new(Some(worker)),
            dropped: AtomicU64::new(0),
        }))
    }

    fn work<T: PushTargets, L: PushLink>(sender: &PushSender<T, L>, rx: &Receiver<Job>) {
        while let Ok(job) = rx.recv() {
            sender.published(Published {
                index: &job.index,
                uuid: &job.uuid,
                shard: job.shard,
                commit: &job.commit,
                mapping_version: -1,
                settings_version: -1,
            });
        }
    }

    /// How many commits were dropped because the queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Stops the worker after what is queued, and waits for it.
    pub fn stop(&self) {
        self.queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }
}

impl CommitSink for CommitPush {
    fn published(&self, index: &str, uuid: &str, shard: u32, manifest: &[u8], taken_over: bool) {
        // Nothing pushes to oneself: the node that owns a shard serves
        // searches of it too.
        self.local.take(index, uuid, shard, manifest);
        if !self.remote && !taken_over {
            return;
        }
        let queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(tx) = queue.as_ref() else {
            return;
        };
        let job = Job {
            // ALLOC: one name and one uuid a publish, bounded by the queue.
            index: index.to_owned(),
            uuid: uuid.to_owned(),
            shard,
            // ALLOC: one commit a publish, at most MAX_NEWEST_BYTES.
            commit: manifest.to_vec(),
        };
        if let Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) = tx.try_send(job) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests;
