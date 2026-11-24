//! docs/specs/node-cluster-routing: the cluster half of the writer's routes.

#![cfg(feature = "writer")]

use std::sync::{Arc, Mutex};

use oss_cluster::placement::{ROLE_INGEST, ROLE_SEARCH, ShardKey, owner};
use oss_format::NodeLease;
use oss_node::cluster_owner::{BulkSend, ClusterRoutes, IndexSource};
use oss_node::forward::ForwardError;
use oss_node::push_link::Leases;
use oss_node::read_forward::GetSend;
use oss_node::writer::Routes;
use oss_transport::actions::read::{GetRequest, GetResponse};
use oss_transport::actions::write::{BulkOutcome, BulkRequest, BulkResponse};
use oss_writer::bulk::{Action, IndexInfo, ItemResult, Kind, Op, Owner, Planned};
use oss_writer::index::{Expect, Outcome};

const NOW: i64 = 1_000_000;

struct Fixed(Arc<[NodeLease]>);

impl Leases for Fixed {
    fn current(&self) -> Arc<[NodeLease]> {
        Arc::clone(&self.0)
    }
}

struct Indexes;

impl IndexSource for Indexes {
    fn index(&self, name: &str) -> Option<IndexInfo> {
        (name == "logs").then(info)
    }
}

enum Answer {
    Outcomes(Vec<BulkOutcome>),
    Fail(fn() -> ForwardError),
}

struct Fake {
    seen: Mutex<Vec<(String, BulkRequest<'static>)>>,
    answer: Mutex<Option<Answer>>,
}

struct Sender(Arc<Fake>);

impl BulkSend for Sender {
    fn bulk(&self, node: &str, request: &BulkRequest) -> Result<BulkResponse, ForwardError> {
        self.0
            .seen
            .lock()
            .expect("lock")
            .push((node.to_owned(), request.clone().into_owned()));
        match self.0.answer.lock().expect("lock").take() {
            Some(Answer::Outcomes(outcomes)) => Ok(BulkResponse {
                owner_node_id: node.to_owned(),
                outcomes,
                refreshed: request.refresh,
            }),
            Some(Answer::Fail(e)) => Err(e()),
            None => Err(ForwardError::NoPeer),
        }
    }
}

fn info() -> IndexInfo {
    IndexInfo {
        name: "logs".into(),
        uuid: "u1".into(),
        shards: 4,
        partition_size: 1,
        routing_shards: 4,
        routing_required: false,
        default_pipeline: None,
        final_pipeline: None,
    }
}

fn lease(id: &str, role: &str) -> NodeLease {
    let mut l = NodeLease::new(id, "eph", "127.0.0.1:9300", NOW + 30_000);
    l.set_roles([role.to_owned()]);
    l
}

fn routes(leases: Vec<NodeLease>, fake: &Arc<Fake>, me: &str) -> impl Routes {
    ClusterRoutes::new(Indexes, Fixed(leases.into()), Sender(Arc::clone(fake)), me)
}

fn fake(answer: Answer) -> Arc<Fake> {
    Arc::new(Fake {
        seen: Mutex::default(),
        answer: Mutex::new(Some(answer)),
    })
}

fn op(kind: Kind, create: bool, source: &[u8]) -> Op {
    Op {
        action: Action {
            kind,
            index: Some("logs".into()),
            id: Some("a".into()),
            routing: Some("r".into()),
            create,
            expect: Expect::default(),
            pipeline: None,
            require_alias: false,
        },
        source: source.to_vec(),
    }
}

fn fleet() -> Vec<NodeLease> {
    ["n1", "n2", "n3"].map(|id| lease(id, ROLE_INGEST)).to_vec()
}

fn winner(ids: &[&str], shard: i32) -> String {
    owner(ShardKey::new("logs", shard), ids.iter().copied())
        .expect("a winner")
        .to_owned()
}

// AC-1
#[test]
fn the_winner_is_local_only_on_its_own_node() {
    let f = fake(Answer::Outcomes(vec![]));
    for shard in 0..4u32 {
        let w = winner(&["n1", "n2", "n3"], i32::try_from(shard).unwrap());
        for me in ["n1", "n2", "n3"] {
            let got = routes(fleet(), &f, me).owner(&info(), shard);
            if me == w {
                assert_eq!(got, Owner::Local, "shard {shard} on {me}");
            } else {
                assert_eq!(got, Owner::Node(w.clone()), "shard {shard} on {me}");
            }
        }
    }
}

// AC-2
#[test]
fn dead_and_non_ingest_members_are_never_chosen() {
    let f = fake(Answer::Outcomes(vec![]));
    let mut leases = fleet();
    leases.push(lease("search-only", ROLE_SEARCH));
    let mut revoked = lease("revoked", ROLE_INGEST);
    revoked.revoked = true;
    leases.push(revoked);
    for shard in 0..32u32 {
        let w = winner(&["n1", "n2", "n3"], i32::try_from(shard).unwrap());
        assert_eq!(
            routes(leases.clone(), &f, "n9").owner(&info(), shard),
            Owner::Node(w)
        );
    }
    assert_eq!(
        routes(vec![lease("s", ROLE_SEARCH)], &f, "n1").owner(&info(), 0),
        Owner::Local
    );
}

#[test]
fn an_index_is_the_sources() {
    let f = fake(Answer::Outcomes(vec![]));
    let r = routes(fleet(), &f, "n1");
    assert!(r.index("logs").is_some());
    assert!(r.index("other").is_none());
}

fn planned<'o>(ops: &'o [Op], ids: &'o [&'o str]) -> Vec<Planned<'o>> {
    ops.iter()
        .zip(ids)
        .enumerate()
        .map(|(position, (op, id))| Planned { position, op, id })
        .collect()
}

