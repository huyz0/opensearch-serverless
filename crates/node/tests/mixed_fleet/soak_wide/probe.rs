//! What a failed soak prints of the cluster: heads, leases and the published commits.

use super::INDEX;

/// Prints, for each shard of `INDEX`, the head's owner and its claim, and the
/// lease of the owner and of each node the head names, as Java reads them.
pub(super) fn probe(store: &oss_blob::S3Store, shards: u32) {
    use oss_blob::BlobStore;
    let read = |key: &str| {
        let mut buf = vec![0u8; 1 << 16];
        let got = store.read_register(key, &mut buf).ok().flatten()?;
        buf.get(..got.len).map(<[u8]>::to_vec)
    };
    let lease = |id: &str| -> String {
        let key = oss_format::lease_key("", id).expect("a key");
        match read(&key).and_then(|b| oss_format::NodeLease::parse(&b).ok()) {
            Some(l) => format!(
                "lease {id}: ephemeral={} expires={} revoked={} version={:?} epochs={:?}",
                l.ephemeral_id, l.expires_at_millis, l.revoked, l.version, l.node_log
            ),
            None => format!("lease {id}: none"),
        }
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    eprintln!("WIDE probe now={now}");
    for shard in 0..shards {
        let key = oss_cluster::heads::shard_head_key("", INDEX, shard);
        let Some(head) = read(&key).and_then(|b| oss_format::ShardHead::parse(&b).ok()) else {
            eprintln!("WIDE probe shard {shard}: no head");
            continue;
        };
        eprintln!(
            "WIDE probe shard {shard}: term={} owner={:?} ephemeral={:?} claim_expires={} writers={:?}",
            head.term,
            head.owner_node_id,
            head.owner_ephemeral_id,
            head.lease_expires_at_millis,
            head.writers
        );
        let mut ids: Vec<String> = head.writers.clone();
        if let Some(owner) = head.owner_node_id.as_ref().filter(|o| !ids.contains(o)) {
            ids.push(owner.clone());
        }
        for id in ids {
            eprintln!("WIDE probe   {}", lease(&id));
        }
    }
}

/// Opens each shard's published commit as a reader does and prints what it
/// counts.
pub(super) fn probe_commits(store: oss_blob::S3Store, shards: u32) {
    use lucene_store::page_cache::{PageCache, PageCacheConfig};
    use oss_mem::{BudgetConfig, BufferPool, MemoryBudget, Pool};
    let pools = BudgetConfig::new(8 << 20)
        .with_pool(Pool::Transport, 8 << 20)
        .expect("fits");
    let buffers =
        BufferPool::new(&MemoryBudget::new(&pools), Pool::Transport, 64 << 10, 16).expect("fits");
    let pooled = std::sync::Arc::new(oss_blob::PooledStore::new(store, buffers));
    for shard in 0..shards {
        let cache = std::sync::Arc::new(PageCache::new(PageCacheConfig::default()).expect("cache"));
        match oss_reader::open_shard(std::sync::Arc::clone(&pooled), cache, "", INDEX, shard) {
            Ok(open) => eprintln!(
                "WIDE commit shard {shard}: {open:?} segments={}",
                open.reader.segment_readers().len()
            ),
            Err(e) => eprintln!("WIDE commit shard {shard}: {e}"),
        }
    }
}
