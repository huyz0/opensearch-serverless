//! What a node tells a joining reader about the shards it holds: which are
//! open and how their pages are keyed in the page cache.

use std::sync::Arc;

use lucene_store::page_cache::PageCache;
use oss_blob::BlobStore;
use oss_transport::actions::tail::ShardRef;

use super::SearchService;
use crate::warm_copy::Shards;
use crate::warm_expect::Expected;
use crate::warm_handlers::HeldShards;
use crate::warm_peer::HeldShard;

impl<S: BlobStore + 'static> SearchService<S> {
    /// The page cache this service reads through.
    #[must_use]
    pub fn page_cache(&self) -> &Arc<PageCache> {
        &self.cache
    }

    /// The shards open now, each with what its files' cache keys start
    /// with. A shard is listed while it is open, held by a search or not.
    #[must_use]
    pub fn held_shards(&self) -> Vec<HeldShard> {
        // The lock every search takes is held only to copy the open shards'
        // handles; the names are built after it is released.
        // ALLOC: one handle per open shard, which the open_indexes pool bounds.
        let open: Vec<_> = self.lock().values().map(Arc::clone).collect();
        // ALLOC: one entry per open shard, which the open_indexes pool bounds.
        open.iter()
            .filter(|held| crate::warm_setting::index_wants_warm(&held.shard.meta.settings))
            .map(|held| HeldShard {
                shard: ShardRef {
                    index_name: held.index.clone(),
                    index_uuid: held.shard.meta.uuid.clone(),
                    shard: i32::try_from(held.shard_no).unwrap_or(i32::MAX),
                },
                prefix: format!("{}/{}/{}/t=", self.base, held.index, held.shard_no),
            })
            .collect()
    }
}

impl<S: BlobStore + 'static> HeldShards for Arc<SearchService<S>> {
    fn held(&self) -> Vec<HeldShard> {
        self.held_shards()
    }
}

impl<S: BlobStore + 'static> Shards for Arc<SearchService<S>> {
    /// Opens the shard, which reads its commit as a search would, and says
    /// which pages of it a peer may fill.
    fn expected(&self, shard: &ShardRef) -> Option<Arc<Expected>> {
        let number = u32::try_from(shard.shard).ok()?;
        self.open(&shard.index_name, number).ok()?;
        let held = self
            .lock()
            .values()
            .find(|h| h.index == shard.index_name && h.shard_no == number)
            .map(Arc::clone)?;
        let files = held.shard.directory.store().files();
        let namespace = format!(
            "{}/{}/{}/t={}",
            self.base, shard.index_name, number, files.term
        );
        Some(Arc::new(Expected::new(
            &namespace,
            files,
            self.cache.config().page_size,
            &|name| self.cache.is_pinned_name(name),
        )))
    }
}
