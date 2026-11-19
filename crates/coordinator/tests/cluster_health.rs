//! docs/specs/coordinator-cluster-health: health is judged from what the
//! shard heads say.

// ARITH: test counts are small constants.
#![allow(clippy::arithmetic_side_effects)]

use oss_coordinator::cluster::health::{
    Counts, HeadRead, Health, IndexInput, NodeCondition, Params, ShardState, Status, compute,
    refuse,
};

fn cond(text: &str) -> Option<(String, i32)> {
    NodeCondition::parse(text).map(|c| (c.operator.to_owned(), c.count))
}

#[test]
fn node_conditions_parse_and_compare() {
    let cases = [
        ("3", "=", 3),
        (" 3 ", "=", 3),
        (">=2", ">=", 2),
        (">2", ">", 2),
        ("<=4", "<=", 4),
        ("<4", "<", 4),
        ("ge(5)", ">=", 5),
        ("gt( 6 )", ">", 6),
        ("le(7)", "<=", 7),
        ("lt(8)", "<", 8),
    ];
    for (text, op, n) in cases {
        assert_eq!(cond(text), Some((op.to_owned(), n)), "{text}");
    }
    for bad in ["", "x", ">=", "ge(5", "3.5", "=3"] {
        assert_eq!(cond(bad), None, "{bad}");
    }
    let at = |text: &str, nodes| {
        NodeCondition::parse(text)
            .expect("parses")
            .satisfied_by(nodes)
    };
    assert!(at("3", 3) && !at("3", 2));
    assert!(at(">=3", 3) && at(">=3", 4) && !at(">=3", 2));
    assert!(at(">3", 4) && !at(">3", 3));
    assert!(at("<=3", 3) && at("<=3", 2) && !at("<=3", 4));
    assert!(at("<3", 2) && !at("<3", 3));
}

fn params() -> Params<'static> {
    Params {
        cat: false,
        cat_unsupported: None,
        wait_for_status: None,
        wait_for_active_shards: None,
        wait_for_events: None,
        level: "cluster",
    }
}

#[test]
fn refusals_come_in_the_shells_order_and_words() {
    assert_eq!(refuse(&params()), None);
    let all = Params {
        cat: true,
        cat_unsupported: Some("no such cat option"),
        wait_for_status: Some("red"),
        wait_for_active_shards: Some("all"),
        wait_for_events: Some("urgent"),
        level: "nope",
    };
    assert_eq!(refuse(&all).as_deref(), Some("no such cat option"));
    let p = Params { cat: false, ..all };
    assert!(refuse(&p).expect("refused").starts_with(
        "wait_for_status must be 'green' or 'yellow'; 'red' is not a state to wait for"
    ));
    let p = Params {
        wait_for_status: Some("yellow"),
        ..p
    };
    assert!(
        refuse(&p)
            .expect("refused")
            .starts_with("wait_for_active_shards counts shards an allocator has placed")
    );
    let p = Params {
        wait_for_active_shards: None,
        ..p
    };
    assert!(
        refuse(&p)
            .expect("refused")
            .starts_with("wait_for_events waits on a cluster manager's pending-task queue")
    );
    let p = Params {
        wait_for_events: None,
        ..p
    };
    assert_eq!(
        refuse(&p).as_deref(),
        Some("level must be 'cluster', 'indices' or 'shards'")
    );
    for level in ["cluster", "indices", "shards"] {
        assert_eq!(refuse(&Params { level, ..p }), None);
    }
}

fn owned() -> HeadRead {
    HeadRead::Owned { held: true }
}

#[test]
fn unreadable_things_are_red_and_a_missing_index_ends_it() {
    let h = compute(
        Err(()),
        &[("a".into(), IndexInput::Shards(vec![(0, owned())]))],
        false,
    );
    assert_eq!((h.status, h.nodes), (Status::Red, 0));
    assert!(h.indices.is_empty());
    let h = compute(
        Ok(3),
        &[
            ("a".into(), IndexInput::Unreadable),
            ("gone".into(), IndexInput::Missing),
            ("b".into(), IndexInput::Shards(vec![(0, owned())])),
        ],
        false,
    );
    assert_eq!(h.missing.as_deref(), Some("gone"));
    assert_eq!(h.indices.len(), 1, "stopped at the missing index");
    assert_eq!(h.indices[0].1.status, Status::Red);
    assert_eq!(h.status, Status::Red);
}

#[test]
fn shards_are_owned_dormant_or_unservable() {
    let reads = vec![
        (0, owned()),
        (1, HeadRead::Unowned),
        (2, HeadRead::Owned { held: false }),
        (3, HeadRead::Unreadable),
    ];
    let h = compute(Ok(2), &[("a".into(), IndexInput::Shards(reads))], true);
    let (name, index) = &h.indices[0];
    assert_eq!(name, "a");
    assert_eq!(
        index.per_shard,
        [
            (0, ShardState::Owned),
            (1, ShardState::Dormant),
            (2, ShardState::Dormant),
            (3, ShardState::Unservable),
        ]
    );
    assert_eq!(index.status, Status::Red);
    assert_eq!(h.status, Status::Red);
}

#[test]
fn counts_add_up_and_per_shard_states_are_optional() {
    let a = IndexInput::Shards(vec![(0, owned()), (1, HeadRead::Unowned)]);
    let b = IndexInput::Shards(vec![
        (0, HeadRead::Unowned),
        (1, HeadRead::Unowned),
        (2, owned()),
    ]);
    let h = compute(Ok(4), &[("a".into(), a), ("b".into(), b)], false);
    assert_eq!(h.status, Status::Green);
    assert_eq!(h.nodes, 4);
    assert_eq!(
        h.indices[0].1.counts,
        Counts {
            shards: 2,
            active: 2,
            dormant: 1,
            unservable: 0
        }
    );
    assert_eq!(
        h.indices[1].1.counts,
        Counts {
            shards: 3,
            active: 3,
            dormant: 2,
            unservable: 0
        }
    );
    assert_eq!(
        (h.active_shards, h.dormant_shards, h.unservable_shards),
        (5, 3, 0)
    );
    assert!(h.indices[0].1.per_shard.is_empty());
}

fn health(status: Status, nodes: usize, missing: Option<&str>) -> Health {
    Health {
        status,
        nodes,
        missing: missing.map(str::to_owned),
        ..Health::default()
    }
}

#[test]
fn a_wait_is_satisfied_by_green_enough_nodes_or_a_missing_index() {
    let wait_nodes = NodeCondition::parse(">=3");
    let green = health(Status::Green, 3, None);
    assert!(green.satisfied(None, None));
    assert!(green.satisfied(Some("green"), wait_nodes.as_ref()));
    assert!(green.satisfied(Some("yellow"), None));
    assert!(!health(Status::Green, 2, None).satisfied(None, wait_nodes.as_ref()));
    assert!(!health(Status::Red, 3, None).satisfied(Some("green"), None));
    assert!(health(Status::Red, 3, None).satisfied(None, None));
    // A missing index ends the wait whatever else is asked.
    assert!(health(Status::Red, 0, Some("gone")).satisfied(Some("green"), wait_nodes.as_ref()));
}

#[test]
fn a_status_is_reported_as_its_word() {
    assert_eq!(Status::Green.as_str(), "green");
    assert_eq!(Status::Red.as_str(), "red");
    assert_eq!(Status::default(), Status::Green);
}
