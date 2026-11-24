//! What a node tells a joining reader about the shards it holds: which are
//! open and how their pages are keyed in the page cache.

use std::sync::Arc;

use lucene_store::page_cache::PageCache;
use oss_blob::BlobStore;
use oss_transport::actions::tail::ShardRef;

use super::SearchService;
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
        let open = self.lock();
        // ALLOC: one entry per open shard, which the open_indexes pool bounds.
        open.values()
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
