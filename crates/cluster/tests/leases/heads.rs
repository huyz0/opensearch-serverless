//! Shard heads (docs/specs/shard-head/SPEC.md, AC-3 to AC-7).

use std::sync::Arc;

use oss_blob::{BlobStore, S3Op};
use oss_cluster::heads::{Acquisition, ShardHeadStore, shard_head_key};
use oss_cluster::{LeaseClock, LivenessOracle};

use crate::{BASE, Fleet, TTL, lease};

fn heads(fleet: &Fleet) -> ShardHeadStore {
    let store: Arc<dyn BlobStore> = fleet.store.clone();
    let clock: Arc<dyn LeaseClock> = fleet.clock.clone();
    let oracle = Arc::new(LivenessOracle::new(Arc::new(fleet.node())));
    ShardHeadStore::new(store, BASE, clock, TTL, oracle).unwrap()
}

#[test]
fn a_key_is_the_javas_shard_head_blob() {
    // From `RegisterMap.shardHeadBlob` over `Murmur3HashFunction`.
    for (index, shard, name) in [
        ("logs", 0, "00-logs#0"),
        ("logs", 3, "70-logs#3"),
        ("a", 12, "5b-a#12"),
        ("my-index", 7, "d2-my-index#7"),
    ] {
        assert_eq!(
            shard_head_key("", index, shard),
            format!("shards/{name}"),
            "{index}[{shard}]"
        );
    }
    assert_eq!(shard_head_key("d/", "logs", 3), "d/shards/70-logs#3");
}

#[test]
fn a_read_is_one_get_and_an_absent_head_is_none() {
    let fleet = Fleet::new();
    let heads = heads(&fleet);
    let before = fleet.total();
    assert_eq!(heads.read("logs", 0).unwrap(), None);
    assert_eq!(fleet.total() - before, 1);
    heads.acquire("logs", 0, "n1", "en1", None).unwrap();
    let before = fleet.total();
    let head = heads.read("logs", 0).unwrap().unwrap();
    assert_eq!(fleet.total() - before, 1);
    assert!(head.owned_by("n1", "en1"));
    assert_eq!(fleet.count(S3Op::List), 0);
}

#[test]
fn an_absent_head_is_created_at_term_one_with_one_create() {
    let fleet = Fleet::new();
    let heads = heads(&fleet);
    let before = fleet.total();
    let Acquisition::Won { head, taken_from } =
        heads.acquire("logs", 0, "n1", "en1", Some("u1")).unwrap()
    else {
        panic!("won");
    };
    assert_eq!((head.term, taken_from), (1, None));
    assert_eq!(head.index_uuid.as_deref(), Some("u1"));
    assert_eq!(head.lease_expires_at_millis, fleet.clock.now() + TTL);
    assert_eq!(fleet.total() - before, 2, "a read and a create");
}

#[test]
fn a_live_owner_keeps_the_shard_and_nothing_is_written() {
    let fleet = Fleet::new();
    fleet.node().renew(&lease("n1")).unwrap();
    let heads = heads(&fleet);
    heads.acquire("logs", 0, "n1", "en1", None).unwrap();
    // Past the head's own stamp, so the lease decides: n1's is live.
    fleet.clock.advance(TTL - 1);
    fleet.node().renew(&lease("n1")).unwrap();
    fleet.clock.advance(2);
    let (puts, gets) = (fleet.count(S3Op::Put), fleet.count(S3Op::Get));
    let Acquisition::HeldByAnother(head) = heads.acquire("logs", 0, "n2", "en2", None).unwrap()
    else {
        panic!("refused");
    };
    assert!(head.owned_by("n1", "en1"));
    assert_eq!(fleet.count(S3Op::Put), puts, "no write");
    let spent = fleet.count(S3Op::Get) - gets;
    // The head and the owner's lease; only an unowned head is read again.
    assert_eq!(spent, 2, "reads");
}

#[test]
fn a_gone_owners_shard_is_taken_at_the_next_term_naming_it_a_writer() {
    let fleet = Fleet::new();
    fleet.node().renew(&lease("n1")).unwrap();
    let heads = heads(&fleet);
    heads.acquire("logs", 0, "n1", "en1", None).unwrap();
    fleet.clock.advance(TTL * 2);
    let Acquisition::Won { head, taken_from } =
        heads.acquire("logs", 0, "n2", "en2", None).unwrap()
    else {
        panic!("won");
    };
    assert_eq!(head.term, 2);
    assert_eq!(taken_from.as_deref(), Some("n1"));
    assert_eq!(head.writers, ["n1"]);
    assert!(
        fleet.lease("n1").unwrap().revoked,
        "its lease was revoked first"
    );
}

#[test]
fn another_incarnation_is_taken_without_its_writers() {
    let fleet = Fleet::new();
    fleet.node().renew(&lease("n1")).unwrap();
    let heads = heads(&fleet);
    heads.acquire("logs", 0, "n1", "en1", Some("old")).unwrap();
    let Acquisition::Won { head, taken_from } =
        heads.acquire("logs", 0, "n2", "en2", Some("new")).unwrap()
    else {
        panic!("won");
    };
    assert_eq!((head.term, taken_from), (2, None));
    assert!(head.writers.is_empty());
    assert_eq!(head.index_uuid.as_deref(), Some("new"));
}

