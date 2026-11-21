//! Tests; compiled only under `#[cfg(test)]` from the parent module.

use std::sync::Arc;

use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{Emulator, PooledStore, SliceSource};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool, SMALL_BUFFER};
use oss_testkit::index::{Spec, blobs};

use super::*;
use crate::{OpenShard, open_shard};

fn shard() -> OpenShard<Emulator> {
    let config = BudgetConfig::new(4 << 20)
        .with_pool(Pool::Transport, 4 << 20)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let buffers = BufferPool::new(&budget, Pool::Transport, SMALL_BUFFER, 8).expect("fits");
    let store = PooledStore::new(Emulator::new(64 << 20, 1), buffers);
    for (key, bytes) in blobs("", &Spec::new("hm", 10).segments_of(10)) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", "hm", 0).expect("opens")
}

#[test]
fn a_document_without_the_doc_values_writes_nothing_however_it_is_asked() {
    let shard = shard();
    let readers = shard.reader.segment_readers();
    for meta in [
        HitMeta::default(),
        HitMeta {
            version: true,
            seq_no: false,
        },
        HitMeta {
            version: false,
            seq_no: true,
        },
        HitMeta {
            version: true,
            seq_no: true,
        },
    ] {
        let mut out = Vec::new();
        write_meta(&mut out, meta, &readers[0], 0).expect("writes");
        assert!(
            out.is_empty(),
            "{meta:?}: {}",
            String::from_utf8_lossy(&out)
        );
    }
}
