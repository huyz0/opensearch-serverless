//! docs/specs/proactive-takeover: a node takes, before a write asks, the
//! shards a dead node's log names.

mod proactive_rig;

use proactive_rig::*;

use std::sync::Arc;
use std::time::Duration;

use oss_node::writer::{ProactiveConfig, ProactiveTakeover, Takeover};
use oss_writer::bulk::ItemResult;
use oss_writer::takeover::{HeadWriters, Predecessors};

// docs/specs/proactive-takeover AC-6
#[test]
fn every_named_shard_is_taken_whatever_the_concurrency() {
    let mut reports = Vec::new();
    for max_concurrent in [1, 3, 4] {
        let c = cluster();
        let one = four_shards(&c);
        let (pass, two) = pass_over(
            &c,
            &one,
            ProactiveConfig {
                max_concurrent,
                ..config(true)
            },
        );
        let report = pass.pass(&dead(&one));
        assert!(report.named >= 2, "{report:?}");
        assert_eq!(report.taken, report.named, "{report:?}");
        assert_eq!((report.skipped, report.deferred), (0, 0), "{report:?}");
        assert_eq!(two.role.held_shards().len(), report.named);
        reports.push(report);
    }
    assert_eq!(reports[0], reports[1]);
    assert_eq!(reports[1], reports[2]);
}

// docs/specs/proactive-takeover AC-7
#[test]
fn a_pass_past_its_deadline_starts_no_take_and_stays_due() {
    let c = cluster();
    let one = four_shards(&c);
    let (pass, two) = pass_over(
        &c,
        &one,
        ProactiveConfig {
            pass: Duration::ZERO,
            ..config(true)
        },
    );
    let lease = dead(&one);
    let report = pass.pass(&lease);
    assert_eq!((report.taken, report.skipped), (0, 0), "{report:?}");
    assert_eq!(report.deferred, report.named, "{report:?}");
    assert!(report.named >= 2);
    assert!(two.role.held_shards().is_empty());
    // Still due, and a write takes the shard the lazy way.
    let r = refreshed(&["node-2"], lease);
    assert_eq!(pass.due(&r, i64::MAX).len(), 1);
    assert!(matches!(write(&two.role, "d0"), ItemResult::Written { .. }));
}

// docs/specs/proactive-takeover AC-13
#[test]
fn a_shard_of_an_index_deleted_and_recreated_is_skipped() {
    let c = cluster();
    let one = four_shards(&c);
    c.descriptors.delete("logs").expect("deleted");
    create_shards(&c, "logs", "u2", 4);
    let (pass, two) = pass_over(&c, &one, config(true));
    let report = pass.pass(&dead(&one));
    assert!(report.named >= 2, "{report:?}");
    assert_eq!(report.skipped, report.named, "{report:?}");
    assert_eq!((report.taken, report.deferred), (0, 0), "{report:?}");
    assert!(two.role.held_shards().is_empty());
}

// docs/specs/proactive-takeover AC-8: a log that cannot be taken is not
// tried at every refresh.
#[test]
fn a_pass_that_failed_waits_a_stagger_and_then_twice_as_long() {
    let c = cluster();
    let one = first_writer(&c);
    let broken = Takeover {
        predecessors: Arc::new(Predecessors::new(Box::new(|_| {
            Err(oss_writer::log::TakeoverError::Lease(Arc::new(
                std::io::Error::other("no lease"),
            )))
        }))),
        heads: HeadWriters::new(c.store.clone(), ""),
        max_buffer_bytes: 1 << 20,
    };
    let predecessors = Arc::clone(&broken.predecessors);
    let two = node(&c, "node-2", 2, true, Some(broken));
    let pass = ProactiveTakeover::new("node-2", config(true), Arc::clone(&two.role), predecessors);
    let lease = dead(&one);
    let r = refreshed(&["node-2"], lease.clone());
    let stagger = 10_000;
    pass.pass_at(&lease, 100_000);
    assert!(
        pass.due(&r, 100_000 + stagger - 1).is_empty(),
        "waits a stagger"
    );
    assert_eq!(pass.due(&r, 100_000 + stagger).len(), 1);
    pass.pass_at(&lease, 200_000);
    assert!(
        pass.due(&r, 200_000 + 2 * stagger - 1).is_empty(),
        "then twice as long"
    );
    assert_eq!(pass.due(&r, 200_000 + 2 * stagger).len(), 1);
    // The wait stops doubling at 64 staggers.
    for i in 0..10 {
        pass.pass_at(&lease, 300_000 + i);
    }
    let last = 300_009;
    assert!(pass.due(&r, last + 64 * stagger - 1).is_empty());
    assert_eq!(pass.due(&r, last + 64 * stagger).len(), 1);
}
