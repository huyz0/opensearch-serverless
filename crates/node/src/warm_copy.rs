//! A joining reader taking one peer's hot pages into its own cache.
//!
//! The joiner asks the peer for its hot set, keeps only the pages its own
//! open commit would read (see [`Expected`]), then asks for them in batches
//! and inserts each page the peer answers at exactly the length the commit
//! stores. A peer that does not answer, names another page size, or sends a
//! page that does not match ends its own copy and nothing else's. After each
//! batch the copy pauses long enough that the bytes taken never exceed the
//! byte rate, and it stops at once when `expired` says the wait is over.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use lucene_store::page_cache::PageCache;
use oss_transport::actions::tail::ShardRef;
use oss_transport::actions::warm::{
    HotFile, HotSetRequest, HotSetResponse, MAX_PAGES_PER_REQUEST, PageAsk, PagesRequest,
    PagesResponse,
};
use oss_transport::stream::TaskId;

use crate::warm_expect::Expected;

/// One peer, as the copy asks it. `None` is any failure: not live, no
/// answer in time, a refusal, or bytes that do not decode.
pub trait Peer {
    /// The peer's hot set for `request`.
    fn hot_set(&self, request: &HotSetRequest) -> Option<HotSetResponse>;
    /// The pages of `request`.
    fn pages(&self, request: &PagesRequest) -> Option<PagesResponse>;
}

/// Sleeping, injected so the pacing is tested without a clock.
pub trait Pause {
    /// Waits `duration`.
    fn pause(&self, duration: Duration);
}

/// Opening the joiner's own shards.
pub trait Shards {
    /// What the open commit of `shard` accepts, or `None` when the joiner
    /// does not serve that shard or cannot open it.
    fn expected(&self, shard: &ShardRef) -> Option<Arc<Expected>>;
}

/// The bytes named and the bytes taken, across every peer, for readiness.
#[derive(Debug, Default)]
pub struct Progress {
    named: AtomicU64,
    copied: AtomicU64,
}

impl Progress {
    /// Bytes named by the peers' lists, less a failed peer's remainder.
    pub fn named(&self) -> u64 {
        self.named.load(Ordering::Acquire)
    }

    /// Bytes inserted into the cache.
    pub fn copied(&self) -> u64 {
        self.copied.load(Ordering::Acquire)
    }

    /// Sets both counts, for a test of what reads them.
    #[cfg(test)]
    pub(crate) fn seed(&self, named: u64, copied: u64) {
        self.named.store(named, Ordering::Release);
        self.copied.store(copied, Ordering::Release);
    }

    /// Whether `fraction` of the named bytes are in; true when none are named.
    pub fn reached(&self, fraction: f64) -> bool {
        let named = self.named();
        // A byte count is far below 2^53.
        #[allow(clippy::cast_precision_loss)]
        let target = named as f64 * fraction;
        #[allow(clippy::cast_precision_loss)]
        let copied = self.copied() as f64;
        copied >= target
    }
}

/// How one peer's copy went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Every page named was taken or was no longer held.
    Done,
    /// The peer did not answer, refused, or named another page size.
    Unreachable,
    /// The peer sent a page the commit does not accept.
    Bad,
    /// `expired` said the wait was over.
    Stopped,
}

/// What a copy is given besides the peer.
pub struct Settings<'a> {
    /// The joiner's node name, for the peer's placement.
    pub joiner: &'a str,
    /// The most shards asked of the peer.
    pub max_shards: i32,
    /// The most stored bytes asked of the peer.
    pub max_bytes: i64,
    /// The most bytes taken a second; at least 1.
    pub bytes_per_second: u64,
    /// The most of the peer's shards opened at once; at least 1.
    pub open_parallel: usize,
}

/// Opens the shards a peer named, up to `parallel` at once, answers in
/// the peer's order. Opening one reads its commit from the store, so a long
/// list one after another is a chain of round trips as long as the list;
/// side by side it takes as long as the list divided by the workers.
fn open_shards(
    shards: &(dyn Shards + Sync),
    named: &[(ShardRef, Vec<HotFile>)],
    parallel: usize,
) -> Vec<Option<Arc<Expected>>> {
    let next = AtomicUsize::new(0);
    let workers = parallel.max(1).min(named.len());
    // ALLOC: one slot per shard named, bounded by the request's max_shards.
    let mut opened: Vec<Option<Arc<Expected>>> = vec![None; named.len()];
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    // ALLOC: the shards this worker opened, a share of the list.
                    let mut mine = Vec::new();
                    loop {
                        let at = next.fetch_add(1, Ordering::AcqRel);
                        let Some((shard, _)) = named.get(at) else {
                            break;
                        };
                        mine.push((at, shards.expected(shard)));
                    }
                    mine
                })
            })
            .collect();
        for handle in handles {
            for (at, expected) in handle.join().unwrap_or_default() {
                if let Some(slot) = opened.get_mut(at) {
                    *slot = expected;
                }
            }
        }
    });
    opened
}

