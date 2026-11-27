//! The slow start in the ranking (docs/specs/reader-slow-start/SPEC.md, AC-3 to AC-5).

use oss_cluster::placement::{ROLE_SEARCH, RampConfig, ShardKey, readers_for, readers_for_at};
use oss_format::NodeLease;

const CFG: RampConfig = RampConfig {
    min_ms: 0,
    max_ms: 100_000,
};

fn fleet(n: usize) -> Vec<NodeLease> {
    (0..n)
        .map(|i| {
            let mut l = NodeLease::new(&format!("node-{i}"), "e", "a", 0);
            l.set_roles([ROLE_SEARCH.to_owned()]);
            l
        })
        .collect()
}

fn with_joiner(n: usize, since: i64) -> Vec<NodeLease> {
    let mut f = fleet(n);
    let mut j = NodeLease::new("joiner", "e", "a", 0);
    j.set_roles([ROLE_SEARCH.to_owned()]);
    j.search_since = Some(since);
    f.push(j);
    f
}

fn ranked(f: &[NodeLease], s: i32, now: i64, limit: usize) -> Vec<&str> {
    readers_for_at(
        ShardKey::new("idx", s),
        f.iter(),
        ROLE_SEARCH,
        limit,
        CFG,
        now,
    )
}

#[test]
fn off_ranks_exactly_as_before() {
    let f = with_joiner(12, 0);
    for s in 0..300 {
        let key = ShardKey::new("idx", s);
        let old = readers_for(key, f.iter(), ROLE_SEARCH, 3);
        let new = readers_for_at(key, f.iter(), ROLE_SEARCH, 3, RampConfig::OFF, 1);
        assert_eq!(old, new);
    }
}

#[test]
fn the_joiner_takes_its_share_in_proportion_and_never_gives_a_shard_back() {
    let f = with_joiner(12, 0);
    let first = |s, now| ranked(&f, s, now, 1)[0] == "joiner";
    let at = |now| (0..2000).filter(|s| first(*s, now)).count();
    let full = at(100_000);
    assert!(full > 100, "{full}");
    let half = at(50_000);
    let want = full / 2;
    assert!(half.abs_diff(want) * 20 <= 2000, "{half} vs {want}");
    for s in 0..2000 {
        if first(s, 25_000) {
            assert!(
                first(s, 50_000) && first(s, 100_000),
                "shard {s} moved back"
            );
        }
    }
    assert_eq!(at(0), 0);
}

#[test]
fn others_keep_their_order_while_the_joiner_ramps() {
    let f = with_joiner(12, 0);
    let base = fleet(12);
    for s in 0..500 {
        let others: Vec<&str> = ranked(&f, s, 30_000, 13)
            .into_iter()
            .filter(|i| *i != "joiner")
            .collect();
        let steady: Vec<&str> = ranked(&base, s, 0, 12);
        assert_eq!(others, steady, "shard {s}");
    }
}

#[test]
fn a_joiner_not_admitted_ranks_last_and_only_fills_a_long_limit() {
    let f = with_joiner(4, 0);
    for s in 0..200 {
        let short = ranked(&f, s, 0, 4);
        assert!(!short.contains(&"joiner"), "{short:?}");
        let long = ranked(&f, s, 0, 5);
        assert_eq!(long.last(), Some(&"joiner"));
    }
}
