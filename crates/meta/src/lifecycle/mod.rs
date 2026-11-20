//! Creating and deleting an index, and finishing a delete once its intent
//! falls due.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/IndexLifecycle.java`
//! (`createIndex`, `deleteIndex`, `reclaim`). A create is the descriptor's
//! conditional create, after the longest key its layout will need is
//! checked. A delete reads the descriptor and its heads, files a reclaim
//! intent, removes the descriptor at the generation it read, then the
//! heads, the aliases' mentions of it and its bytes; each later step is
//! scoped to the deleted uuid, so a new index of the same name is never
//! touched. The unit layout of a shard mode is carried as the record gives
//! it: computing one is the shard modes' work.

mod queue;
mod shards;

use std::borrow::Cow;

use oss_format::{IndexRecord, ReclaimIntent, ShardRef};

pub use queue::{DEFAULT_RECLAIM_DELAY_MILLIS, ReclaimQueue};
pub use shards::{HeadAt, Purged, ShardHeads, ShardPins};

use crate::alias_mutations::AliasMutations;
use crate::descriptors::{DeleteMode, DescriptorStore, Target};
use crate::error::MetaError;
use crate::names::validate_index_or_alias;

/// What a delete did once the descriptor was gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deleted {
    /// Aliases that may still name the index because they kept changing.
    pub aliases_left: usize,
    /// What purging the bytes left, or `None` when a shard had a writer and
    /// the purge waits for the intent to fall due.
    pub purged: Option<Purged>,
}

/// The index lifecycle of one deployment, with the buffer it reads
/// descriptors through.
pub struct IndexLifecycle<'a> {
    descriptors: &'a DescriptorStore,
    aliases: AliasMutations<'a>,
    queue: &'a ReclaimQueue,
    heads: &'a dyn ShardHeads,
    pins: &'a dyn ShardPins,
    buf: Vec<u8>,
    inline_purge_max_shards: u32,
    repurge_pinned: bool,
}

/// An index's identity, copied out of the buffer it was read through.
struct Read {
    generation: u64,
    uuid: String,
    shards: u32,
    aliased_by: Vec<Cow<'static, str>>,
}

impl<'a> IndexLifecycle<'a> {
    /// The lifecycle over `descriptors`, changing aliases through
    /// `aliases`, filing intents in `queue`, and reaching shards through
    /// `heads` and `pins`; descriptors are read through a buffer of
    /// `register_bytes`, taken once.
    pub fn new(
        descriptors: &'a DescriptorStore,
        aliases: AliasMutations<'a>,
        queue: &'a ReclaimQueue,
        heads: &'a dyn ShardHeads,
        pins: &'a dyn ShardPins,
        register_bytes: usize,
    ) -> Self {
        Self {
            descriptors,
            aliases,
            queue,
            heads,
            pins,
            // ALLOC: one register buffer a lifecycle, once.
            buf: vec![0; register_bytes],
            inline_purge_max_shards: u32::MAX,
            repurge_pinned: false,
        }
    }

    /// Purges inside a delete only an index of at most `shards` shards: a
    /// larger one files its intent as if a writer held it, so the purge,
    /// which lists and deletes every shard one after another, runs at the
    /// reclaim and not while a request waits. The default has no limit, as
    /// the Java.
    #[must_use]
    pub fn with_inline_purge_limit(mut self, shards: u32) -> Self {
        self.inline_purge_max_shards = shards;
        self
    }

    /// Files an intent again when a purge leaves shards a pin names, so
    /// their bytes go once the pin ends ([`Self::reclaim_at`] for a reclaim,
    /// a delete for its own purge). Each retry costs the pins' read, until
    /// the pin expires. Off by default, as the Java leaves them.
    #[must_use]
    pub fn with_repurge_pinned(mut self, on: bool) -> Self {
        self.repurge_pinned = on;
        self
    }

    /// Creates the index `record` describes and answers its descriptor's
    /// generation. Nothing is cleared first: the uuid in every shard's path
    /// is minted per create, so no earlier index of the name wrote there.
    ///
    /// A retry of a create whose response was lost finds its own record and
    /// answers its generation: the uuid is the create's, so a name holding
    /// that uuid holds this create. That costs one read, only on a refusal.
    ///
    /// # Errors
    ///
    /// [`MetaError::InvalidName`] for a name no index may have, and
    /// [`MetaError::Blob`] for a shard key the store cannot hold, before any
    /// request; otherwise as [`DescriptorStore::create`].
    pub fn create_index(
        &mut self,
        record: &IndexRecord<'_>,
        now_millis: i64,
    ) -> Result<u64, MetaError> {
        validate_index_or_alias(&record.name)?;
        let longest = ShardRef {
            index: &record.name,
            uuid: &record.uuid,
            shard: record.number_of_shards.saturating_sub(1),
        }
        .manifest_key(&self.descriptors.base)?;
        self.descriptors.store.validate_key(&longest)?;
        match self.descriptors.create(record, now_millis, &mut self.buf) {
            Err(MetaError::AlreadyExists) => {
                let held = self.descriptors.resolve(&record.name, &mut self.buf)?;
                match held.target {
                    Target::Index(index) if index.uuid == record.uuid => Ok(held.generation),
                    _ => Err(MetaError::AlreadyExists),
                }
            }
            created => created,
        }
    }

