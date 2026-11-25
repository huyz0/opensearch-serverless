//! Opening a shard that has a history: its published commit restored, what
//! its previous writers logged and never published replayed, and the result
//! published under the new head's term before the shard serves a write.
//!
//! Ported from the writer open of `ShardReconciler` in huyz0/OpenSearch
//! `feature/serverlessshell`; the replay is [`oss_writer::takeover::adopt`].

use std::sync::{Arc, Mutex};

use lucene_store::ByteBuffersDirectory;
use oss_format::{ShardHead, ShardKey};
use oss_writer::bulk::IndexInfo;
use oss_writer::mapping::Mapping;
use oss_writer::publish::Published;
use oss_writer::takeover::{HeadWriters, Predecessors};

use super::{Key, ShardSlot, WriterRole, lock, now_millis};

/// The analyzers an index's pins name, loaded: what a shard opened for an
/// index with pins is given, so that it never indexes a pinned field with the
/// built-in analyzer.
pub trait PinnedModules: Send + Sync {
    /// The analyzers and the names a mapping selects them by.
    fn analyzers(&self) -> Vec<(String, Arc<dyn oss_ext::analyzer::Analyze>)>;

    /// Runs the analyzers as a replay of logged writes when `on`, and as live
    /// writes when not.
    fn replaying(&self, on: bool);
}

/// Loads the modules an index's pins name, from its descriptor settings.
pub trait ModuleLoader: Send + Sync {
    /// The modules `settings` pin, each distinct one read once; none for an
    /// index with no pin.
    ///
    /// # Errors
    ///
    /// The reason the open is refused: a node that cannot run a pin never
    /// opens the index with the built-in analyzer.
    fn load(&self, settings: &[u8], tenant: &str)
    -> Result<Option<Arc<dyn PinnedModules>>, String>;
}

/// What a shard is opened to read and index with: its mapping and the
/// modules its index pins.
pub(super) struct Opening<'a> {
    /// The mapping its documents are read against.
    pub(super) mapping: Arc<Mapping>,
    /// The modules its index pins, when it does.
    pub(super) modules: Option<&'a dyn PinnedModules>,
}

/// Puts a set of modules back to live writes when a replay ends, however it
/// ends.
struct Replaying<'a>(&'a dyn PinnedModules);

impl Drop for Replaying<'_> {
    fn drop(&mut self) {
        self.0.replaying(false);
    }
}

/// What a node needs to take over a shard that has a history.
pub struct Takeover {
    /// The logs of the nodes a head names, each read once however many
    /// shards of a batch name it.
    pub predecessors: Arc<Predecessors>,
    /// Clears the replayed writers from a head.
    pub heads: HeadWriters,
    /// The bytes a replay may buffer before it commits locally.
    pub max_buffer_bytes: usize,
}

impl WriterRole {
    /// Holds shard `shard` of `index`, empty, at `term`, its documents read
    /// against `mapping`.
    ///
    /// # Errors
    ///
    /// The engine not opening.
    pub fn open_shard(
        &self,
        index: &IndexInfo,
        shard: u32,
        term: i64,
        mapping: impl Into<Arc<Mapping>>,
    ) -> Result<(), String> {
        self.open_shard_from(
            index,
            shard,
            term,
            Opening {
                mapping: mapping.into(),
                modules: None,
            },
            |_| Ok(()),
            |_| Ok(None),
        )
        .map(|_| ())
    }

    /// Opens a shard over the files `fill` puts in its directory, and holds
    /// it only once `settle` has brought it up: a shard that cannot be
    /// settled is not held. What `settle` published is recorded as the
    /// shard's newest commit.
    pub(super) fn open_shard_from(
        &self,
        index: &IndexInfo,
        shard: u32,
        term: i64,
        opening: Opening<'_>,
        fill: impl FnOnce(&ByteBuffersDirectory) -> Result<(), String>,
        settle: impl FnOnce(&mut ShardSlot) -> Result<Option<Published>, String>,
    ) -> Result<Option<Published>, String> {
        let key = ShardKey::new(index.uuid.as_str(), shard, Some(index.name.as_str()));
        let mut slot = ShardSlot::open(
            key,
            term,
            opening.mapping,
            opening.modules,
            now_millis,
            fill,
        )?;
        let done = settle(&mut slot)?;
        self.names.insert(&index.uuid, &index.name);
        // ALLOC: one key a shard opened.
        let key = (index.uuid.clone(), shard);
        let max_seq_no = slot.max_seq_no();
        // Lock order: shards, then newest, everywhere both are held.
        let mut shards = lock(&self.shards);
        // ALLOC: one entry a shard opened.
        shards.insert(key.clone(), Arc::new(Mutex::new(slot)));
        // A shard opened again has published nothing yet.
        lock(&self.newest).remove(&key);
        if let Some(p) = &self.publishing {
            p.forget(&key);
            if done.is_some() {
                p.published(&key, max_seq_no);
            }
        }
        drop(shards);
        if let Some(done) = &done {
            self.staged_taken(&index.uuid, shard, &done.manifest);
        }
        Ok(done)
    }

    /// Opens `shard` of `index` over the files `fill` restores, replays the
    /// writers `head` names and publishes the result, then holds the shard.
    ///
    /// # Errors
    ///
    /// The restore, a predecessor's log, the publish or the head failing;
    /// the shard is then not held, and a later write tries again.
    pub(super) fn open_taken(
        &self,
        index: &IndexInfo,
        shard: u32,
        head: &ShardHead,
        opening: Opening<'_>,
        takeover: &Takeover,
        fill: impl FnOnce(&ByteBuffersDirectory) -> Result<(), String>,
    ) -> Result<(), String> {
        let key: Key = (index.uuid.clone(), shard);
        let modules = opening.modules;
        self.open_shard_from(index, shard, head.term, opening, fill, |slot| {
            if head.writers.is_empty() {
                return Ok(None);
            }
            let publishing = self
                .publishing
                .as_ref()
                .ok_or_else(|| String::from("takeover needs a role that publishes"))?;
            // The replay runs the modules as the log's writes ran them.
            let _replaying = modules.map(|m| {
                m.replaying(true);
                Replaying(m)
            });
            publishing.with_publisher(&key, &index.name, |publisher, node, now| {
                slot.adopt(publisher, node, now, takeover, head)
                    .map(|adopted| adopted.published)
            })
        })
        .map(|_| ())
    }
}
