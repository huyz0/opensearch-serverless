//! A live sweep, and the evidence it deletes only what no commit names.

#![allow(dead_code)]

use std::collections::BTreeSet;

use oss_blob::{BlobStore, SliceSource};
use oss_janitor::gc::Config as GcConfig;
use oss_node::writer::JanitorPass;

use super::Cluster;
pub const SHARD: &str = "segments/logs#u1#0/";

pub fn gc_config(dry_run: bool) -> GcConfig {
    GcConfig {
        dry_run,
        minimum_unreferenced_millis: 0,
        max_watched: 1000,
        max_pinned: 1000,
        relist_after_millis: 0,
    }
}

/// Every key under the shard's container.
pub fn keys(c: &Cluster) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut after: Option<String> = None;
    loop {
        let mut page = Vec::new();
        // LIST: a test comparing a shard's container before and after a sweep.
        let done = c
            .store
            .list_prefix(SHARD, after.as_deref(), 1000, &mut |k, _| {
                page.push(k.to_owned());
            })
            .expect("lists");
        after = page.last().cloned();
        out.extend(page);
        if !done.truncated {
            return out;
        }
    }
}

/// The commit register's value as text.
pub fn manifest(c: &Cluster) -> String {
    let mut buf = vec![0u8; 1 << 16];
    let read = c
        .store
        .read_register(&format!("{SHARD}manifest"), &mut buf)
        .expect("reads")
        .expect("there");
    String::from_utf8_lossy(&buf[..read.len]).into_owned()
}

/// The part of a blob's key the manifest records: `t=<term>/<blob>`.
pub fn recorded(key: &str) -> &str {
    key.strip_prefix(SHARD).expect("under the shard")
}

pub fn plant(c: &Cluster) -> String {
    let key = format!("{SHARD}t=0/orphan");
    c.store
        .put(&key, &mut SliceSource::new(b"old"), &mut [0u8; 16])
        .expect("planted");
    key
}

/// Sweeps dry, then live, and holds the evidence: the dry sweep would delete
/// what the live one does, and every blob deleted is one the commit does not name.
pub fn sweeps_delete_only_what_the_commit_does_not_name(
    c: &Cluster,
    pass: &JanitorPass,
    orphan: &str,
) {
    let before = keys(c);
    assert!(before.contains(orphan));
    pass.run(gc_config(true));
    let dry = pass.run(gc_config(true));
    assert_eq!(dry.deleted, 0, "a dry run deletes nothing");
    assert_eq!(keys(c), before, "a dry run changes nothing");
    pass.run(gc_config(false));
    let live = pass.run(gc_config(false));
    assert!(live.failed.is_empty(), "{:?}", live.failed);
    let after = keys(c);
    let named = manifest(c);
    for gone in before.difference(&after) {
        assert!(
            !named.contains(recorded(gone)),
            "{gone} was deleted and the commit names it: {named}"
        );
    }
    assert!(!after.contains(orphan), "the planted blob is swept");
    for kept in &after {
        assert!(c.store.head(kept).expect("heads").is_some());
    }
}
