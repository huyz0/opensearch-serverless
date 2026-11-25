//! docs/specs/writer-segment-merge AC-10: the janitor leaves a live term's
//! merged-away blobs alone, and after a takeover and a sweep a reader opens
//! the commit and counts every document.

// ARITH: test arithmetic on counts; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]
#![cfg(all(feature = "writer", feature = "reader"))]

mod proactive_support;

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use lucene_store::page_cache::{PageCache, PageCacheConfig};
use oss_blob::{BlobStore, PooledStore};
use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool};
use oss_node::writer::{MergeConfig, ProactiveTakeover};
use oss_reader::open_shard;
use proactive_support::*;

const DOCS: usize = 30;

fn count(c: &Cluster) -> i32 {
    let pools = BudgetConfig::new(8 << 20)
        .with_pool(Pool::Transport, 8 << 20)
        .expect("fits");
    let buffers =
        BufferPool::new(&MemoryBudget::new(&pools), Pool::Transport, 64 << 10, 16).expect("fits");
    let pooled = Arc::new(PooledStore::new(oss_blob::Shared::new(c.base()), buffers));
    let cache = Arc::new(PageCache::new(PageCacheConfig::default()).expect("cache"));
    open_shard(pooled, cache, "", "logs", 0)
        .expect("a reader opens the commit")
        .reader
        .num_docs()
}

/// A merging node-1 with `DOCS` published documents and one more acknowledged
/// and not published, and a node-2 that took the shard from its log.
fn merged_and_taken(c: &Cluster) -> (Node, Node) {
    create_n(c, "logs", "u1", 1);
    let one = node(c, "node-1", 1, false, None);
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
    for round in 0..DOCS {
        write_id(&one.role, &format!("doc-{round}"));
        one.role.refresh(&info, 0).expect("published");
    }
    write_id(&one.role, "tail");
    let calls = Arc::new(AtomicUsize::new(0));
    let take = takeover(c, &one, &calls);
    let predecessors = Arc::clone(&take.predecessors);
    let two = node(c, "node-2", 2, true, Some(take));
    let proactive =
        ProactiveTakeover::new("node-2", config(true), Arc::clone(&two.role), predecessors);
    assert_eq!(proactive.pass(&dead(&one)).taken, 1);
    (one, two)
}

// docs/specs/writer-segment-merge AC-10
#[test]
fn a_snapshot_or_view_holds_the_merged_away_blobs_through_a_takeover_and_a_sweep() {
    use oss_format::PointInTime;
    use oss_meta::PointInTimeStore;

    let c = cluster();
    let (_one, two) = merged_and_taken(&c);
    let store: Arc<dyn BlobStore> = c.store.clone();
    // A snapshot names the index; a view is being captured on it.
    c.store
        .put(
            "snapshots/s1",
            &mut oss_blob::SliceSource::new(br#"{"indices":{"logs":{"uuid":"u1"}}}"#),
            &mut [0u8; 16],
        )
        .expect("snapshot");
    let pits = PointInTimeStore::new(Arc::clone(&store), "", Arc::new(|| 1_000)).expect("pits");
    pits.create(
        &PointInTime {
            id: "AAAAAAAAAAAAAAAAAAAAAA".to_owned(),
            index: "logs".to_owned(),
            index_uuid: Some("u1".to_owned()),
            expires_at_millis: 9_000_000,
            capturing: true,
            shards: Vec::new(),
        },
        &mut vec![0u8; 1 << 16],
    )
    .expect("view");
    let before = keys(&c);
    let pass = janitor(&c, &two.role);
    pass.run(gc_config(false));
    pass.run(gc_config(false));
    assert_eq!(
        keys(&c),
        before,
        "nothing a snapshot or a view holds was swept"
    );
    assert_eq!(count(&c), DOCS as i32 + 1);
}

// docs/specs/writer-segment-merge AC-10
#[test]
fn a_sweep_never_takes_a_blob_the_head_names_and_the_live_term_keeps_the_rest() {
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
    for round in 0..DOCS {
        write_id(&one.role, &format!("doc-{round}"));
        one.role.refresh(&info, 0).expect("published");
    }
    assert!(one.role.merges_started() > 0, "merging ran");
    assert_eq!(count(&c), DOCS as i32);

    // The live term: blobs merged away are still there, and a sweep keeps them.
    let before = keys(&c);
    let named = manifest(&c);
    assert!(
        before.iter().any(|k| !named.contains(recorded(k))),
        "a merged-away blob is in the container"
    );
    let pass = janitor(&c, &one.role);
    pass.run(gc_config(false));
    pass.run(gc_config(false));
    assert_eq!(keys(&c), before, "the live term lost no blob");

    // One more acknowledged and not published, so the log names the shard.
    write_id(&one.role, "tail");
    // A taker, then a sweep that may take what the head does not name.
    let calls = Arc::new(AtomicUsize::new(0));
    let take = takeover(&c, &one, &calls);
    let predecessors = Arc::clone(&take.predecessors);
    let two = node(&c, "node-2", 2, true, Some(take));
    let proactive =
        ProactiveTakeover::new("node-2", config(true), Arc::clone(&two.role), predecessors);
    assert_eq!(proactive.pass(&dead(&one)).taken, 1);
    let pass = janitor(&c, &two.role);
    pass.run(gc_config(false));
    let live = pass.run(gc_config(false));
    assert!(live.failed.is_empty(), "{:?}", live.failed);
    let named = manifest(&c);
    for kept in keys(&c) {
        assert!(c.store.head(&kept).expect("heads").is_some());
    }
    assert_eq!(
        count(&c),
        DOCS as i32 + 1,
        "the commit still opens and counts every document"
    );
    assert!(!named.is_empty());
}
