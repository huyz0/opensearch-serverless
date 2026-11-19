//! docs/specs/coordinator-placement: where a request's documents belong and
//! what a failed forward means.

// ARITH: test counters and digits are small constants.
#![allow(clippy::arithmetic_side_effects)]

use std::collections::HashMap;
use std::sync::Mutex;

use oss_coordinator::route::place::{
    Failure, ForwardFailure, Outcome, PlaceError, Placement, Plane, Refusal, Router,
};

/// An in-memory plane that counts what it is asked.
struct Fake {
    /// index -> (uuid, shards)
    indices: HashMap<String, (String, u32)>,
    hints: Mutex<HashMap<(String, u32), String>>,
    heads: Mutex<HashMap<(String, u32), String>>,
    system: Vec<String>,
    local_shard: Mutex<Vec<(String, u32)>>,
    described: Mutex<u32>,
    head_reads: Mutex<u32>,
    doubts: Mutex<Vec<(String, u32)>>,
    forgets: Mutex<Vec<(String, u32)>>,
}

impl Fake {
    fn new() -> Self {
        let mut indices = HashMap::new();
        indices.insert("logs".to_owned(), ("u1".to_owned(), 3));
        Self {
            indices,
            hints: Mutex::default(),
            heads: Mutex::default(),
            system: vec![".sec".to_owned()],
            local_shard: Mutex::default(),
            described: Mutex::default(),
            head_reads: Mutex::default(),
            doubts: Mutex::default(),
            forgets: Mutex::default(),
        }
    }

    fn head(&self, shard: u32, owner: &str) {
        self.heads
            .lock()
            .expect("lock")
            .insert(("logs".to_owned(), shard), owner.to_owned());
    }

    fn hint(&self, shard: u32, owner: &str) {
        self.hints
            .lock()
            .expect("lock")
            .insert(("logs".to_owned(), shard), owner.to_owned());
    }
}

impl Plane for Fake {
    type Desc = (String, u32);

    fn is_system_index(&self, name: &str) -> bool {
        self.system.iter().any(|s| s == name)
    }

    fn describe(&self, index: &str) -> Result<Option<Self::Desc>, PlaceError> {
        *self.described.lock().expect("lock") += 1;
        Ok(self.indices.get(index).cloned())
    }

    fn uuid(&self, desc: &Self::Desc) -> String {
        desc.0.clone()
    }

    fn shard_for(&self, desc: &Self::Desc, id: &str, _routing: Option<&str>) -> u32 {
        // The last digit of the id picks the shard.
        id.bytes()
            .last()
            .map_or(0, |b| u32::from(b - b'0') % desc.1)
    }

    fn owner_hint(&self, index: &str, shard: u32) -> Option<String> {
        self.hints
            .lock()
            .expect("lock")
            .get(&(index.to_owned(), shard))
            .cloned()
    }

    fn read_head(&self, index: &str, shard: u32) -> Result<Option<String>, PlaceError> {
        *self.head_reads.lock().expect("lock") += 1;
        Ok(self
            .heads
            .lock()
            .expect("lock")
            .get(&(index.to_owned(), shard))
            .cloned())
    }

    fn forget_owner(&self, index: &str, shard: u32) {
        self.hints
            .lock()
            .expect("lock")
            .remove(&(index.to_owned(), shard));
        self.forgets
            .lock()
            .expect("lock")
            .push((index.to_owned(), shard));
    }

    fn holds_shard(&self, index: &str, _uuid: &str, shard: u32) -> bool {
        self.local_shard
            .lock()
            .expect("lock")
            .contains(&(index.to_owned(), shard))
    }

    fn local_node_id(&self) -> &str {
        "me"
    }

    fn ownership_doubted(&self, index: &str, shard: u32) {
        self.doubts
            .lock()
            .expect("lock")
            .push((index.to_owned(), shard));
    }
}

fn failure(kind: ForwardFailure, message: &str) -> Failure {
    Failure {
        kind,
        message: message.to_owned(),
        client_status: false,
        owner_unreachable: false,
    }
}

fn stale(shard: u32, owner: &str) -> Placement {
    Placement {
        local: false,
        shard,
        owner: Some(owner.to_owned()),
        uuid: "u1".to_owned(),
    }
}

fn not_here(outcome: Outcome) -> (String, Option<String>, bool) {
    match outcome {
        Outcome::Refuse(Refusal::NotHere {
            message,
            owner,
            retryable,
        }) => (message, owner, retryable),
        other => panic!("not a not-here refusal: {other:?}"),
    }
}

