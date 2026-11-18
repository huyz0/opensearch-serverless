//! What a writer does after it publishes a commit: tell the readers of the
//! shard (`internal:serverless/commit/push`).
//!
//! Ported from the send side of the shell's `CommitPush`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/transport/CommitPush.java`:
//! `published`, `readersOpen`). The commit register value goes to the readers
//! placement ranks first for the shard, other than this node, in parallel. A
//! reader that answers that it has the shard open is remembered, so a later
//! push can carry blocks to it. A lost push costs nothing: the reader's open
//! reads the register.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use oss_transport::actions::push::PushRequest;
use oss_transport::stream::TaskId;

/// How many shards' open readers are remembered.
pub const MAX_SHARDS: usize = 4096;

/// A push that did not reach its reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkError;

/// The readers a shard's pushes go to.
pub trait PushTargets: Sync {
    /// The readers of shard `shard` of `index`, best first, other than this
    /// node.
    fn readers(&self, index: &str, shard: u32) -> Vec<String>;
}

/// How a push reaches one reader.
pub trait PushLink: Sync {
    /// Sends `request` to `target` and gives whether it has the shard open.
    ///
    /// # Errors
    ///
    /// [`LinkError`] when the push was not answered.
    fn send(&self, target: &str, request: &PushRequest) -> Result<bool, LinkError>;
}

/// What a publish tells the readers.
#[derive(Clone, Copy, Debug)]
pub struct Published<'a> {
    /// The index's name.
    pub index: &'a str,
    /// The index's uuid.
    pub uuid: &'a str,
    /// The shard.
    pub shard: u32,
    /// The commit register's value, as the publish wrote it.
    pub commit: &'a [u8],
    /// The descriptor's mapping version, or -1.
    pub mapping_version: i64,
    /// The descriptor's settings version, or -1.
    pub settings_version: i64,
}

/// Sends published commits to readers.
pub struct PushSender<T, L> {
    targets: T,
    link: L,
    enabled: bool,
    open: Mutex<BTreeMap<(String, u32), BTreeSet<String>>>,
    sent: AtomicU64,
    failed: AtomicU64,
}

impl<T: PushTargets, L: PushLink> PushSender<T, L> {
    /// A sender over `targets` and `link`; `enabled` false sends nothing.
    pub fn new(targets: T, link: L, enabled: bool) -> Self {
        Self {
            targets,
            link,
            enabled,
            // ALLOC: empty; at most MAX_SHARDS shards, a few readers each.
            open: Mutex::new(BTreeMap::new()),
            sent: AtomicU64::new(0),
            failed: AtomicU64::new(0),
        }
    }

