//! Bucket heads (docs/specs/bucket-heads/SPEC.md).

use std::collections::HashMap;
use std::sync::Arc;

use oss_blob::{BlobStore, S3Op};
use oss_cluster::heads::{
    Acquisition, BucketHeads, Member, ShardHeadStore, bucket_head_index, shard_head_key,
};
use oss_cluster::{ClusterError, LeaseClock, LivenessOracle};

use crate::{BASE, Fleet, TTL, lease};

/// `index` is a member of `bucket`, under uuid `<index>-uuid`; others are not.
struct Map(HashMap<&'static str, &'static str>);

impl BucketHeads for Map {
    fn bucket_of(
        &self,
        index: &str,
        _shard: u32,
        index_uuid: Option<&str>,
    ) -> Result<Option<Member>, ClusterError> {
        let Some(bucket) = self.0.get(index) else {
            return Ok(None);
        };
        let uuid = format!("{index}-uuid");
        if index_uuid.is_some_and(|u| u != uuid) {
            return Ok(None);
        }
        Member::new(bucket, &uuid).map(Some)
    }
}

fn map(pairs: &[(&'static str, &'static str)]) -> Arc<Map> {
    Arc::new(Map(pairs.iter().copied().collect()))
}

fn plain(fleet: &Fleet) -> ShardHeadStore {
    let store: Arc<dyn BlobStore> = fleet.store.clone();
    let clock: Arc<dyn LeaseClock> = fleet.clock.clone();
    let oracle = Arc::new(LivenessOracle::new(Arc::new(fleet.node())));
    ShardHeadStore::new(store, BASE, clock, TTL, oracle).unwrap()
}

fn bucketed(fleet: &Fleet, pairs: &[(&'static str, &'static str)]) -> ShardHeadStore {
    plain(fleet).with_bucket_heads(map(pairs))
}

fn won(a: Acquisition) -> (oss_format::ShardHead, Option<String>) {
    match a {
        Acquisition::Won { head, taken_from } => (head, taken_from),
        Acquisition::HeldByAnother(h) => panic!("refused: {h:?}"),
    }
}