fn ok(created: bool) -> BulkOutcome {
    BulkOutcome {
        created,
        version: 3,
        seq_no: 7,
        primary_term: 2,
        ..BulkOutcome::default()
    }
}

// AC-3 and AC-4
#[test]
fn a_group_is_one_request_and_each_outcome_is_the_items() {
    let ops = [
        op(Kind::Index, false, b"{\"a\":1}"),
        op(Kind::Create, false, b"{}"),
        op(Kind::Delete, false, b""),
        op(Kind::Delete, false, b""),
        op(Kind::Index, false, b"{}"),
        op(Kind::Index, true, b"{}"),
        op(Kind::Index, false, b"{}"),
    ];
    let ids = ["1", "2", "3", "4", "5", "6", "7"];
    let outcomes = vec![
        ok(true),
        ok(false),
        BulkOutcome {
            deletion: true,
            found: true,
            ..ok(false)
        },
        BulkOutcome {
            deletion: true,
            ..ok(false)
        },
        BulkOutcome {
            failure: Some("conflict".into()),
            conflict: true,
            ..BulkOutcome::default()
        },
        BulkOutcome {
            failure: Some("busy".into()),
            retryable: true,
            ..BulkOutcome::default()
        },
        BulkOutcome {
            failure: Some("boom".into()),
            ..BulkOutcome::default()
        },
    ];
    let f = fake(Answer::Outcomes(outcomes));
    let group = planned(&ops, &ids);
    let results = routes(fleet(), &f, "n1").forward("n2", &info(), 2, &group);
    let seen = f.seen.lock().expect("lock");
    assert_eq!(seen.len(), 1);
    let (node, request) = &seen[0];
    assert_eq!(node, "n2");
    assert_eq!((request.index.as_str(), request.shard), ("logs", 2));
    assert_eq!(request.index_uuid.as_deref(), Some("u1"));
    let sent = &request.operations;
    assert_eq!(sent.len(), 7);
    assert_eq!((sent[0].id.as_str(), &*sent[0].source), ("1", "{\"a\":1}"));
    assert_eq!(sent[0].routing.as_deref(), Some("r"));
    assert!(!sent[0].deletion && !sent[0].require_absent);
    assert!(sent[1].require_absent);
    assert!(sent[2].deletion && sent[2].source.is_empty());
    assert!(sent[5].require_absent);
    assert_eq!(sent[0].if_seq_no, oss_writer::index::UNASSIGNED_SEQ_NO);

    let written = |r: &ItemResult| match r {
        ItemResult::Written { written, .. } => Some(*written),
        ItemResult::Failed { .. } => None,
    };
    let w0 = written(&results[0]).expect("written");
    assert_eq!(
        (w0.outcome, w0.version, w0.seq_no, w0.primary_term),
        (Outcome::Created, 3, 7, 2)
    );
    assert_eq!(written(&results[1]).expect("w").outcome, Outcome::Updated);
    assert_eq!(written(&results[2]).expect("w").outcome, Outcome::Deleted);
    assert_eq!(written(&results[3]).expect("w").outcome, Outcome::NotFound);
    let status = |r: &ItemResult| match r {
        ItemResult::Failed { status, .. } => *status,
        ItemResult::Written { .. } => 0,
    };
    assert_eq!(
        [
            status(&results[4]),
            status(&results[5]),
            status(&results[6])
        ],
        [409, 429, 500]
    );
}

