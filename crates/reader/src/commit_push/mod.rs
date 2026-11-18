//! The commits a shard's writer pushed to this reader, kept until a reader
//! open takes one, and the order that picks the newer of two commits.
//!
//! Ported from the receive side of the shell's `CommitPush`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/transport/CommitPush.java`:
//! the pushed map, its keep time and bound), `NewestCommits.newerThan` and
//! `CommitManifest.stagedAfter`. Sans I/O: the transport that carries a push
//! calls [`PushedCommits::push`], and the open that wants the commit calls
//! [`PushedCommits::take`], each under the caller's lock and clock.

use std::collections::BTreeMap;

use oss_format::CommitManifest;

/// How long a pushed commit is kept for the open it is meant for; older, the
/// register decides again.
pub const KEEP_MILLIS: u64 = 10_000;

/// How many shards' pushed commits the shell keeps.
pub const MAX_SHARDS: usize = 4096;

/// Why a push was not kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The bytes are not a commit register's value.
    NotACommit,
    /// The mailbox is at its shard bound or its byte bound.
    Full,
}

/// A shard, by its index's uuid and number.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ShardKey {
    uuid: String,
    shard: u32,
}

impl ShardKey {
    /// The key of shard `shard` of the index with `uuid`.
    pub fn new(uuid: &str, shard: u32) -> Self {
        Self {
            // ALLOC: one uuid a key, at most the shards the mailbox holds.
            uuid: uuid.to_owned(),
            shard,
        }
    }
}

/// Where a commit sits among those its writer staged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stage {
    term: u64,
    writer: Option<String>,
    sequence: i64,
}

impl Stage {
    /// A commit staged by `writer` in `term` as its `sequence`th, or
    /// negative when it was not staged.
    pub fn new(term: u64, writer: Option<String>, sequence: i64) -> Self {
        Self {
            term,
            writer,
            sequence,
        }
    }

    /// Where `manifest` sits.
    pub fn of(manifest: &CommitManifest<'_>) -> Self {
        Self {
            term: manifest.term,
            // ALLOC: one writer name a commit.
            writer: manifest.writer.as_deref().map(str::to_owned),
            sequence: manifest.stage_sequence,
        }
    }
}

/// `CommitManifest.stagedAfter`: `a` is later than `b`, both staged by one
/// writer in one term.
pub fn staged_after(a: &Stage, b: &Stage) -> bool {
    a.sequence >= 0
        && b.sequence >= 0
        && a.term == b.term
        && a.writer.is_some()
        && a.writer == b.writer
        && a.sequence > b.sequence
}

/// `NewestCommits.newerThan`: `newest`, what the register's writer staged
/// last, when it is staged after the register's commit; else the register's.
pub fn newer_than<'a>(register: &'a Stage, newest: Option<&'a Stage>) -> &'a Stage {
    match newest {
        Some(n) if register.sequence >= 0 && staged_after(n, register) => n,
        _ => register,
    }
}

struct Held {
    value: Vec<u8>,
    stage: Stage,
    at_millis: u64,
}

impl Held {
    fn expired(&self, now_millis: u64) -> bool {
        now_millis.saturating_sub(self.at_millis) > KEEP_MILLIS
    }
}

/// The newest commit pushed for each shard, bounded in shards and in bytes.
pub struct PushedCommits {
    held: BTreeMap<ShardKey, Held>,
    bytes: usize,
    max_shards: usize,
    max_bytes: usize,
}

impl PushedCommits {
    /// A mailbox for at most `max_shards` shards and `max_bytes` of commits.
    pub fn new(max_shards: usize, max_bytes: usize) -> Self {
        Self {
            // ALLOC: empty; at most `max_shards` commits, within `max_bytes`.
            held: BTreeMap::new(),
            bytes: 0,
            max_shards,
            max_bytes,
        }
    }

    /// How many shards have a commit waiting.
    pub fn len(&self) -> usize {
        self.held.len()
    }

    /// Whether no shard has a commit waiting.
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// The bytes of the commits waiting.
    pub fn held_bytes(&self) -> usize {
        self.bytes
    }

    /// Keeps `value`, a commit register's value, for `key`. Gives whether
    /// it was kept: a commit staged before the one already held is not.
    ///
    /// # Errors
    ///
    /// [`Refused::NotACommit`] for bytes that do not parse as a commit;
    /// [`Refused::Full`] past the shard or byte bound, after dropping what is
    /// past its keep time.
    pub fn push(&mut self, key: &ShardKey, value: &[u8], now_millis: u64) -> Result<bool, Refused> {
        let manifest = CommitManifest::parse(value).map_err(|_| Refused::NotACommit)?;
        let stage = Stage::of(&manifest);
        if let Some(held) = self.held.get(key)
            && !held.expired(now_millis)
            && staged_after(&held.stage, &stage)
        {
            return Ok(false);
        }
        if !self.fits(key, value.len()) {
            self.held.retain(|_, h| !h.expired(now_millis));
            self.bytes = self.held.values().map(|h| h.value.len()).sum();
            if !self.fits(key, value.len()) {
                return Err(Refused::Full);
            }
        }
        let replaced = self.held.insert(
            key.clone(),
            Held {
                // ALLOC: one commit a shard, within the byte bound.
                value: value.to_vec(),
                stage,
                at_millis: now_millis,
            },
        );
        let before = replaced.map_or(0, |h| h.value.len());
        self.bytes = self
            .bytes
            .saturating_sub(before)
            .saturating_add(value.len());
        Ok(true)
    }

    /// The commit pushed for `key`, once: it is not kept after. Nothing when
    /// none came or it is past its keep time.
    pub fn take(&mut self, key: &ShardKey, now_millis: u64) -> Option<Vec<u8>> {
        let held = self.held.remove(key)?;
        self.bytes = self.bytes.saturating_sub(held.value.len());
        (!held.expired(now_millis)).then_some(held.value)
    }

    /// Whether a commit of `size` bytes for `key` is within both bounds.
    fn fits(&self, key: &ShardKey, size: usize) -> bool {
        let existing = self.held.get(key).map(|h| h.value.len());
        let shards_ok = existing.is_some() || self.held.len() < self.max_shards;
        let after = self
            .bytes
            .saturating_sub(existing.unwrap_or(0))
            .saturating_add(size);
        shards_ok && after <= self.max_bytes
    }
}
