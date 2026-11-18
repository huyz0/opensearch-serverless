//! What a writer sends the readers subscribed to a shard: the writes it
//! acknowledged, batched, with each answer counted so the writer knows how
//! far each reader has them.
//!
//! Ported from `acked`, `flush`, `send` and `confirm` of the shell's
//! `TailPush`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/transport/TailPush.java`).
//! The caller flushes after the linger and chooses the thread; waiting for
//! readers to confirm is a separate piece.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use oss_format::WalRecord;
use oss_transport::actions::tail::{PushRequest, PushResponse, Record, ShardRef};

use crate::push_sender::LinkError;
use crate::tail_keep::TailKeep;
use crate::tail_serve::{Sent, ShardFacts, TailServe, owned};

/// Records queued for one shard before its subscribers are let go: a reader
/// with a gap fetches again when it next opens the shard.
pub const MAX_QUEUED: usize = 20_000;

/// How a push reaches one reader.
pub trait TailLink: Send + Sync {
    /// Sends `request` to `target` and gives its answer.
    ///
    /// # Errors
    ///
    /// [`LinkError`] when the push was not answered.
    fn send(&self, target: &str, request: &PushRequest) -> Result<PushResponse, LinkError>;
}

struct Queued {
    index: String,
    uuid: String,
    shard: u32,
    records: Vec<Record>,
    end: u64,
}

/// Pushes acknowledged writes to a shard's subscribers.
pub struct TailPush {
    keep: Arc<TailKeep>,
    serve: Arc<TailServe>,
    link: Arc<dyn TailLink>,
    facts: Arc<dyn ShardFacts>,
    queue: Mutex<Vec<Queued>>,
    /// Held while a flush drains, numbers and sends, so each node's pushes of
    /// a shard are numbered and sent in queue order.
    flushing: Mutex<()>,
}

impl TailPush {
    /// A sender over `serve`.
    pub fn new(
        keep: Arc<TailKeep>,
        serve: Arc<TailServe>,
        link: Arc<dyn TailLink>,
        facts: Arc<dyn ShardFacts>,
    ) -> Self {
        Self {
            keep,
            serve,
            link,
            facts,
            queue: Mutex::new(Vec::new()),
            flushing: Mutex::new(()),
        }
    }

    fn queue(&self) -> MutexGuard<'_, Vec<Queued>> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes writes just acknowledged: keeps them until published and queues
    /// them for the shard's subscribers. Nothing is kept while the overlay
    /// is off; a write that asked not to be pushed is neither queued nor
    /// stored, and readers see it in the next commit they open.
    pub fn acked(
        &self,
        index: &str,
        uuid: &str,
        shard: u32,
        records: &[WalRecord<'static>],
        push: bool,
    ) {
        if records.is_empty() {
            return;
        }
        if !self.facts.overlay_on(uuid, shard) {
            if self.keep.keeps(uuid, shard) {
                self.serve.stop(uuid, shard);
                self.queue()
                    .retain(|q| !(q.uuid == uuid && q.shard == shard));
            }
            return;
        }
        // The queue's lock is held across the keep's position and the push
        // onto the queue, so positions follow the queue's order. Nothing takes
        // the keep's lock and then the queue's.
        let mut queue = self.queue();
        let Some(end) = self.keep.acked(uuid, shard, records, push) else {
            return;
        };
        // ALLOC: bounded by MAX_QUEUED a shard, enforced below
        queue.push(Queued {
            index: index.to_owned(),
            uuid: uuid.to_owned(),
            shard,
            records: records.iter().map(owned).collect(),
            end,
        });
        let queued: usize = queue
            .iter()
            .filter(|q| q.uuid == uuid && q.shard == shard)
            .map(|q| q.records.len())
            .sum();
        if queued > MAX_QUEUED {
            queue.retain(|q| !(q.uuid == uuid && q.shard == shard));
            drop(queue);
            self.serve.unsubscribe_all(uuid, shard);
        }
    }

    /// Sends what is queued: each subscribed node gets one request holding
    /// every queued shard it follows, and its answer is counted.
    pub fn flush(&self) {
        let _flushing = self.flushing.lock().unwrap_or_else(PoisonError::into_inner);
        let drained = std::mem::take(&mut *self.queue());
        if drained.is_empty() {
            return;
        }
        // One entry per shard, in the order first acknowledged.
        let mut shards: Vec<Queued> = Vec::new();
        for queued in drained {
            match shards
                .iter_mut()
                .find(|s| s.uuid == queued.uuid && s.shard == queued.shard)
            {
                Some(held) => {
                    held.records.extend(queued.records);
                    // Positions follow the queue's order.
                    held.end = queued.end;
                }
                None => shards.push(queued),
            }
        }
        type Numbered = Vec<(String, u32, Sent)>;
        let mut by_node: BTreeMap<String, (PushRequest, Numbered)> = BTreeMap::new();
        for group in &shards {
            for (node, sent) in self.serve.targets(&group.uuid, group.shard, group.end) {
                let (request, numbered) = by_node.entry(node).or_default();
                request.shards.push((
                    ShardRef {
                        index_name: group.index.clone(),
                        index_uuid: group.uuid.clone(),
                        shard: i32::try_from(group.shard).unwrap_or(i32::MAX),
                    },
                    group.records.clone(),
                ));
                numbered.push((group.uuid.clone(), group.shard, sent));
            }
        }
        for (node, (request, numbered)) in by_node {
            match self.link.send(&node, &request) {
                Ok(answer) => {
                    for ((shard_ref, _), (uuid, shard, sent)) in
                        request.shards.iter().zip(&numbered)
                    {
                        if answer.unwanted.contains(shard_ref) {
                            self.serve.drop_sent(uuid, *shard, &node, *sent);
                        } else {
                            self.serve.confirm(uuid, *shard, &node, *sent);
                        }
                    }
                }
                Err(LinkError) => {
                    // Lost: later pushes would leave a gap in what the reader
                    // holds, so it is pushed nothing more; it subscribes
                    // again when it next opens the shard on a commit.
                    for (uuid, shard, sent) in &numbered {
                        self.serve.drop_sent(uuid, *shard, &node, *sent);
                    }
                }
            }
        }
    }
}
