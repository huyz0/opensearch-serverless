//! Whether a reader may answer for a shard: only while it knows it holds
//! every write its writer answered as refreshed.
//!
//! Ported from `ensureFresh` of the shell's `TailOverlays`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/transport/TailOverlays.java`)
//! in its `everywhere` visibility mode. Under a grant from the writer the
//! reader holds every such write; without one it asks the writer, once for
//! every search that finds none meanwhile, and one that still cannot know
//! refuses, so the search asks the next copy. Blocking: the ask is a round
//! trip.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use oss_transport::actions::tail::{CHECK, FetchRequest, FetchResponse, ShardRef};
use oss_transport::stream::TaskId;

use crate::overlay_builder::TailFetcher;
use crate::tail_held::TailHeld;
use crate::tail_reader_link::Clock;

type Key = (String, u32);

/// Decides whether a shard held here may answer a search.
pub struct TailFresh {
    held: Arc<TailHeld>,
    fetcher: Arc<dyn TailFetcher>,
    subscriber: String,
    clock: Clock,
    /// Whether grants are in force (`everywhere`); off, every search may
    /// answer.
    grants: bool,
    /// Whether a search has the engine take what is held for it before it
    /// answers without a grant (`serving`): the writer waited for these
    /// records to be taken, so they must be searchable.
    applies: bool,
    /// Held while the writer is asked for a grant, so searches that find
    /// none ask once between them.
    asking: Mutex<HashMap<Key, Arc<Mutex<()>>>>,
    checks: AtomicU64,
    refusals: AtomicU64,
    unreachable: AtomicU64,
}

impl TailFresh {
    /// A check over `held`, asking writers through `fetcher` as `subscriber`.
    pub fn new(
        held: Arc<TailHeld>,
        fetcher: Arc<dyn TailFetcher>,
        subscriber: String,
        clock: Clock,
        grants: bool,
    ) -> Self {
        Self {
            held,
            fetcher,
            subscriber,
            clock,
            grants,
            applies: false,
            asking: Mutex::new(HashMap::new()),
            checks: AtomicU64::new(0),
            refusals: AtomicU64::new(0),
            unreachable: AtomicU64::new(0),
        }
    }

    /// This check with a search taking what is held for it first even where
    /// no grant is asked.
    #[must_use]
    pub fn applying(mut self, applies: bool) -> Self {
        self.applies = applies;
        self
    }

    /// Round trips made to a writer for a grant before a search, searches
    /// refused for want of one, and writers that did not answer.
    pub fn counts(&self) -> (u64, u64, u64) {
        (
            self.checks.load(Ordering::Relaxed),
            self.refusals.load(Ordering::Relaxed),
            self.unreachable.load(Ordering::Relaxed),
        )
    }

    fn granted(&self, uuid: &str, shard: u32, writer: &str) -> bool {
        self.held.granted(uuid, shard, writer, (self.clock)())
    }

    fn ask(
        &self,
        index: &str,
        uuid: &str,
        shard: u32,
        writer: &str,
        commit_checkpoint: i64,
        subscription: i64,
    ) -> Option<FetchResponse> {
        // ALLOC: one request a grant ask, a few short strings.
        let request = FetchRequest {
            parent: TaskId::EMPTY,
            shard: ShardRef {
                index_name: index.to_owned(),
                index_uuid: uuid.to_owned(),
                shard: i32::try_from(shard).unwrap_or(i32::MAX),
            },
            after_seq_no: commit_checkpoint,
            subscriber: self.subscriber.clone(),
            subscription,
        };
        self.fetcher.fetch(writer, &request)
    }

    /// Whether shard `shard` of `uuid`, open on a commit at
    /// `commit_checkpoint`, may answer a search that started before this
    /// call: whether it holds every write `writer` may have answered as
    /// refreshed. `apply_pending` has the engine take what is held for it, so
    /// a record it cannot take is known.
    ///
    /// Under a grant from the writer it may. Without one it asks the writer
    /// for a renewal of the subscription it holds, or a subscription anew
    /// with what is kept past its commit. One that still holds no grant (no
    /// overlay, a gap, a record its engine left out) may answer only if its
    /// commit holds every write the writer says it may have answered as
    /// refreshed. A writer that does not answer is treated as dead: the
    /// reader serves its commit, as it does for any acknowledged write until
    /// the shard is taken over.
    pub fn ensure_fresh(
        &self,
        index: &str,
        uuid: &str,
        shard: u32,
        writer: &str,
        commit_checkpoint: i64,
        apply_pending: &dyn Fn(),
    ) -> bool {
        if !self.grants {
            if self.applies {
                apply_pending();
            }
            return true;
        }
        if self.granted(uuid, shard, writer) {
            apply_pending();
            if self.granted(uuid, shard, writer) {
                return true;
            }
        }
        let mut claimed_through = i64::MAX;
        let mut asked = false;
        let mut unanswered = false;
        let gate = Arc::clone(
            self.asking
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry((uuid.to_owned(), shard))
                .or_default(),
        );
        {
            let _asking = gate.lock().unwrap_or_else(PoisonError::into_inner);
            if !self.granted(uuid, shard, writer) {
                let subscription = self.held.subscription_to_ask(uuid, shard, writer);
                self.checks.fetch_add(1, Ordering::Relaxed);
                let sent_at = (self.clock)();
                if let Some(answer) =
                    self.ask(index, uuid, shard, writer, commit_checkpoint, subscription)
                {
                    asked = true;
                    claimed_through = answer.claimed_through;
                    if subscription != CHECK {
                        self.held
                            .answered(uuid, shard, writer, subscription, sent_at, &answer);
                    }
                } else {
                    unanswered = true;
                }
            }
        }
        apply_pending();
        if self.granted(uuid, shard, writer) {
            return true;
        }
        if unanswered {
            // The writer did not answer, or this node's leases show it gone:
            // treated as dead for the whole search. Asking again for how far
            // its writes went could find it alive a moment later (a lease
            // view that was stale and has since been refreshed) and refuse a
            // search that the first answer let through.
            self.unreachable.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        if !asked {
            let Some(answer) = self.ask(index, uuid, shard, writer, commit_checkpoint, CHECK)
            else {
                self.unreachable.fetch_add(1, Ordering::Relaxed);
                return true;
            };
            claimed_through = answer.claimed_through;
        }
        if commit_checkpoint >= claimed_through {
            return true;
        }
        self.refusals.fetch_add(1, Ordering::Relaxed);
        false
    }
}