// AC-5
#[test]
fn a_gone_owner_a_large_request_and_a_short_answer_fail_their_items() {
    let ops = [op(Kind::Index, false, b"{}"), op(Kind::Index, false, b"{}")];
    let ids = ["1", "2"];
    let group = planned(&ops, &ids);
    let status = |r: &ItemResult| match r {
        ItemResult::Failed { status, .. } => *status,
        ItemResult::Written { .. } => 0,
    };
    let f = fake(Answer::Fail(|| ForwardError::Gone));
    let r = routes(fleet(), &f, "n1").forward("n2", &info(), 0, &group);
    assert_eq!(r.iter().map(status).collect::<Vec<_>>(), [503, 503]);
    let f = fake(Answer::Fail(|| ForwardError::TooLarge));
    let r = routes(fleet(), &f, "n1").forward("n2", &info(), 0, &group);
    assert_eq!(r.iter().map(status).collect::<Vec<_>>(), [413, 413]);
    let f = fake(Answer::Outcomes(vec![ok(true)]));
    let r = routes(fleet(), &f, "n1").forward("n2", &info(), 0, &group);
    assert_eq!(r.iter().map(status).collect::<Vec<_>>(), [0, 502]);
    let f = fake(Answer::Outcomes(vec![ok(true), ok(true), ok(true)]));
    let r = routes(fleet(), &f, "n1").forward("n2", &info(), 0, &group);
    assert_eq!(r.iter().map(status).collect::<Vec<_>>(), [502, 502]);
}

// AC-6
#[test]
fn an_update_or_bad_utf8_fails_alone_and_is_not_sent() {
    let ops = [
        op(Kind::Update, false, b"{}"),
        op(Kind::Index, false, &[0xff, 0xfe]),
        op(Kind::Delete, false, &[0xff]),
        op(Kind::Index, false, b"{}"),
    ];
    let ids = ["1", "2", "3", "4"];
    let group = planned(&ops, &ids);
    let f = fake(Answer::Outcomes(vec![
        BulkOutcome {
            deletion: true,
            found: true,
            ..ok(false)
        },
        ok(true),
    ]));
    let r = routes(fleet(), &f, "n1").forward("n2", &info(), 0, &group);
    let seen = f.seen.lock().expect("lock");
    let ids_sent: Vec<&str> = seen[0].1.operations.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(ids_sent, ["3", "4"]);
    assert!(matches!(r[0], ItemResult::Failed { status: 400, .. }));
    assert!(matches!(r[1], ItemResult::Failed { status: 400, .. }));
    assert!(matches!(r[2], ItemResult::Written { .. }));
    assert!(matches!(r[3], ItemResult::Written { .. }));

    let f = fake(Answer::Outcomes(vec![]));
    let only = planned(&ops[..1], &ids[..1]);
    let r = routes(fleet(), &f, "n1").forward("n2", &info(), 0, &only);
    assert!(f.seen.lock().expect("lock").is_empty());
    assert!(matches!(r[0], ItemResult::Failed { status: 400, .. }));
}

