//! docs/specs/janitor-default-live: over the writer's own publishes and over a
//! takeover, no blob a commit names is ever reported or deleted by a sweep,
//! and a blob nothing names is.

// ARITH: small counts.
#![allow(clippy::arithmetic_side_effects)]
#![cfg(feature = "writer")]

mod proactive_support;

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_node::writer::Takeover;
use std::sync::atomic::AtomicUsize;

use proactive_support::*;

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

#[test]
fn a_shard_that_slept_and_woke_loses_only_what_no_commit_names() {
    let c = cluster();
    create(&c, "logs", "u1");
    let store: Arc<dyn BlobStore> = c.store.clone();
    let one = node(
        &c,
        "node-1",
        1,
        false,
        Some(Takeover::from_leases(store, "", "node-1", 1, 1 << 20)),
    );
    seq(&write(&one.role, "a"));
    one.role.refresh(&info(), 0).expect("published");
    assert_eq!(one.role.hibernate_idle(0, 0).hibernated, 1);
    let orphan = plant(&c);
    let pass = janitor(&c, &one.role);
    // Asleep the shard is not one the node holds, so no pass touches it.
    let before = keys(&c);
    for _ in 0..3 {
        let report = pass.run(gc_config(false));
        assert_eq!((report.swept, report.deleted), (0, 0), "{report:?}");
    }
    assert_eq!(keys(&c), before);
    assert!(before.contains(&orphan));
    // Woken by a write, published again, swept again.
    seq(&write(&one.role, "b"));
    one.role.refresh(&info(), 0).expect("published");
    let orphan = plant(&c);
    sweeps_delete_only_what_the_commit_does_not_name(&c, &pass, &orphan);
}

#[test]
fn a_sweep_between_a_write_and_its_publish_loses_nothing_the_commit_will_name() {
    let c = cluster();
    create(&c, "logs", "u1");
    let one = node(&c, "node-1", 1, false, None);
    seq(&write(&one.role, "a"));
    one.role.refresh(&info(), 0).expect("published");
    seq(&write(&one.role, "b"));
    // `b` is acknowledged, in the node log, and not in a commit yet.
    let orphan = plant(&c);
    let pass = janitor(&c, &one.role);
    sweeps_delete_only_what_the_commit_does_not_name(&c, &pass, &orphan);
    one.role.refresh(&info(), 0).expect("published");
    let orphan = plant(&c);
    sweeps_delete_only_what_the_commit_does_not_name(&c, &pass, &orphan);
    assert!(
        manifest(&c).contains("t=1/"),
        "the commit names the term-1 segments it published"
    );
}
