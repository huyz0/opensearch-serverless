//! docs/specs/proactive-takeover: a node takes, before a write asks, the
//! shards a dead node's log names.

#[path = "proactive_support/mod.rs"]
mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use oss_cluster::placement::reapers_for;
use oss_node::writer::{ProactiveTakeover, Takeover};
use oss_writer::takeover::{HeadWriters, Predecessors};
use support::*;

#[test]
fn a_pass_takes_the_shard_the_dead_log_names_before_any_write() {
    let c = cluster();
    let one = first_writer(&c);
    let calls = Arc::new(AtomicUsize::new(0));
    let take = takeover(&c, &one, &calls);
    let predecessors = Arc::clone(&take.predecessors);
    let two = node(&c, "node-2", 2, true, Some(take));
    let pass = ProactiveTakeover::new("node-2", config(true), Arc::clone(&two.role), predecessors);
    let report = pass.pass(&dead(&one));
    assert_eq!(
        (report.named, report.taken, report.deferred),
        (1, 1, 0),
        "{report:?}"
    );
    assert_eq!(two.role.held_shards().len(), 1);
    assert_eq!(registered_term(&c), 2, "b was published under the new term");
    // The first write finds the shard held: no second take, no second log read.
    assert_eq!(seq(&write(&two.role, "c")), 2);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn a_dead_log_that_cannot_be_read_takes_nothing_and_stays_due() {
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
    assert_eq!(pass.pass(&lease).taken, 0);
    assert!(two.role.held_shards().is_empty());
    let r = refreshed(&["node-2"], lease);
    assert_eq!(pass.due(&r, i64::MAX).len(), 1, "not recorded as finished");
}

#[test]
fn only_the_reapers_act_each_after_its_turn_and_a_finished_pass_is_not_repeated() {
    let c = cluster();
    let one = first_writer(&c);
    let calls = Arc::new(AtomicUsize::new(0));
    let take = takeover(&c, &one, &calls);
    let predecessors = Arc::clone(&take.predecessors);
    let two = node(&c, "node-2", 2, true, Some(take));
    let ids = ["n1", "n2", "n3", "n4"];
    let lease = dead(&one);
    let r = refreshed(&ids, lease.clone());
    let reapers = reapers_for("node-1", r.live.iter(), 2);
    assert_eq!(reapers.len(), 2);
    let of = |id: &str, enabled: bool| {
        ProactiveTakeover::new(
            id,
            config(enabled),
            Arc::clone(&two.role),
            Arc::clone(&predecessors),
        )
    };
    let (first, second) = (of(reapers[0], true), of(reapers[1], true));
    let other = ids.iter().find(|id| !reapers.contains(id)).expect("one");
    // The lease ran out at 1,000 and the stagger is 10,000.
    assert_eq!(first.due(&r, 1_000).len(), 1, "the first acts at once");
    assert!(
        second.due(&r, 10_999).is_empty(),
        "the second waits its turn"
    );
    assert_eq!(second.due(&r, 11_000).len(), 1);
    assert!(of(other, true).due(&r, i64::MAX).is_empty(), "not a reaper");
    assert!(
        of(reapers[0], false).due(&r, i64::MAX).is_empty(),
        "switched off"
    );
    // A finished pass is not started again for the same epochs, and a new
    // epoch set is.
    let done = ProactiveTakeover::new(
        reapers[0],
        config(true),
        Arc::clone(&two.role),
        Arc::clone(&predecessors),
    );
    assert_eq!(done.pass(&lease).deferred, 0);
    assert!(done.due(&r, i64::MAX).is_empty());
    let mut later = lease;
    later.node_log.push(oss_format::NodeLogEpoch {
        epoch: 99,
        trim: 0,
        landed: 0,
    });
    assert_eq!(done.due(&refreshed(&ids, later), i64::MAX).len(), 1);
}

// docs/specs/proactive-takeover AC-21
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

// docs/specs/proactive-takeover AC-10 and AC-14
#[test]
fn a_dead_node_whose_lease_names_no_epochs_is_not_due_and_a_pass_makes_no_request() {
    let c = cluster();
    let one = first_writer(&c);
    let calls = Arc::new(AtomicUsize::new(0));
    let take = takeover(&c, &one, &calls);
    let predecessors = Arc::clone(&take.predecessors);
    let two = node(&c, "node-2", 2, true, Some(take));
    let mut lease = dead(&one);
    lease.node_log.clear();
    let ids = ["node-2"];
    let r = refreshed(&ids, lease);
    let pass = ProactiveTakeover::new("node-2", config(true), Arc::clone(&two.role), predecessors);
    assert!(pass.due(&r, i64::MAX).is_empty(), "nothing to take");
    assert_eq!(calls.load(Ordering::SeqCst), 0, "no lease read for it");
    assert!(two.role.held_shards().is_empty());
}

// docs/specs/proactive-takeover AC-22
#[test]
fn reapers_are_chosen_only_among_live_nodes_of_this_build_when_asked() {
    let c = cluster();
    let one = first_writer(&c);
    let calls = Arc::new(AtomicUsize::new(0));
    let take = takeover(&c, &one, &calls);
    let predecessors = Arc::clone(&take.predecessors);
    let two = node(&c, "node-2", 2, true, Some(take));
    let ids = ["n1", "n2", "n3", "n4", "n5", "n6"];
    let mut r = refreshed(&ids, dead(&one));
    let mut live = r.live.to_vec();
    for lease in &mut live {
        // Only n5 and n6 are of this build; the rest are another build's.
        lease.version = match lease.node_id.as_str() {
            "n5" | "n6" => Some(format!("{}0.1.0", oss_node::read_forward::BUILD_PREFIX)),
            "n1" => None,
            _ => Some("3.0.0".to_owned()),
        };
    }
    r.live = live.into();
    let of = |id: &str, only: bool| {
        let mut cfg = config(true);
        cfg.rust_reapers_only = only;
        ProactiveTakeover::new(id, cfg, Arc::clone(&two.role), Arc::clone(&predecessors))
    };
    for id in ids {
        let acts = !of(id, true).due(&r, i64::MAX).is_empty();
        assert_eq!(acts, id == "n5" || id == "n6", "{id} with the switch on");
    }
    let off: Vec<_> = ids
        .iter()
        .filter(|id| !of(id, false).due(&r, i64::MAX).is_empty())
        .collect();
    assert_eq!(off.len(), 2, "the switch off keeps the old choice: {off:?}");
}

// docs/specs/proactive-takeover AC-22
#[test]
fn with_no_live_node_of_this_build_nobody_is_a_reaper_and_the_next_waits_its_turn() {
    let c = cluster();
    let one = first_writer(&c);
    let calls = Arc::new(AtomicUsize::new(0));
    let take = takeover(&c, &one, &calls);
    let predecessors = Arc::clone(&take.predecessors);
    let two = node(&c, "node-2", 2, true, Some(take));
    let ids = ["n1", "n2", "n3"];
    let mut r = refreshed(&ids, dead(&one));
    let mut live = r.live.to_vec();
    for lease in &mut live {
        lease.version = Some("3.0.0".to_owned());
    }
    r.live = live.clone().into();
    let of = |id: &str| {
        let mut cfg = config(true);
        cfg.rust_reapers_only = true;
        ProactiveTakeover::new(id, cfg, Arc::clone(&two.role), Arc::clone(&predecessors))
    };
    for id in ids {
        assert!(of(id).due(&r, i64::MAX).is_empty(), "{id}");
    }
    // Two of this build: the second waits one stagger past the first.
    for lease in &mut live {
        lease.version = Some(format!("{}0.1.0", oss_node::read_forward::BUILD_PREFIX));
    }
    r.live = live.into();
    let ranked = reapers_for("node-1", r.live.iter(), 2);
    assert_eq!(of(ranked[0]).due(&r, 1_000).len(), 1);
    assert!(of(ranked[1]).due(&r, 10_999).is_empty());
    assert_eq!(of(ranked[1]).due(&r, 11_000).len(), 1);
}