    /// Deletes index `name`, only the incarnation `expected_uuid` when one
    /// is given, and answers what was left once its descriptor was gone;
    /// `None` when there was no such index or a concurrent change to it won.
    ///
    /// Where deletes are conditional, the heads are read first and an
    /// intent is filed before anything is removed, so no crash leaves heads
    /// or bytes nothing names; the bytes of a shard with a writer are
    /// purged when the intent falls due, once that writer has stopped.
    ///
    /// # Errors
    ///
    /// [`MetaError`] from any step. One after the descriptor is gone ends the
    /// delete there, as in the Java: the intent's reclaim removes the heads,
    /// and purges again only when a shard had a writer or a head
    /// reappeared, so an alias's mention or a no-writer shard's bytes left
    /// by the failure stay.
    pub fn delete_index(
        &mut self,
        name: &str,
        expected_uuid: Option<&str>,
        now_millis: i64,
    ) -> Result<Option<Deleted>, MetaError> {
        let Some(read) = self.read(name)? else {
            return Ok(None);
        };
        if expected_uuid.is_some_and(|uuid| uuid != read.uuid) {
            return Ok(None);
        }
        let conditional = self.descriptors.deletes == DeleteMode::Conditional;
        let owned = if conditional {
            self.heads.read_all_of(name, read.shards, &read.uuid)?
        } else {
            // ALLOC: none; an empty Vec.
            Vec::new()
        };
        let had_writer = owned.iter().any(|head| head.owned);
        // A purge deferred to the reclaim is filed as a writer's.
        let had_writer = had_writer || (conditional && read.shards > self.inline_purge_max_shards);
        if conditional {
            let intent = ReclaimIntent {
                name: Cow::Borrowed(name),
                uuid: Cow::Borrowed(&read.uuid),
                shards: read.shards,
                had_writer,
            };
            self.queue.enqueue(&intent, now_millis)?;
        }
        if !self
            .descriptors
            .delete_if_unchanged(name, read.generation, now_millis)?
        {
            return Ok(None);
        }
        if conditional {
            self.heads.delete_if_unchanged(name, &owned)?;
        } else {
            self.heads.release_all_of(name, read.shards, &read.uuid)?;
        }
        let aliases_left = self.aliases.detach_from_aliases(name, &read.aliased_by)?;
        // Heads are read only where deletes are conditional, so a writer
        // implies a filed intent to purge from.
        let purged = if had_writer {
            None
        } else {
            let purged = self.purge(name, &read.uuid, read.shards)?;
            if conditional && self.repurge_pinned && purged.pinned > 0 {
                self.refile(name, &read.uuid, read.shards, now_millis)?;
            }
            Some(purged)
        };
        Ok(Some(Deleted {
            aliases_left,
            purged,
        }))
    }

    /// Finishes a delete whose intent fell due: nothing when the index is
    /// live under that uuid (its delete never happened); otherwise removes
    /// the uuid's heads, and purges its bytes again when a shard had a
    /// writer or a head reappeared, the sign a writer that missed the
    /// delete may have published since.
    ///
    /// # Errors
    ///
    /// [`MetaError`] when the store fails; the intent is kept and tried
    /// again.
    pub fn reclaim(&mut self, intent: &ReclaimIntent<'_>) -> Result<(), MetaError> {
        self.reclaim_inner(intent).map(|_| ())
    }

    /// As [`Self::reclaim`], and with [`Self::with_repurge_pinned`] on, files
    /// the intent again as of `now_millis` when a pin kept shards, so the
    /// next reclaim purges them.
    ///
    /// # Errors
    ///
    /// [`MetaError`] when the store fails; the intent is kept and tried
    /// again.
    pub fn reclaim_at(
        &mut self,
        intent: &ReclaimIntent<'_>,
        now_millis: i64,
    ) -> Result<(), MetaError> {
        let purged = self.reclaim_inner(intent)?;
        if self.repurge_pinned && purged.is_some_and(|p| p.pinned > 0) {
            self.refile(&intent.name, &intent.uuid, intent.shards, now_millis)?;
        }
        Ok(())
    }

    fn refile(
        &self,
        name: &str,
        uuid: &str,
        shards: u32,
        now_millis: i64,
    ) -> Result<(), MetaError> {
        self.queue.enqueue(
            &ReclaimIntent {
                name: Cow::Borrowed(name),
                uuid: Cow::Borrowed(uuid),
                shards,
                had_writer: true,
            },
            now_millis,
        )
    }

    fn reclaim_inner(&mut self, intent: &ReclaimIntent<'_>) -> Result<Option<Purged>, MetaError> {
        if self
            .read(&intent.name)?
            .is_some_and(|read| read.uuid == intent.uuid)
        {
            return Ok(None);
        }
        let reappeared = self
            .heads
            .delete_all_of(&intent.name, intent.shards, &intent.uuid)?;
        if intent.had_writer || reappeared > 0 {
            return self
                .purge(&intent.name, &intent.uuid, intent.shards)
                .map(Some);
        }
        Ok(None)
    }

    /// The index `name` names, copied out, or `None` for no index.
    fn read(&mut self, name: &str) -> Result<Option<Read>, MetaError> {
        let resolved = self.descriptors.resolve(name, &mut self.buf)?;
        let Target::Index(index) = resolved.target else {
            return Ok(None);
        };
        Ok(Some(Read {
            generation: resolved.generation,
            uuid: index.uuid.into_owned(),
            shards: index.number_of_shards,
            aliased_by: index
                .aliased_by
                .into_iter()
                .map(|alias| Cow::Owned(alias.into_owned()))
                // ALLOC: the index's alias hint, bounded by its record.
                .collect(),
        }))
    }

    fn purge(&self, name: &str, uuid: &str, shards: u32) -> Result<Purged, MetaError> {
        shards::purge(
            self.descriptors.store.as_ref(),
            &self.descriptors.base,
            self.pins,
            name,
            uuid,
            shards,
        )
    }
}
