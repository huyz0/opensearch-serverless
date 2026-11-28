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

// docs/specs/reader-slow-start AC-1 and AC-6: one file both languages answer the same.
// `OSS_REGEN_RAMP_FIXTURE=1` rewrites it from this implementation; the Java reads a copy.
#[test]
fn the_ramp_fixture_is_what_this_implementation_answers() {
    use oss_cluster::placement::{admission, ramp_permille};
    let cfg = RampConfig {
        min_ms: 30_000,
        max_ms: 300_000,
    };
    let mut rows = Vec::new();
    for (since, said) in [
        (1_000i64, None),
        (1_000, Some(0)),
        (1_000, Some(200)),
        (1_000, Some(1000)),
    ] {
        for age in [
            0i64, 1, 5_000, 29_999, 30_000, 60_000, 150_000, 299_999, 300_000, 9_999_999,
        ] {
            let mut l = NodeLease::new("n", "e", "a", 0);
            l.search_since = Some(since);
            l.set_ramp_permille(said);
            let r = ramp_permille(&l, since + age, cfg);
            let said_text = said.map_or("-".to_owned(), |s| s.to_string());
            rows.push(format!("R {since} {said_text} {} {r}", since + age));
        }
    }
    for node in ["node-0", "node-7", "joiner", "né-ø"] {
        for shard in [0, 1, 7, 255] {
            rows.push(format!(
                "A {node} idx {shard} {}",
                admission(node, ShardKey::new("idx", shard))
            ));
        }
    }
    let mut fleet = fleet(8);
    let mut joiner = NodeLease::new("joiner", "e", "a", 0);
    joiner.set_roles([ROLE_SEARCH.to_owned()]);
    joiner.search_since = Some(1_000);
    joiner.set_ramp_permille(Some(200));
    fleet.push(joiner);
    for now in [1_000i64, 31_000, 91_000, 301_000] {
        for shard in 0..40 {
            let ids = readers_for_at(
                ShardKey::new("idx", shard),
                fleet.iter(),
                ROLE_SEARCH,
                3,
                cfg,
                now,
            );
            rows.push(format!("K idx {shard} {now} {}", ids.join(",")));
        }
    }
    let text = rows.join("\n") + "\n";
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/placement/ramp.txt");
    if std::env::var("OSS_REGEN_RAMP_FIXTURE").is_ok() {
        std::fs::write(&path, &text).unwrap();
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
}
