//! Which writer takes a bucket nobody holds.

use oss_cluster::routing::murmur3_x64_h1;
use oss_format::NodeLease;
use oss_writer::bucket::{
    BucketPlacement, DEFAULT_HOLD_DOWN_MILLIS, KEEP_RANKS, LOAD_BOUND, MAX_HOLD_DOWN_MILLIS,
};

const FAR: i64 = i64::MAX / 4;

fn writer(node: &str, ephemeral: &str) -> NodeLease {
    let mut lease = NodeLease::new(node, ephemeral, "10.0.0.1:9300", FAR);
    lease.set_roles(["ingest".to_owned()]);
    lease
}

fn is_writer(lease: &NodeLease) -> bool {
    lease.roles().iter().any(|r| r == "ingest")
}

fn score(bucket: &str, node: &str) -> i64 {
    murmur3_x64_h1(format!("{bucket}\0{node}").as_bytes()) as i64
}

fn fleet(n: usize) -> Vec<NodeLease> {
    (0..n)
        .map(|i| writer(&format!("node-{i}"), &format!("e{i}")))
        .collect()
}

#[test]
fn the_best_score_ranks_first_and_ties_break_by_node_id() {
    let placement = BucketPlacement::new(DEFAULT_HOLD_DOWN_MILLIS);
    let leases = fleet(6);
    for bucket in ["s00-0", "s07-3", "s15-12"] {
        let ranked = placement.rank(bucket, &leases, 1_000, is_writer);
        let mut expected: Vec<&str> = leases.iter().map(|l| l.node_id.as_str()).collect();
        expected.sort_by(|a, b| score(bucket, b).cmp(&score(bucket, a)).then(a.cmp(b)));
        assert_eq!(ranked, expected, "{bucket}");
        assert_eq!(
            placement
                .owner_for(bucket, &leases, 1_000, is_writer)
                .as_deref(),
            Some(expected[0])
        );
    }
}

#[test]
fn expired_revoked_and_non_writer_leases_are_not_ranked() {
    let placement = BucketPlacement::new(DEFAULT_HOLD_DOWN_MILLIS);
    let mut leases = fleet(4);
    leases[0].expires_at_millis = 500;
    leases[1].revoked = true;
    leases[2].set_roles(["search".to_owned()]);
    let ranked = placement.rank("s01-0", &leases, 1_000, is_writer);
    assert_eq!(ranked, ["node-3"]);
    assert_eq!(placement.rank("s01-0", &[], 1_000, is_writer).len(), 0);
    assert!(
        placement
            .owner_for("s01-0", &leases[..3], 1_000, is_writer)
            .is_none()
    );
}

#[test]
fn a_node_seen_later_waits_out_its_hold_down() {
    let placement = BucketPlacement::new(1_000);
    let mut leases = fleet(2);
    placement.observe(&leases, 0);
    leases.push(writer("late", "l1"));
    let at = |now| placement.rank("s02-0", &leases, now, is_writer);
    placement.observe(&leases, 10_000);
    assert!(!at(10_000).contains(&"late".to_owned()));
    assert!(!at(10_999).contains(&"late".to_owned()));
    assert!(at(11_000).contains(&"late".to_owned()));
    // alone, a node still in its hold-down is ranked
    let alone = BucketPlacement::new(1_000);
    alone.observe(&[], 0);
    alone.observe(&[writer("a", "e")], 5);
    assert_eq!(
        alone.rank("s02-0", &[writer("a", "e")], 6, is_writer),
        ["a"]
    );
}

#[test]
fn the_first_view_settles_everyone_in_it() {
    let placement = BucketPlacement::new(DEFAULT_HOLD_DOWN_MILLIS);
    let leases = fleet(3);
    assert_eq!(placement.rank("s03-0", &leases, 0, is_writer).len(), 3);
    // settled at once, not after the hold-down: a node seen soon after waits
    // while they are ranked
    let mut more = leases.clone();
    more.push(writer("late", "l"));
    let ranked = placement.rank("s03-0", &more, 100, is_writer);
    assert_eq!(ranked.len(), 3);
    assert!(!ranked.contains(&"late".to_owned()));
}

#[test]
fn a_new_incarnation_at_exactly_twice_the_hold_down_is_not_flapping() {
    let placement = BucketPlacement::new(1_000);
    let stable = writer("stable", "s");
    placement.observe(&[stable.clone(), writer("n", "e0")], 0);
    // first seen under e0 at the first view; e1 comes exactly twice the hold-down on
    placement.observe(&[stable.clone(), writer("n", "e1")], 5_000);
    // e1 at 5000 was a long time after: base hold-down. Now e2 exactly 2 * 1000 later.
    let leases = [stable.clone(), writer("n", "e2")];
    placement.observe(&leases, 7_000);
    assert!(
        !placement
            .rank("b", &leases, 7_999, is_writer)
            .contains(&"n".to_owned())
    );
    assert!(
        placement
            .rank("b", &leases, 8_000, is_writer)
            .contains(&"n".to_owned())
    );
}

