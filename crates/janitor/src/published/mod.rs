//! Which shards a pass sweeps: the ones a node just published a drop in.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use oss_format::ShardRef;

use crate::JanitorError;
use crate::gc::{Collector, Memory, Sweep};

/// A shard's identity, owned so a driver can keep it between passes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedShard {
    /// The index's name.
    pub index: String,
    /// The index's uuid.
    pub uuid: String,
    /// The shard.
    pub shard: u32,
}

impl OwnedShard {
    fn of(shard: ShardRef<'_>) -> Self {
        Self {
            index: shard.index.to_owned(),
            uuid: shard.uuid.to_owned(),
            shard: shard.shard,
        }
    }

    /// The borrowed form.
    pub fn as_ref(&self) -> ShardRef<'_> {
        ShardRef {
            index: &self.index,
            uuid: &self.uuid,
            shard: self.shard,
        }
    }
}

/// A shard a node just published, with the file names of the commit.
#[derive(Debug, Clone, Copy)]
pub struct Published<'a> {
    /// The shard.
    pub shard: ShardRef<'a>,
    /// The term of the manifest it published.
    pub term: u64,
    /// The file names its manifest now names.
    pub files: &'a [&'a str],
}

/// Decides which shards a pass sweeps, and keeps what the sweeps learned.
#[derive(Debug)]
pub struct Sweeps {
    base: String,
    memory: Memory,
    /// The term and files of each shard's last publish, swept or not.
    last_published: BTreeMap<String, (u64, BTreeSet<String>)>,
    /// Shards to sweep on the next pass: forced, or whose sweep failed.
    pending: BTreeMap<String, OwnedShard>,
    watched: BTreeMap<String, OwnedShard>,
}

impl Sweeps {
    /// A driver for the deployment under `base`.
    pub fn new(base: &str) -> Self {
        Self {
            base: base.to_owned(),
            memory: Memory::new(),
            last_published: BTreeMap::new(),
            pending: BTreeMap::new(),
            watched: BTreeMap::new(),
        }
    }

    /// What the sweeps have seen unreferenced.
    pub fn memory(&self) -> &Memory {
        &self.memory
    }

    /// Makes the next pass sweep `shard`: a reaped view may have freed
    /// something the last sweep saw held.
    pub fn force(&mut self, shard: ShardRef<'_>) {
        if let Ok(path) = shard.base_path(&self.base) {
            // ALLOC: one key a shard to sweep, removed by its next successful sweep.
            self.pending.insert(path, OwnedShard::of(shard));
        }
    }

    /// Drops everything remembered of `shard`: the node let it go, and a
    /// later owner starts empty.
    pub fn forget(&mut self, shard: ShardRef<'_>) {
        if let Ok(path) = shard.base_path(&self.base) {
            self.memory.forget(&path);
            self.last_published.remove(&path);
            self.pending.remove(&path);
            self.watched.remove(&path);
        }
    }

    /// Sweeps what `published` and the watch list call for, and reports each
    /// sweep: the published shards that need one, in order, then the watched
    /// ones not published, by storage path (watched and pending alike). A shard whose sweep fails is
    /// reported and left as it was, so the next pass tries it again; the
    /// others are not held up by it.
    pub fn sweep_published(
        &mut self,
        collector: &Collector<'_>,
        published: &[Published<'_>],
        buf: &mut [u8],
    ) -> Vec<(OwnedShard, Result<Sweep, JanitorError>)> {
        let mut reports = Vec::new();
        let mut done = HashSet::new();
        for publish in published {
            let owned = OwnedShard::of(publish.shard);
            let path = match publish.shard.base_path(&self.base) {
                Ok(path) => path,
                Err(error) => {
                    reports.push((owned, Err(error.into())));
                    continue;
                }
            };
            let files = commit_files(publish.files);
            let needed = self.needs_sweep(&path, publish.term, &files);
            self.last_published
                .insert(path.clone(), (publish.term, files));
            if !done.insert(path.clone()) || !needed {
                continue;
            }
            let result = collector.sweep(publish.shard, &mut self.memory, buf);
            self.after(&path, &owned, &result);
            reports.push((owned, result));
        }
        let held: Vec<(String, OwnedShard)> = self
            .watched
            .iter()
            .chain(&self.pending)
            .filter(|(path, _)| !done.contains(*path))
            .map(|(path, shard)| (path.clone(), shard.clone()))
            .collect();
        for (path, owned) in held {
            if !done.insert(path.clone()) {
                continue;
            }
            let result = collector.sweep(owned.as_ref(), &mut self.memory, buf);
            self.after(&path, &owned, &result);
            reports.push((owned, result));
        }
        reports
    }

    /// Whether a publish of `files` at `term` to the shard at `path` can have
    /// made something to delete: a first one, one the shard is pending for,
    /// one in a later term (a file dropped in the live term is not judged
    /// until the term moves on), or one that dropped a file the last publish
    /// had.
    fn needs_sweep(&self, path: &str, term: u64, files: &BTreeSet<String>) -> bool {
        self.pending.contains_key(path)
            || self
                .last_published
                .get(path)
                .is_none_or(|(last_term, last)| term > *last_term || !last.is_subset(files))
    }

    /// Records what a sweep left: pending again if it failed, and watched
    /// while blobs of the shard are on watch.
    fn after(&mut self, path: &str, owned: &OwnedShard, result: &Result<Sweep, JanitorError>) {
        if result.is_ok() {
            self.pending.remove(path);
        } else {
            // ALLOC: one entry a shard whose sweep failed, until one succeeds.
            self.pending.insert(path.to_owned(), owned.clone());
        }
        if self.memory.watching(path) > 0 {
            // ALLOC: one entry a shard with blobs on watch; the memory's
            // caps bound those.
            self.watched.insert(path.to_owned(), owned.clone());
        } else {
            self.watched.remove(path);
        }
    }
}

/// The file names of a commit, without `segments_N`, which every commit
/// replaces.
fn commit_files(files: &[&str]) -> BTreeSet<String> {
    files
        .iter()
        .filter(|name| !name.starts_with("segments_"))
        .map(|name| (*name).to_owned())
        .collect()
}