#[test]
fn one_of_two_racing_nodes_wins_an_absent_head() {
    let fleet = Fleet::new();
    let heads = Arc::new(heads(&fleet));
    let threads: Vec<_> = ["n1", "n2"]
        .into_iter()
        .map(|n| {
            let heads = Arc::clone(&heads);
            std::thread::spawn(move || heads.acquire("logs", 0, n, "e", None).unwrap())
        })
        .collect();
    let won = threads
        .into_iter()
        .map(|t| t.join().unwrap())
        .filter(|a| matches!(a, Acquisition::Won { .. }))
        .count();
    assert_eq!(won, 1);
}

#[test]
fn a_release_clears_the_owner_and_keeps_the_term_and_writers() {
    let fleet = Fleet::new();
    let heads = heads(&fleet);
    heads.acquire("logs", 0, "n1", "en1", Some("u1")).unwrap();
    let before = fleet.total();
    assert!(heads.release("logs", 0, "n1", "en1", 1, "u1").unwrap());
    assert_eq!(fleet.total() - before, 2, "one read and one swap");
    let head = heads.read("logs", 0).unwrap().unwrap();
    assert_eq!(head.owner_node_id, None);
    assert_eq!((head.term, head.index_uuid.as_deref()), (1, Some("u1")));
    assert!(head.writers.is_empty(), "no log to read: all published");
    // Taken at once, with no revocation, by anyone.
    let Acquisition::Won { head, taken_from } =
        heads.acquire("logs", 0, "n2", "en2", Some("u1")).unwrap()
    else {
        panic!("won");
    };
    assert_eq!((head.term, taken_from), (2, None));
}

#[test]
fn a_release_leaves_a_head_that_is_not_this_processes_at_that_term() {
    let fleet = Fleet::new();
    let heads = heads(&fleet);
    heads.acquire("logs", 0, "n1", "en1", Some("u1")).unwrap();
    let held = heads.read("logs", 0).unwrap().unwrap();
    for (node, eph, term, uuid) in [
        ("n2", "en1", 1, "u1"),
        ("n1", "en9", 1, "u1"),
        ("n1", "en1", 2, "u1"),
        ("n1", "en1", 1, "other"),
    ] {
        assert!(!heads.release("logs", 0, node, eph, term, uuid).unwrap());
        assert_eq!(heads.read("logs", 0).unwrap().unwrap(), held);
    }
    assert!(!heads.release("logs", 5, "n1", "en1", 1, "u1").unwrap());
}

#[test]
fn a_head_naming_a_node_alone_is_not_given_back() {
    let fleet = Fleet::new();
    let heads = heads(&fleet);
    let nameless = oss_format::ShardHead {
        index: "logs".to_owned(),
        shard: 0,
        term: 1,
        owner_node_id: Some("n1".to_owned()),
        owner_ephemeral_id: None,
        lease_expires_at_millis: i64::MAX,
        index_uuid: Some("u1".to_owned()),
        writers: Vec::new(),
    };
    fleet
        .store
        .create_register(
            &shard_head_key(BASE, "logs", 0),
            &nameless.to_bytes().unwrap(),
        )
        .unwrap();
    assert!(!heads.release("logs", 0, "n1", "en1", 1, "u1").unwrap());
    assert_eq!(heads.read("logs", 0).unwrap().unwrap(), nameless);
}

#[test]
fn a_take_that_loses_the_swap_to_a_release_wins_the_unowned_head_at_once() {
    let fleet = Fleet::new();
    let owner = heads(&fleet);
    owner.acquire("logs", 0, "n1", "en1", Some("u1")).unwrap();
    let racing = crate::Racing::new(fleet.store.clone());
    {
        // Between the take's read and its swap, the owner gives the shard back.
        let owner = heads(&fleet);
        let done = std::sync::atomic::AtomicBool::new(false);
        racing.set(move |_| {
            if !done.swap(true, std::sync::atomic::Ordering::SeqCst) {
                assert!(owner.release("logs", 0, "n1", "en1", 1, "u1").unwrap());
            }
        });
    }
    let store: Arc<dyn BlobStore> = racing;
    let clock: Arc<dyn LeaseClock> = fleet.clock.clone();
    let oracle = Arc::new(LivenessOracle::new(Arc::new(fleet.node())));
    let taker = ShardHeadStore::new(store, BASE, clock, TTL, oracle).unwrap();
    let Acquisition::Won { head, .. } = taker.acquire("logs", 0, "n1", "en1b", Some("u1")).unwrap()
    else {
        panic!("won, not refused by a head nobody owns");
    };
    assert_eq!(
        (head.term, head.owner_ephemeral_id.as_deref()),
        (2, Some("en1b"))
    );
}