// AC-7
#[test]
fn a_refresh_of_a_shard_another_node_owns_is_asked_of_that_owner() {
    let f = fake(Answer::Outcomes(Vec::new()));
    let r = routes(fleet(), &f, "n1");
    assert_eq!(r.refresh_remote("n2", &info(), 3), Ok(()));
    let seen = f.seen.lock().expect("lock");
    let (node, request) = seen.first().expect("one request");
    assert_eq!(node, "n2");
    assert!(request.refresh && request.operations.is_empty());
    assert_eq!((request.shard, request.index.as_str()), (3, "logs"));
    drop(seen);
    let f = fake(Answer::Fail(|| ForwardError::Gone));
    assert!(
        routes(fleet(), &f, "n1")
            .refresh_remote("n2", &info(), 3)
            .is_err()
    );
}

/// Answers a forwarded get from a fixed result.
struct Reader(Mutex<Vec<(String, GetRequest)>>, Option<GetResponse>);

impl GetSend for Reader {
    fn get(&self, node: &str, request: &GetRequest) -> Result<GetResponse, ForwardError> {
        self.0
            .lock()
            .expect("lock")
            .push((node.to_owned(), request.clone()));
        self.1.clone().ok_or(ForwardError::NoPeer)
    }
}

fn reading(answer: Option<GetResponse>) -> (Arc<Reader>, impl Routes) {
    let reader = Arc::new(Reader(Mutex::default(), answer));
    let routes = ClusterRoutes::new(
        Indexes,
        Fixed(fleet().into()),
        Sender(fake(Answer::Outcomes(Vec::new()))),
        "n1",
    )
    .with_reads(Arc::clone(&reader) as Arc<dyn GetSend>);
    (reader, routes)
}

#[test]
fn a_get_of_a_shard_another_node_owns_is_asked_of_that_owner() {
    let found = GetResponse {
        owner_node_id: "n2".into(),
        id: "a".into(),
        found: true,
        source: Some(r#"{"x":1}"#.into()),
        seq_no: 5,
        primary_term: 2,
        version: 3,
        routing: None,
    };
    let (reader, r) = reading(Some(found));
    let doc = r
        .get_remote("n2", &info(), 3, "a")
        .expect("asked")
        .expect("found");
    assert_eq!(
        (
            doc.version,
            doc.seq_no,
            doc.primary_term,
            doc.source.as_slice()
        ),
        (3, 5, 2, br#"{"x":1}"#.as_slice())
    );
    let seen = reader.0.lock().expect("lock");
    let (node, request) = seen.first().expect("one request");
    assert_eq!(node, "n2");
    assert_eq!(
        (request.index.as_str(), request.shard, request.id.as_str()),
        ("logs", 3, "a")
    );
    assert_eq!(request.index_uuid.as_deref(), Some("u1"));
    drop(seen);

    let (_, r) = reading(Some(GetResponse::default()));
    assert!(
        r.get_remote("n2", &info(), 3, "a")
            .expect("asked")
            .is_none()
    );
    let (_, r) = reading(None);
    assert!(r.get_remote("n2", &info(), 3, "a").is_err());
    assert!(
        routes(fleet(), &fake(Answer::Outcomes(Vec::new())), "n1")
            .get_remote("n2", &info(), 3, "a")
            .is_err(),
        "no reads configured"
    );
}

// AC-8
#[test]
fn a_lease_past_its_stamp_in_the_view_still_takes_writes() {
    let f = fake(Answer::Outcomes(vec![]));
    let mut leases = fleet();
    for l in &mut leases {
        // The view's copy can be a renewal old; the refresh drops the dead.
        l.expires_at_millis = NOW - 5_000;
    }
    for shard in 0..32u32 {
        let w = winner(&["n1", "n2", "n3"], i32::try_from(shard).unwrap());
        assert_eq!(
            routes(leases.clone(), &f, "n9").owner(&info(), shard),
            Owner::Node(w)
        );
    }
}