fn pause_for(bytes: u64, rate: u64) -> Duration {
    Duration::from_nanos(
        bytes
            .saturating_mul(1_000_000_000)
            .checked_div(rate.max(1))
            .unwrap_or(u64::MAX),
    )
}

/// Takes `peer`'s hot pages into `cache`, stopping when `expired` is true.
pub fn copy_from_peer(
    peer: &dyn Peer,
    shards: &(dyn Shards + Sync),
    cache: &PageCache,
    settings: &Settings<'_>,
    pause: &dyn Pause,
    progress: &Progress,
    expired: &(dyn Fn() -> bool + Sync),
) -> Outcome {
    let request = HotSetRequest {
        parent: TaskId::default(),
        joiner: settings.joiner.to_owned(),
        max_shards: settings.max_shards,
        max_bytes: settings.max_bytes,
    };
    let Some(listed) = peer.hot_set(&request) else {
        return Outcome::Unreachable;
    };
    let ours = i32::try_from(cache.config().page_size).unwrap_or(-1);
    if listed.page_size != ours {
        return Outcome::Unreachable;
    }
    // ALLOC: one ask per named page, bounded by the request's max_bytes.
    let mut wanted: Vec<(PageAsk, u64)> = Vec::new();
    let mut accepts: Vec<Arc<Expected>> = Vec::new();
    let mut owners: Vec<usize> = Vec::new();
    let opened = open_shards(shards, &listed.shards, settings.open_parallel);
    for ((_, files), expected) in listed.shards.iter().zip(opened) {
        let Some(expected) = expected else {
            continue;
        };
        accepts.push(expected);
        let at = accepts.len().saturating_sub(1);
        let expected = &accepts[at];
        for file in files {
            for page in &file.pages {
                let Some(len) = expected.page_len(&file.key, page.page) else {
                    continue;
                };
                if len != u64::from(page.len) {
                    continue;
                }
                wanted.push((
                    PageAsk {
                        key: file.key.clone(),
                        page: page.page,
                    },
                    len,
                ));
                owners.push(at);
            }
        }
    }
    let total: u64 = wanted.iter().map(|(_, len)| *len).sum();
    progress.named.fetch_add(total, Ordering::AcqRel);
    let mut taken = 0u64;
    let mut index = 0usize;
    let outcome = loop {
        if index >= wanted.len() {
            break Outcome::Done;
        }
        if expired() {
            break Outcome::Stopped;
        }
        let end = index
            .saturating_add(MAX_PAGES_PER_REQUEST)
            .min(wanted.len());
        let batch = &wanted[index..end];
        let ask = PagesRequest {
            parent: TaskId::default(),
            // ALLOC: one ask per page of the batch, at most MAX_PAGES_PER_REQUEST.
            asks: batch.iter().map(|(ask, _)| ask.clone()).collect(),
        };
        let Some(answer) = peer.pages(&ask) else {
            break Outcome::Unreachable;
        };
        let asked: HashSet<(&str, u64)> = batch
            .iter()
            .map(|(ask, _)| (ask.key.as_str(), ask.page))
            .collect();
        let mut bytes_in = 0u64;
        let mut bad = false;
        for page in &answer.pages {
            let owner = batch
                .iter()
                .position(|(ask, _)| ask.key == page.key && ask.page == page.page)
                .and_then(|at| owners.get(index.saturating_add(at)));
            let accepted = asked.contains(&(page.key.as_str(), page.page))
                && owner
                    .is_some_and(|o| accepts[*o].accepts(&page.key, page.page, page.bytes.len()));
            if !accepted {
                bad = true;
                break;
            }
            cache.insert_page(&page.key, page.page, &page.bytes);
            bytes_in = bytes_in.saturating_add(u64::try_from(page.bytes.len()).unwrap_or(0));
        }
        progress.copied.fetch_add(bytes_in, Ordering::AcqRel);
        taken = taken.saturating_add(bytes_in);
        if bad {
            break Outcome::Bad;
        }
        index = end;
        pause.pause(pause_for(bytes_in, settings.bytes_per_second));
    };
    if matches!(outcome, Outcome::Unreachable | Outcome::Bad) {
        // What the peer will not give is no longer waited for.
        progress
            .named
            .fetch_sub(total.saturating_sub(taken), Ordering::AcqRel);
    }
    outcome
}

#[cfg(test)]
mod tests;