#[test]
fn a_flapping_node_doubles_its_hold_down_to_the_ceiling() {
    let base = 1_000;
    let placement = BucketPlacement::new(base);
    let stable = writer("stable", "s");
    placement.observe(std::slice::from_ref(&stable), 0);
    let mut hold = base;
    let mut now = 10_000;
    placement.observe(&[stable.clone(), writer("flap", "f0")], now);
    for incarnation in 1..12 {
        // back under a new incarnation within twice the hold-down
        now += hold;
        let flap = writer("flap", &format!("f{incarnation}"));
        placement.observe(&[stable.clone(), flap.clone()], now);
        hold = (hold * 2).min(MAX_HOLD_DOWN_MILLIS);
        let leases = [stable.clone(), flap];
        let ranked = |at| placement.rank("s04-0", &leases, at, is_writer);
        assert!(!ranked(now + hold - 1).contains(&"flap".to_owned()));
        assert!(ranked(now + hold).contains(&"flap".to_owned()));
    }
    assert_eq!(hold, MAX_HOLD_DOWN_MILLIS);
    // long after, a new incarnation gets only the base hold-down
    now += 10 * MAX_HOLD_DOWN_MILLIS;
    let calm = writer("flap", "calm");
    placement.observe(&[stable.clone(), calm.clone()], now);
    let leases = [stable, calm];
    assert!(
        !placement
            .rank("s04-0", &leases, now + base - 1, is_writer)
            .contains(&"flap".to_owned())
    );
    assert!(
        placement
            .rank("s04-0", &leases, now + base, is_writer)
            .contains(&"flap".to_owned())
    );
}

#[test]
fn the_same_incarnation_and_a_missing_one_change_nothing() {
    let placement = BucketPlacement::new(1_000);
    let a = writer("a", "e1");
    let b = writer("b", "e2");
    placement.observe(&[a.clone(), b.clone()], 0);
    placement.observe(&[a.clone(), b.clone()], 50);
    let unknown = writer("b", "");
    placement.observe(&[a.clone(), unknown.clone()], 60);
    assert_eq!(
        placement.rank("s05-0", &[a, unknown], 70, is_writer).len(),
        2
    );
}

#[test]
fn an_overloaded_writer_ranks_last() {
    let placement = BucketPlacement::new(DEFAULT_HOLD_DOWN_MILLIS);
    let mut leases = fleet(5);
    for lease in &mut leases {
        lease.held = 100;
    }
    // mean 120, bound 150: 151 is over, 150 is not
    leases[0].held = 200;
    let mean = |ls: &[NodeLease]| f64::from(ls.iter().map(|l| l.held).sum::<i32>()) / 5.0;
    assert!(f64::from(leases[0].held) > LOAD_BOUND * mean(&leases));
    for bucket in ["s06-0", "s06-1", "s06-2", "s06-3", "s06-4", "s06-5"] {
        let ranked = placement.rank(bucket, &leases, 0, is_writer);
        assert_eq!(
            ranked.last().map(String::as_str),
            Some("node-0"),
            "{bucket}"
        );
    }
    // at the bound exactly it keeps its score position
    let mut level = fleet(2);
    level[0].held = 100;
    level[1].held = 100;
    let ranked = placement.rank("s06-0", &level, 0, is_writer);
    let best = if score("s06-0", "node-0") > score("s06-0", "node-1") {
        "node-0"
    } else {
        "node-1"
    };
    assert_eq!(ranked[0], best);
    // a load exactly at the bound is not over it
    let mut at_bound = fleet(2);
    at_bound[0].held = 60;
    at_bound[1].held = 100;
    for i in 0..16 {
        let bucket = format!("s09-{i}");
        let mut expected = vec!["node-0", "node-1"];
        expected.sort_by_key(|n| std::cmp::Reverse(score(&bucket, n)));
        assert_eq!(placement.rank(&bucket, &at_bound, 0, is_writer), expected);
    }
    // leases reporting no load are not in the mean, and nobody reports: no overload
    let mut mixed = fleet(3);
    mixed[0].held = 200;
    mixed[1].held = -1;
    mixed[2].held = -1;
    assert_eq!(placement.rank("s06-0", &mixed, 0, is_writer).len(), 3);
    assert_eq!(
        placement.rank("s06-0", &mixed, 0, is_writer),
        placement.rank("s06-0", &fleet(3), 0, is_writer)
    );
}

#[test]
fn a_joining_writer_takes_about_one_bucket_in_n() {
    // each view is the first this placement has, so every node is settled
    let placement = BucketPlacement::new(DEFAULT_HOLD_DOWN_MILLIS);
    let joined = BucketPlacement::new(DEFAULT_HOLD_DOWN_MILLIS);
    let before = fleet(9);
    let after = fleet(10);
    let mut moved = 0;
    for i in 0..2_000 {
        let bucket = format!("s{:02}-{}", i % 16, i / 16);
        let was = placement.owner_for(&bucket, &before, 0, is_writer).unwrap();
        let now = joined.owner_for(&bucket, &after, 0, is_writer).unwrap();
        if was != now {
            assert_eq!(now, "node-9", "{bucket} moved between old writers");
            moved += 1;
        }
    }
    // one in ten, within a wide margin
    assert!((100..300).contains(&moved), "{moved}");
}

#[test]
fn keeps_holds_for_the_top_two() {
    let placement = BucketPlacement::new(DEFAULT_HOLD_DOWN_MILLIS);
    let leases = fleet(5);
    let ranked = placement.rank("s08-0", &leases, 0, is_writer);
    assert_eq!(KEEP_RANKS, 2);
    for (at, node) in ranked.iter().enumerate() {
        assert_eq!(
            placement.keeps("s08-0", node, &leases, 0, is_writer),
            at < 2,
            "{node}"
        );
    }
    assert!(!placement.keeps("s08-0", "ghost", &leases, 0, is_writer));
}
