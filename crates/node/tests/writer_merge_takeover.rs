//! docs/specs/writer-segment-merge: a node taking a shard whose writer merged
//! its segments finds every document that writer acknowledged.

// ARITH: test arithmetic on counts; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]
#![cfg(feature = "writer")]

mod proactive_support;

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::PooledStore;
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool};
use oss_node::writer::{MergeConfig, ProactiveTakeover};
use oss_reader::open_shard;
use proactive_support::*;

// docs/specs/writer-segment-merge AC-6
#[test]
fn a_taker_finds_every_document_a_merging_writer_acknowledged() {
    let c = cluster();
    create_n(&c, "logs", "u1", 1);
    let one = node(&c, "node-1", 1, false, None);
    let mut merge = MergeConfig::off();
    merge.enabled = true;
    merge.segments_per_tier = 2;
    merge.max_merge_at_once = 4;
    let budget = MemoryBudget::new(
        &BudgetConfig::new(256 << 20)
            .with_pool(Pool::OpenIndexes, 128 << 20)
            .expect("fits"),
    );
    one.role.set_merging(&merge, &budget);
    let info = info_n(1);
    let mut ids = Vec::new();
    for round in 0..40 {
        let id = format!("doc-{round}");
        write_id(&one.role, &id);
        ids.push(id);
        // The last five are acknowledged and not yet refreshed.
        if round < 35 {
            one.role.refresh(&info, 0).expect("published");
        }
    }
    assert!(one.role.merges_started() > 0, "merging ran");
    let pools = BudgetConfig::new(8 << 20)
        .with_pool(Pool::Transport, 8 << 20)
        .expect("fits");
    let buffers =
        BufferPool::new(&MemoryBudget::new(&pools), Pool::Transport, 64 << 10, 16).expect("fits");
    let pooled = Arc::new(PooledStore::new(oss_blob::Shared::new(c.base()), buffers));
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("cache"));
    let open =
        open_shard(pooled, cache, "", "logs", 0).expect("a reader opens the published commit");
    eprintln!("published commit: {open:?}");
    assert_eq!(
        open.reader.num_docs(),
        35,
        "a searcher counts the refreshed documents"
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let take = takeover(&c, &one, &calls);
    let predecessors = Arc::clone(&take.predecessors);
    let two = node(&c, "node-2", 2, true, Some(take));
    let pass = ProactiveTakeover::new("node-2", config(true), Arc::clone(&two.role), predecessors);
    assert_eq!(pass.pass(&dead(&one)).taken, 1);
    for id in &ids {
        let got = two.role.get(&info, 0, id, true).expect("reads");
        assert!(got.is_some(), "{id} lost across the takeover");
    }
}