#[test]
fn a_system_index_is_refused_before_it_is_read_and_a_missing_one_is_not_found() {
    let plane = Fake::new();
    let router = Router::new(&plane, true);
    assert_eq!(
        router.place(".sec", "x1", None),
        Err(Refusal::SystemIndex(".sec".to_owned()))
    );
    assert_eq!(*plane.described.lock().expect("lock"), 0, "not read");
    let err = router.place("nope", "x1", None).expect_err("refused");
    assert_eq!(err, Refusal::NoSuchIndex("nope".to_owned()));
    assert_eq!(err.to_string(), "no such index: nope");
    // Not refused when the caller does not refuse system indices.
    let open = Router::new(&plane, false);
    assert_eq!(
        open.place(".sec", "x1", None),
        Err(Refusal::NoSuchIndex(".sec".to_owned()))
    );
}

#[test]
fn a_placement_has_the_shard_the_uuid_the_local_flag_and_the_owner_read_once() {
    let plane = Fake::new();
    plane.head(1, "n1");
    plane.hint(2, "n2");
    plane.head(2, "stale-head");
    plane
        .local_shard
        .lock()
        .expect("lock")
        .push(("logs".to_owned(), 1));
    let router = Router::new(&plane, true);
    let a = router.place("logs", "d1", None).expect("placed");
    assert_eq!(
        a,
        Placement {
            local: true,
            shard: 1,
            owner: Some("n1".to_owned()),
            uuid: "u1".to_owned()
        }
    );
    let b = router.place("logs", "d2", None).expect("placed");
    assert_eq!(b.owner.as_deref(), Some("n2"), "the hint, not the head");
    assert!(!b.local);
    let c = router.place("logs", "d4", None).expect("placed");
    assert_eq!((c.shard, c.owner), (1, Some("n1".to_owned())));
    router.place("logs", "d1", None).expect("placed");
    assert_eq!(*plane.described.lock().expect("lock"), 1, "one descriptor");
    assert_eq!(
        *plane.head_reads.lock().expect("lock"),
        1,
        "one head, shard 1"
    );
    let none = router.place("logs", "d0", None).expect("placed");
    assert_eq!(none.owner, None, "no head and no hint");
}

#[test]
fn a_failure_forgets_the_hint_and_the_memo_first() {
    let plane = Fake::new();
    plane.hint(1, "n1");
    let router = Router::new(&plane, true);
    router.place("logs", "d1", None).expect("placed");
    plane.head(1, "n3");
    let out = router.after_forward_failure(
        "logs",
        &stale(1, "n1"),
        &failure(ForwardFailure::NotOwner, "no"),
        true,
    );
    assert!(
        plane
            .forgets
            .lock()
            .expect("lock")
            .contains(&("logs".to_owned(), 1))
    );
    assert!(matches!(out, Outcome::Forward(_)));
    // The memo now holds the fresh owner, so a placement does not read again.
    let reads = *plane.head_reads.lock().expect("lock");
    let again = router.place("logs", "d1", None).expect("placed");
    assert_eq!(again.owner.as_deref(), Some("n3"));
    assert_eq!(*plane.head_reads.lock().expect("lock"), reads);
}

#[test]
fn a_lost_condition_and_an_owners_client_answer_are_rethrown_and_a_server_one_is_not_here() {
    let plane = Fake::new();
    let router = Router::new(&plane, true);
    let conflict = failure(ForwardFailure::Conflict, "version conflict");
    assert_eq!(
        router.after_forward_failure("logs", &stale(1, "n1"), &conflict, true),
        Outcome::Refuse(Refusal::Conflict(conflict.clone()))
    );
    for kind in [ForwardFailure::Rejected, ForwardFailure::Remote] {
        let client = Failure {
            client_status: true,
            ..failure(kind, "429")
        };
        assert_eq!(
            router.after_forward_failure("logs", &stale(1, "n1"), &client, true),
            Outcome::Refuse(Refusal::Answer(client.clone()))
        );
        let server = failure(kind, "boom");
        let (message, owner, retryable) =
            not_here(router.after_forward_failure("logs", &stale(1, "n1"), &server, true));
        assert_eq!(
            message,
            "could not forward to n1, which the shard-head named as owner: boom"
        );
        assert_eq!(owner.as_deref(), Some("n1"));
        assert!(retryable);
    }
    assert_eq!(*plane.head_reads.lock().expect("lock"), 0, "no head read");
}

#[test]
fn a_deadline_or_a_dropped_connection_is_never_retried_and_doubts_ownership() {
    let plane = Fake::new();
    let router = Router::new(&plane, true);
    let (message, owner, retryable) = not_here(router.after_forward_failure(
        "logs",
        &stale(1, "n1"),
        &failure(ForwardFailure::Deadline, "late"),
        true,
    ));
    assert_eq!(
        message,
        "the owner n1 did not answer within the deadline; the operation may or may not have \
         been applied there, so read before retrying a write"
    );
    assert_eq!((owner.as_deref(), retryable), (Some("n1"), true));
    let (message, _, _) = not_here(router.after_forward_failure(
        "logs",
        &stale(1, "n1"),
        &failure(ForwardFailure::Interrupted, "cut"),
        true,
    ));
    assert_eq!(
        message,
        "the connection to the owner n1 dropped while the request was in flight; the \
         operation may or may not have been applied there, so read before retrying a write"
    );
    assert_eq!(plane.doubts.lock().expect("lock").len(), 2);
    assert_eq!(*plane.head_reads.lock().expect("lock"), 0);
}

