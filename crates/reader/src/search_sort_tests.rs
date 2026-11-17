//! Tests; compiled only under `#[cfg(test)]` from the parent module.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

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
    let spec = Spec::new("so", 20).segments_of(10).with_keywords(2);
    for (key, bytes) in blobs("", &spec) {
        store.put(&key, &mut SliceSource::new(&bytes)).expect("put");
    }
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("page cache"));
    open_shard(Arc::new(store), cache, "", "so", 0).expect("opens")
}

#[test]
fn a_doc_sort_reads_the_document_id_in_either_direction() {
    let shard = shard();
    for reverse in [false, true] {
        let (f, kind) = field_sort("_doc", reverse, &shard, "so").expect("sorts");
        assert_eq!(kind, KeyKind::Numeric);
        assert_eq!(f.reverse, reverse);
        assert_eq!(f.ty, SortType::Doc);
    }
}

#[test]
fn a_keyword_sorts_its_missing_values_last_in_either_direction() {
    let shard = shard();
    let (asc, kind) = field_sort("tag", false, &shard, "so").expect("sorts");
    assert_eq!((kind, asc.missing, asc.reverse), (KeyKind::Term, 1, false));
    let (desc, _) = field_sort("tag", true, &shard, "so").expect("sorts");
    assert_eq!((desc.missing, desc.reverse), (0, true));
}