    /// Sends the commit of a publish to the shard's readers.
    pub fn published(&self, published: Published<'_>) {
        if !self.enabled {
            return;
        }
        let targets = self.targets.readers(published.index, published.shard);
        if targets.is_empty() {
            return;
        }
        let request = PushRequest {
            parent: TaskId::EMPTY,
            // ALLOC: one name and one uuid a publish.
            index_name: published.index.to_owned(),
            index_uuid: published.uuid.to_owned(),
            shard: i32::try_from(published.shard).unwrap_or(i32::MAX),
            // ALLOC: one commit a publish, a register value.
            commit: published.commit.to_vec(),
            blocks: Vec::new(),
            mapping_version: published.mapping_version,
            settings_version: published.settings_version,
        };
        let answers: Vec<(&String, Result<bool, LinkError>)> = std::thread::scope(|scope| {
            // ALLOC: one thread a target, a hot shard's few readers.
            let sends: Vec<_> = targets
                .iter()
                .map(|target| (target, scope.spawn(|| self.link.send(target, &request))))
                .collect();
            sends
                .into_iter()
                .map(|(target, send)| (target, send.join().unwrap_or(Err(LinkError))))
                .collect()
        });
        let mut open: BTreeSet<String> = self
            .open_readers(published.index, published.shard)
            .into_iter()
            .collect();
        for (target, answer) in answers {
            match answer {
                Ok(is_open) => {
                    self.sent.fetch_add(1, Ordering::Relaxed);
                    if is_open {
                        // ALLOC: one name a reader that has the shard open.
                        open.insert(target.clone());
                    } else {
                        open.remove(target);
                    }
                }
                Err(LinkError) => {
                    self.failed.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        self.remember(published.index, published.shard, open);
    }

    /// Keeps `open` as the readers of the shard that have it open.
    fn remember(&self, index: &str, shard: u32, open: BTreeSet<String>) {
        let mut map = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        // ALLOC: one key a shard, at most MAX_SHARDS.
        let key = (index.to_owned(), shard);
        if open.is_empty() {
            map.remove(&key);
            return;
        }
        if map.len() >= MAX_SHARDS && !map.contains_key(&key) {
            map.clear();
        }
        map.insert(key, open);
    }

    /// The readers that answered, at the last push of the shard, that they
    /// have it open.
    pub fn open_readers(&self, index: &str, shard: u32) -> Vec<String> {
        let map = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        // ALLOC: one name a reader, a hot shard's few.
        map.get(&(index.to_owned(), shard))
            .map(|open| open.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// How many shards have a remembered open reader.
    pub fn remembered_shards(&self) -> usize {
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Pushes answered.
    pub fn sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }

    /// Pushes not answered.
    pub fn failed(&self) -> u64 {
        self.failed.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use super::*;

    struct Fixed(Vec<&'static str>);

    impl PushTargets for Fixed {
        fn readers(&self, _index: &str, _shard: u32) -> Vec<String> {
            self.0.iter().map(|s| (*s).to_owned()).collect()
        }
    }

    /// Records every request; answers per target: open, not open, or lost.
    struct Recording {
        answers: BTreeMap<&'static str, Option<bool>>,
        seen: Mutex<Vec<(String, PushRequest)>>,
    }

    impl Recording {
        fn new(answers: &[(&'static str, Option<bool>)]) -> Self {
            Self {
                answers: answers.iter().copied().collect(),
                seen: Mutex::new(Vec::new()),
            }
        }
    }

    impl PushLink for Recording {
        fn send(&self, target: &str, request: &PushRequest) -> Result<bool, LinkError> {
            self.seen
                .lock()
                .expect("lock")
                .push((target.to_owned(), request.clone()));
            self.answers[target].ok_or(LinkError)
        }
    }

    fn published(shard: u32) -> Published<'static> {
        Published {
            index: "logs",
            uuid: "u1",
            shard,
            commit: b"commit-bytes",
            mapping_version: 4,
            settings_version: 2,
        }
    }

    fn seen_targets(link: &Recording) -> Vec<String> {
        let mut targets: Vec<String> = link
            .seen
            .lock()
            .expect("lock")
            .iter()
            .map(|(t, _)| t.clone())
            .collect();
        targets.sort();
        targets
    }

    #[test]
    fn a_published_commit_goes_once_to_each_target_as_published() {
        let sender = PushSender::new(
            Fixed(vec!["r1", "r2"]),
            Recording::new(&[("r1", Some(true)), ("r2", Some(false))]),
            true,
        );
        sender.published(published(3));
        assert_eq!(seen_targets(&sender.link), ["r1", "r2"]);
        for (_, request) in sender.link.seen.lock().expect("lock").iter() {
            assert_eq!(request.parent, TaskId::EMPTY);
            assert_eq!(request.index_name, "logs");
            assert_eq!(request.index_uuid, "u1");
            assert_eq!(request.shard, 3);
            assert_eq!(request.commit, b"commit-bytes");
            assert!(request.blocks.is_empty());
            assert_eq!(request.mapping_version, 4);
            assert_eq!(request.settings_version, 2);
        }
        assert_eq!((sender.sent(), sender.failed()), (2, 0));
    }

    #[test]
    fn a_target_that_fails_does_not_stop_the_others() {
        let sender = PushSender::new(
            Fixed(vec!["r1", "r2", "r3"]),
            Recording::new(&[("r1", None), ("r2", Some(true)), ("r3", Some(true))]),
            true,
        );
        sender.published(published(0));
        assert_eq!(seen_targets(&sender.link), ["r1", "r2", "r3"]);
        assert_eq!((sender.sent(), sender.failed()), (2, 1));
        assert_eq!(sender.open_readers("logs", 0), ["r2", "r3"]);
    }

    #[test]
    fn with_no_targets_or_pushing_off_nothing_is_sent() {
        let none = PushSender::new(Fixed(vec![]), Recording::new(&[]), true);
        none.published(published(0));
        assert!(none.link.seen.lock().expect("lock").is_empty());
        let off = PushSender::new(
            Fixed(vec!["r1"]),
            Recording::new(&[("r1", Some(true))]),
            false,
        );
        off.published(published(0));
        assert!(off.link.seen.lock().expect("lock").is_empty());
        assert_eq!((off.sent(), off.failed()), (0, 0));
    }

    #[test]
    fn readers_that_answered_open_are_remembered_until_one_says_not() {
        let sender = PushSender::new(
            Fixed(vec!["r1", "r2"]),
            Recording::new(&[("r1", Some(true)), ("r2", Some(true))]),
            true,
        );
        sender.published(published(0));
        assert_eq!(sender.open_readers("logs", 0), ["r1", "r2"]);
        assert!(sender.open_readers("logs", 1).is_empty());
        assert!(sender.open_readers("other", 0).is_empty());
        let closed = PushSender {
            targets: Fixed(vec!["r1", "r2"]),
            link: Recording::new(&[("r1", Some(false)), ("r2", Some(true))]),
            enabled: true,
            open: Mutex::new(std::mem::take(&mut *sender.open.lock().expect("lock"))),
            sent: AtomicU64::new(0),
            failed: AtomicU64::new(0),
        };
        closed.published(published(0));
        assert_eq!(closed.open_readers("logs", 0), ["r2"]);
    }

    #[test]
    fn a_shard_with_no_open_reader_is_not_remembered() {
        let sender = PushSender::new(
            Fixed(vec!["r1"]),
            Recording::new(&[("r1", Some(false))]),
            true,
        );
        sender.published(published(0));
        assert_eq!(sender.remembered_shards(), 0);
    }

    #[test]
    fn the_remembered_shards_stay_within_the_bound() {
        let sender = PushSender::new(
            Fixed(vec!["r1"]),
            Recording::new(&[("r1", Some(true))]),
            true,
        );
        for shard in 0..=u32::try_from(MAX_SHARDS).expect("small") {
            sender.published(published(shard));
            assert!(sender.remembered_shards() <= MAX_SHARDS);
        }
        assert_eq!(
            sender.open_readers("logs", u32::try_from(MAX_SHARDS).expect("small")),
            ["r1"]
        );
        assert!(
            sender.remembered_shards() < MAX_SHARDS,
            "others were dropped"
        );
    }
}