#[test]
fn a_non_owner_or_unreachable_owner_reads_the_head_once_and_forwards_or_refuses() {
    for kind in [ForwardFailure::NotOwner, ForwardFailure::Unreachable] {
        let plane = Fake::new();
        let router = Router::new(&plane, true);
        // A new owner: forwarded to.
        plane.head(1, "n2");
        let out = router.after_forward_failure("logs", &stale(1, "n1"), &failure(kind, "x"), true);
        assert_eq!(
            out,
            Outcome::Forward(Placement {
                local: false,
                shard: 1,
                owner: Some("n2".to_owned()),
                uuid: "u1".to_owned()
            })
        );
        assert_eq!(*plane.head_reads.lock().expect("lock"), 1);
        assert!(
            plane.doubts.lock().expect("lock").is_empty(),
            "no doubt on a forward"
        );
        // No owner.
        let plane = Fake::new();
        let router = Router::new(&plane, true);
        let (message, owner, retryable) = not_here(router.after_forward_failure(
            "logs",
            &stale(1, "n1"),
            &failure(kind, "x"),
            true,
        ));
        assert_eq!(
            message,
            "no node currently owns shard 1 of logs; activate it before writing"
        );
        assert_eq!((owner, retryable), (None, false));
        assert_eq!(plane.doubts.lock().expect("lock").len(), 1);
        // This node.
        let plane = Fake::new();
        plane.head(1, "me");
        let router = Router::new(&plane, true);
        let (message, owner, retryable) = not_here(router.after_forward_failure(
            "logs",
            &stale(1, "n1"),
            &failure(kind, "x"),
            true,
        ));
        assert_eq!(message, "this node is acquiring shard 1 of logs; retry");
        assert_eq!((owner.as_deref(), retryable), (Some("me"), true));
        // The same owner.
        let plane = Fake::new();
        plane.head(1, "n1");
        let router = Router::new(&plane, true);
        let (message, owner, retryable) = not_here(router.after_forward_failure(
            "logs",
            &stale(1, "n1"),
            &failure(kind, "refused"),
            true,
        ));
        assert_eq!(
            message,
            "could not forward to n1, which the shard-head named as owner: refused"
        );
        assert_eq!((owner.as_deref(), retryable), (Some("n1"), true));
        assert_eq!(plane.doubts.lock().expect("lock").len(), 1);
    }
}

#[test]
fn an_owner_with_no_lease_keeps_its_own_words_and_no_retry_reads_no_head() {
    let plane = Fake::new();
    plane.head(1, "n1");
    let router = Router::new(&plane, true);
    let own = Failure {
        owner_unreachable: true,
        ..failure(ForwardFailure::Unreachable, "n1 holds no live lease")
    };
    let (message, _, _) =
        not_here(router.after_forward_failure("logs", &stale(1, "n1"), &own, true));
    assert_eq!(message, "n1 holds no live lease");
    let reads = *plane.head_reads.lock().expect("lock");
    let (message, owner, retryable) = not_here(router.after_forward_failure(
        "logs",
        &stale(1, "n1"),
        &failure(ForwardFailure::NotOwner, "x"),
        false,
    ));
    assert_eq!(
        message,
        "could not forward to n1, which the shard-head named as owner: x"
    );
    assert_eq!((owner.as_deref(), retryable), (Some("n1"), true));
    assert_eq!(*plane.head_reads.lock().expect("lock"), reads, "no read");
}

#[test]
fn not_here_words_no_owner_this_node_and_another_node() {
    let plane = Fake::new();
    let router = Router::new(&plane, true);
    let none = Placement {
        owner: None,
        ..stale(2, "x")
    };
    let r = router.not_here("logs", &none);
    assert_eq!(
        r,
        Refusal::NotHere {
            message: "no node currently owns shard 2 of logs".to_owned(),
            owner: None,
            retryable: false
        }
    );
    let here = router.not_here("logs", &stale(2, "me"));
    assert_eq!(
        here,
        Refusal::NotHere {
            message: "this node is acquiring shard 2 of logs; retry".to_owned(),
            owner: Some("me".to_owned()),
            retryable: true
        }
    );
    assert_eq!(plane.doubts.lock().expect("lock").len(), 2);
    let other = router.not_here("logs", &stale(2, "n9"));
    assert_eq!(
        other,
        Refusal::NotHere {
            message: "shard 2 of logs is owned by n9".to_owned(),
            owner: Some("n9".to_owned()),
            retryable: true
        }
    );
    assert_eq!(
        plane.doubts.lock().expect("lock").len(),
        2,
        "no doubt for a named owner"
    );
}
