//! docs/specs/janitor-default-live: over the writer's own publishes and over a
//! takeover, no blob a commit names is ever reported or deleted by a sweep,
//! and a blob nothing names is.

// ARITH: small counts.
#![allow(clippy::arithmetic_side_effects)]
#![cfg(feature = "writer")]

mod proactive_support;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use oss_blob::{BlobStore, SliceSource};
use oss_janitor::gc::Config;
use oss_node::writer::JanitorPass;
use proactive_support::*;

const SHARD: &str = "segments/logs#u1#0/";

fn config(dry_run: bool) -> Config {
    Config {
        dry_run,
        minimum_unreferenced_millis: 0,
        max_watched: 1000,
        max_pinned: 1000,
        relist_after_millis: 0,
    }
}

/// Every key under the shard's container.
fn keys(c: &Cluster) -> BTreeSet<String> {
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
fn manifest(c: &Cluster) -> String {
    let mut buf = vec![0u8; 1 << 16];
    let read = c
        .store
        .read_register(&format!("{SHARD}manifest"), &mut buf)
        .expect("reads")
        .expect("there");
    String::from_utf8_lossy(&buf[..read.len]).into_owned()
}

/// The part of a blob's key the manifest records: `t=<term>/<blob>`.
fn recorded(key: &str) -> &str {
    key.strip_prefix(SHARD).expect("under the shard")
}

fn plant(c: &Cluster) -> String {
    let key = format!("{SHARD}t=0/orphan");
    c.store
        .put(&key, &mut SliceSource::new(b"old"), &mut [0u8; 16])
        .expect("planted");
    key
}

/// Sweeps dry, then live, and holds the evidence: the dry sweep would delete
/// what the live one does, and every blob deleted is one the commit does not name.
fn sweeps_delete_only_what_the_commit_does_not_name(c: &Cluster, pass: &JanitorPass, orphan: &str) {
    let before = keys(c);
    assert!(before.contains(orphan));
    pass.run(config(true));
    let dry = pass.run(config(true));
    assert_eq!(dry.deleted, 0, "a dry run deletes nothing");
    assert_eq!(keys(c), before, "a dry run changes nothing");
    pass.run(config(false));
    let live = pass.run(config(false));
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

#[test]
fn a_writer_that_publishes_again_and_again_loses_only_what_no_commit_names() {
    let c = cluster();
    create(&c, "logs", "u1");
    let one = node(&c, "node-1", 1, false, None);
    for i in 0..4 {
        seq(&write(&one.role, &format!("d{i}")));
        one.role.refresh(&info(), 0).expect("published");
    }
    let orphan = plant(&c);
    let pass = janitor(&c, &one.role);
    sweeps_delete_only_what_the_commit_does_not_name(&c, &pass, &orphan);
}

#[test]
fn a_successor_after_a_takeover_loses_only_what_no_commit_names() {
    let c = cluster();
    let one = first_writer(&c);
    let calls = Arc::new(AtomicUsize::new(0));
    let two = node(&c, "node-2", 2, true, Some(takeover(&c, &one, &calls)));
    seq(&write(&two.role, "c"));
    two.role.refresh(&info(), 0).expect("published");
    seq(&write(&two.role, "d"));
    two.role.refresh(&info(), 0).expect("published again");
    let orphan = plant(&c);
    let pass = janitor(&c, &two.role);
    sweeps_delete_only_what_the_commit_does_not_name(&c, &pass, &orphan);
    // The risk is a live blob of an older term: the first writer's segment
    // is still named by the successor's commit, and still there.
    let kept: Vec<String> = keys(&c)
        .into_iter()
        .filter(|k| recorded(k).starts_with("t=1/") && manifest(&c).contains(recorded(k)))
        .collect();
    assert!(!kept.is_empty(), "a term-1 blob the term-2 commit names");
}