#[test]
fn without_bucket_heads_a_shard_has_its_own_head() {
    for routed in [false, true] {
        let fleet = Fleet::new();
        let heads = if routed {
            // a map that names no member of this index
            bucketed(&fleet, &[("other", "s00-0")])
        } else {
            plain(&fleet)
        };
        let (head, _) = won(heads.acquire("logs", 0, "n1", "e1", Some("u")).unwrap());
        assert_eq!((head.index.as_str(), head.shard, head.term), ("logs", 0, 1));
        assert!(heads.read("logs", 0).unwrap().is_some());
        // the own head is where it always was, and no bucket head was made
        let own = shard_head_key(BASE, "logs", 0);
        let mut buf = vec![0; 4096];
        assert!(fleet.store.read_register(&own, &mut buf).unwrap().is_some());
        let bucket = shard_head_key(BASE, &bucket_head_index("s00-0"), 0);
        assert!(
            fleet
                .store
                .read_register(&bucket, &mut buf)
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn a_member_read_is_one_get_of_the_bucket_head_shown_as_the_members() {
    let fleet = Fleet::new();
    let heads = bucketed(&fleet, &[("a", "s02-37"), ("b", "s02-37")]);
    let before = fleet.total();
    assert_eq!(heads.read("a", 3).unwrap(), None);
    assert_eq!(fleet.total() - before, 1);
    heads.acquire("a", 3, "n1", "e1", None).unwrap();
    let before = fleet.total();
    let head = heads.read("b", 5).unwrap().unwrap();
    assert_eq!(fleet.total() - before, 1);
    assert_eq!((head.index.as_str(), head.shard, head.term), ("b", 5, 1));
    assert_eq!(head.index_uuid.as_deref(), Some("b-uuid"));
    assert!(head.owned_by("n1", "e1"));
    assert_eq!(fleet.count(S3Op::List), 0);
}

#[test]
fn the_first_member_creates_the_bucket_head() {
    let fleet = Fleet::new();
    let heads = bucketed(&fleet, &[("a", "s02-37")]);
    let before = fleet.total();
    let (head, taken_from) = won(heads.acquire("a", 1, "n1", "e1", Some("a-uuid")).unwrap());
    assert_eq!(fleet.total() - before, 2, "a read and a create");
    assert_eq!(
        (head.index.as_str(), head.shard, head.term, taken_from),
        ("a", 1, 1, None)
    );
    assert_eq!(head.index_uuid.as_deref(), Some("a-uuid"));
    // what is stored is the bucket's, with no member's identity
    let key = shard_head_key(BASE, "#bucket#s02-37", 0);
    let mut buf = vec![0; 4096];
    let read = fleet.store.read_register(&key, &mut buf).unwrap().unwrap();
    let stored = oss_format::ShardHead::parse(&buf[..read.len]).unwrap();
    assert_eq!((stored.index.as_str(), stored.shard), ("#bucket#s02-37", 0));
    assert_eq!(stored.index_uuid, None);
    assert_eq!(stored.term, 1);
}

#[test]
fn a_second_member_of_a_held_bucket_joins_with_no_write() {
    let fleet = Fleet::new();
    let heads = bucketed(&fleet, &[("a", "s02-37"), ("b", "s02-37")]);
    won(heads.acquire("a", 0, "n1", "e1", None).unwrap());
    let puts = fleet.count(S3Op::Put);
    let before = fleet.total();
    let (head, taken_from) = won(heads.acquire("b", 0, "n1", "e1", Some("b-uuid")).unwrap());
    assert_eq!(fleet.total() - before, 1, "one GET");
    assert_eq!(fleet.count(S3Op::Put), puts, "no write");
    assert_eq!((head.index.as_str(), head.term, taken_from), ("b", 1, None));
    assert_eq!(head.index_uuid.as_deref(), Some("b-uuid"));
    // another incarnation's uuid is not a member: its own head
    let (own, _) = won(heads.acquire("b", 1, "n1", "e1", Some("other")).unwrap());
    assert_eq!(own.index_uuid.as_deref(), Some("other"));
}

#[test]
fn a_member_of_a_bucket_this_node_holds_without_an_incarnation_joins() {
    let fleet = Fleet::new();
    let heads = bucketed(&fleet, &[("a", "s02-37")]);
    won(heads.acquire("a", 0, "n1", "e1", None).unwrap());
    // a head with no incarnation recorded is joined by the same node id
    let key = shard_head_key(BASE, "#bucket#s02-37", 0);
    let mut buf = vec![0; 4096];
    let read = fleet.store.read_register(&key, &mut buf).unwrap().unwrap();
    let mut stored = oss_format::ShardHead::parse(&buf[..read.len]).unwrap();
    stored.owner_ephemeral_id = None;
    fleet
        .store
        .swap_register(&key, read.generation, &stored.to_bytes().unwrap())
        .unwrap();
    let puts = fleet.count(S3Op::Put);
    let (head, _) = won(heads
        .acquire("a", 0, "n1", "another-process", None)
        .unwrap());
    assert_eq!(head.term, 1);
    assert_eq!(fleet.count(S3Op::Put), puts);
}

#[test]
fn a_live_buckets_member_is_refused() {
    let fleet = Fleet::new();
    fleet.node().renew(&lease("n1")).unwrap();
    let heads = bucketed(&fleet, &[("a", "s02-37"), ("b", "s02-37")]);
    won(heads.acquire("a", 0, "n1", "en1", None).unwrap());
    fleet.clock.advance(TTL - 1);
    fleet.node().renew(&lease("n1")).unwrap();
    fleet.clock.advance(2);
    let puts = fleet.count(S3Op::Put);
    let Acquisition::HeldByAnother(head) = heads.acquire("b", 2, "n2", "en2", None).unwrap() else {
        panic!("refused");
    };
    assert!(head.owned_by("n1", "en1"));
    assert_eq!((head.index.as_str(), head.shard), ("b", 2));
    assert_eq!(fleet.count(S3Op::Put), puts);
}

#[test]
fn a_gone_owners_bucket_is_taken_at_the_next_term() {
    let fleet = Fleet::new();
    fleet.node().renew(&lease("n1")).unwrap();
    let heads = bucketed(&fleet, &[("a", "s02-37"), ("b", "s02-37")]);
    won(heads.acquire("a", 0, "n1", "en1", None).unwrap());
    fleet.clock.advance(TTL * 2);
    let (head, taken_from) = won(heads.acquire("b", 0, "n2", "en2", None).unwrap());
    assert_eq!(head.term, 2);
    assert_eq!(taken_from.as_deref(), Some("n1"));
    assert_eq!(head.writers, ["n1"]);
    assert!(fleet.lease("n1").unwrap().revoked);
    // the whole bucket moved: the other member now reads the new owner
    assert!(heads.read("a", 0).unwrap().unwrap().owned_by("n2", "en2"));
}

#[test]
fn sixteen_members_at_once_are_one_create() {
    let fleet = Fleet::new();
    let names: Vec<&'static str> = (0..16)
        .map(|i| &*Box::leak(format!("m{i}").into_boxed_str()))
        .collect();
    let pairs: Vec<(&'static str, &'static str)> = names.iter().map(|n| (*n, "s02-37")).collect();
    let heads = Arc::new(bucketed(&fleet, &pairs));
    let before = fleet.count(S3Op::Put);
    let threads: Vec<_> = names
        .iter()
        .map(|n| {
            let heads = Arc::clone(&heads);
            let n = *n;
            std::thread::spawn(move || heads.acquire(n, 0, "n1", "e1", None).unwrap())
        })
        .collect();
    for t in threads {
        let (head, _) = won(t.join().unwrap());
        assert_eq!(head.term, 1);
    }
    assert_eq!(
        fleet.count(S3Op::Put) - before,
        1,
        "one create, no lost race"
    );
}

#[test]
fn one_of_two_racing_nodes_wins_an_absent_bucket_head() {
    let fleet = Fleet::new();
    // two nodes, each its own store over the one bucket
    let a = Arc::new(bucketed(&fleet, &[("x", "s05-1")]));
    let b = Arc::new(bucketed(&fleet, &[("x", "s05-1")]));
    let threads: Vec<_> = [("n1", a), ("n2", b)]
        .into_iter()
        .map(|(n, heads)| std::thread::spawn(move || heads.acquire("x", 0, n, "e", None).unwrap()))
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    let winners = results
        .iter()
        .filter(|a| matches!(a, Acquisition::Won { .. }))
        .count();
    assert_eq!(winners, 1);
    for r in &results {
        if let Acquisition::HeldByAnother(head) = r {
            assert_eq!(head.index, "x");
        }
    }
}

#[test]
fn a_member_names_a_valid_bucket_id() {
    assert!(Member::new("s02-37", "u").is_ok());
    for bad in ["", "S02-1", "a/b", &"x".repeat(65)] {
        assert!(Member::new(bad, "u").is_err(), "{bad:?}");
    }
    assert!(Member::new(&"x".repeat(64), "u").is_ok());
    let m = Member::new("s02-37", "u").unwrap();
    assert_eq!((m.bucket_id(), m.index_uuid()), ("s02-37", "u"));
}
