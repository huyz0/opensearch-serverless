//! docs/specs/node-tail-links: the tail's push and fetch over real sockets,
//! signed, between a writer's `TailServe` and a reader's `TailHeld`.

#![cfg(all(feature = "reader", feature = "writer"))]
// ARITH: test sequence numbers are small constants.
#![allow(clippy::arithmetic_side_effects)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use oss_cluster::lease::LeaseClock;
use oss_format::{NodeLease, WalRecord};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::newest_ask::{LiveWriter, LiveWriters};
use oss_node::push_link::Leases;
use oss_node::push_sender::LinkError;
use oss_node::tail_held::TailHeld;
use oss_node::tail_keep::TailKeep;
use oss_node::tail_link::{TailFetchHandler, TransportTailLink};
use oss_node::tail_push::TailLink;
use oss_node::tail_reader_link::{FETCH_TIMEOUT, TailPushHandler, TransportTailFetch};
use oss_node::tail_serve::{ShardFacts, TailServe};
use oss_transport::actions::tail::{
    FETCH_ACTION, FetchRequest, NEW, PUSH_ACTION, PushRequest, Record, ShardRef,
};
use oss_transport::auth::{AuthConfig, Authenticator, Guarded, SecretSource, Secrets, identity};
use oss_transport::client::{Client, ClientConfig};
use oss_transport::exception::RemoteException;
use oss_transport::frame::VariableHeader;
use oss_transport::node::{DiscoveryNode, NodeRole, TransportAddress};
use oss_transport::server::{Handler, Handlers, Request, Server, ServerConfig};
use oss_transport::stream::Output;
use oss_transport::version::WireVersion;

const NOW: i64 = 1_000_000;
const SECRET: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const OTHER: &str = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";

struct Secret(String);

impl SecretSource for Secret {
    fn secrets(&self) -> Option<Arc<Secrets>> {
        Some(Arc::new(Secrets {
            generation: 1,
            current: self.0.clone(),
            previous: None,
            rotated_at_millis: 0,
        }))
    }

    fn refresh(&self) {}
}

fn auth(who: &str, secret: &str) -> Arc<Authenticator> {
    Arc::new(Authenticator::new(
        AuthConfig {
            identity: who.to_owned(),
            window_millis: 60_000,
            replay_capacity: 64,
        },
        Arc::new(Secret(secret.to_owned())),
        Arc::new(|| NOW),
    ))
}

fn budget() -> MemoryBudget {
    MemoryBudget::new(
        &BudgetConfig::new(64 << 20)
            .with_pool(Pool::Transport, 64 << 20)
            .expect("fits"),
    )
}

fn client() -> Arc<Client> {
    Arc::new(Client::new(ClientConfig::default(), budget()))
}

fn serve(id: &str, handlers: Handlers) -> Server {
    let node = DiscoveryNode {
        name: id.into(),
        id: id.into(),
        ephemeral_id: "eph".into(),
        host_name: "127.0.0.1".into(),
        host_address: "127.0.0.1".into(),
        address: TransportAddress {
            ip: "127.0.0.1".parse().expect("ip"),
            host: "127.0.0.1".into(),
            port: 9300,
        },
        stream_address: None,
        attributes: Vec::new(),
        roles: vec![NodeRole::data()],
        version: WireVersion::SHELL,
    };
    Server::bind(
        "127.0.0.1:0".parse().expect("addr"),
        ServerConfig::new(node, "cluster", 4),
        budget(),
        handlers,
    )
    .expect("binds")
}

struct Fixed(Arc<[NodeLease]>);

impl Leases for Fixed {
    fn current(&self) -> Arc<[NodeLease]> {
        Arc::clone(&self.0)
    }
}

fn lease(id: &str, address: &str, revoked: bool) -> NodeLease {
    let mut lease = NodeLease::new(id, "eph", address, NOW + 60_000);
    lease.revoked = revoked;
    lease
}

fn shard_ref(uuid: &str) -> ShardRef {
    ShardRef {
        index_name: "logs".into(),
        index_uuid: uuid.into(),
        shard: 0,
    }
}

fn record(id: &str, seq: i64) -> Record {
    Record {
        id: id.into(),
        deletion: false,
        source: r#"{"n":1}"#.into(),
        seq_no: seq,
        primary_term: 1,
        version: seq + 1,
        routing: None,
    }
}

fn request<'a>(action: &'a str, message: &'a [u8], variable: &'a VariableHeader) -> Request<'a> {
    Request {
        action,
        variable,
        message,
        version: WireVersion::SHELL,
    }
}

// ---- the reader's side of a push

fn reader(secret: &str, held: Arc<TailHeld>) -> Server {
    let mut handlers: Handlers = HashMap::new();
    handlers.insert(
        PUSH_ACTION.into(),
        Arc::new(Guarded::new(
            auth(&identity("reader-1", "eph"), secret),
            TailPushHandler::new(held, Arc::new(|| 5)),
        )),
    );
    serve("reader-1", handlers)
}

fn link(address: &str, secret: &str, revoked: bool) -> TransportTailLink<Fixed> {
    TransportTailLink::new(
        client(),
        auth(&identity("writer-1", "eph"), secret),
        Fixed(vec![lease("reader-1", address, revoked)].into()),
    )
}

fn push_of(uuids: &[&str]) -> PushRequest {
    PushRequest {
        shards: uuids
            .iter()
            .map(|u| (shard_ref(u), vec![record("a", 10), record("b", 11)]))
            .collect(),
        ..PushRequest::default()
    }
}

#[test]
fn a_push_reaches_the_reader_which_holds_the_shard_it_tracks_and_names_the_rest() {
    let held = Arc::new(TailHeld::new());
    held.track("u1", 0, -1);
    let server = reader(SECRET, Arc::clone(&held));
    let link = link(&server.address().to_string(), SECRET, false);
    let answer = link
        .send("reader-1", &push_of(&["u1", "u2"]))
        .expect("sent");
    assert_eq!(answer.unwanted, [shard_ref("u2")], "the untracked one");
    let kept = held.from("u1", 0, 0);
    let ids: Vec<&str> = kept.iter().map(|(_, r)| r.id.as_str()).collect();
    assert_eq!(ids, ["a", "b"], "as sent, in order");
    assert_eq!(kept[0].1, record("a", 10));
    assert!(held.from("u2", 0, 0).is_empty());
}

#[test]
fn a_push_that_cannot_be_delivered_or_verified_is_a_link_error_and_holds_nothing() {
    let held = Arc::new(TailHeld::new());
    held.track("u1", 0, -1);
    let server = reader(SECRET, Arc::clone(&held));
    let address = server.address().to_string();
    assert_eq!(
        link(&address, OTHER, false).send("reader-1", &push_of(&["u1"])),
        Err(LinkError),
        "signed with another secret"
    );
    let right = link(&address, SECRET, false);
    assert_eq!(right.send("reader-2", &push_of(&["u1"])), Err(LinkError));
    assert_eq!(
        link(&address, SECRET, true).send("reader-1", &push_of(&["u1"])),
        Err(LinkError),
        "revoked"
    );
    assert_eq!(
        link("not an address", SECRET, false).send("reader-1", &push_of(&["u1"])),
        Err(LinkError)
    );
    assert_eq!(
        link("127.0.0.1:1", SECRET, false).send("reader-1", &push_of(&["u1"])),
        Err(LinkError),
        "nobody listens"
    );
    assert!(held.from("u1", 0, 0).is_empty(), "nothing was taken");
}

#[test]
fn the_push_handler_refuses_a_message_that_does_not_decode() {
    let held = Arc::new(TailHeld::new());
    let handler = TailPushHandler::new(held, Arc::new(|| 5));
    let variable = VariableHeader::default();
    let mut out = Vec::new();
    let result = handler.handle(
        &request(PUSH_ACTION, &[0xff, 0xff, 0xff, 0xff], &variable),
        &mut Output::new(&mut out, 1024),
    );
    assert!(result.is_err());
}

// ---- the writer's side of a fetch

struct Facts(bool);

impl ShardFacts for Facts {
    fn writes(&self, _: &str, _: u32) -> bool {
        self.0
    }
    fn ownership_in_doubt(&self) -> bool {
        false
    }
    fn overlay_on(&self, _: &str, _: u32) -> bool {
        true
    }
    fn max_seq_no(&self, _: &str, _: u32) -> i64 {
        11
    }
}

struct Clock;

impl LeaseClock for Clock {
    fn now_millis(&self) -> i64 {
        NOW
    }
    fn pause(&self, _: Duration) {}
}

/// A writer keeping `a` (seq 10) and `b` (seq 11) of shard `u1` past 9.
fn writer(secret: &str) -> Server {
    let keep = Arc::new(TailKeep::new());
    keep.begin("u1", 0, 9);
    keep.acked(
        "u1",
        0,
        &[
            WalRecord::index("a".to_owned(), r#"{"n":1}"#.to_owned(), 10, 1, 11),
            WalRecord::index("b".to_owned(), r#"{"n":1}"#.to_owned(), 11, 1, 12),
        ],
        true,
    );
    let serve_tail = Arc::new(TailServe::new(keep, Arc::new(Facts(true)), Arc::new(Clock)));
    let mut handlers: Handlers = HashMap::new();
    handlers.insert(
        FETCH_ACTION.into(),
        Arc::new(Guarded::new(
            auth(&identity("writer-1", "eph"), secret),
            TailFetchHandler::new(serve_tail),
        )),
    );
    serve("writer-1", handlers)
}

struct Live(Option<LiveWriter>);

impl LiveWriters for Live {
    fn live(&self, node_id: &str) -> Option<LiveWriter> {
        (node_id == "writer-1").then(|| self.0.clone()).flatten()
    }
}

fn asker(address: Option<SocketAddr>) -> TransportTailFetch<Live> {
    TransportTailFetch::new(
        client(),
        auth(&identity("reader-1", "eph"), SECRET),
        Live(address.map(|address| LiveWriter {
            address,
            ephemeral_id: "eph".into(),
        })),
    )
}

fn fetch_request() -> FetchRequest {
    FetchRequest {
        shard: shard_ref("u1"),
        after_seq_no: 9,
        subscriber: "reader-1".into(),
        subscription: NEW,
        ..FetchRequest::default()
    }
}

#[test]
fn a_fetch_reaches_the_writer_signed_and_the_answer_grants_the_shard() {
    let server = writer(SECRET);
    let answer = asker(Some(server.address()))
        .fetch("writer-1", &fetch_request())
        .expect("answered");
    let ids: Vec<&str> = answer.records.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["a", "b"]);
    assert!(answer.subscription >= 0, "subscribed");
    assert!(answer.complete);
    // Fed to the reader's store, the shard is granted under the writer.
    let held = TailHeld::new();
    held.track("u1", 0, 9);
    held.answered("u1", 0, "writer-1", NEW, NOW, &answer);
    assert_eq!(held.from("u1", 0, 0).len(), 2);
    assert!(held.granted("u1", 0, "writer-1", NOW + 10));
}

#[test]
fn a_fetch_to_a_writer_not_shown_live_or_one_that_does_not_answer_gives_nothing() {
    let server = writer(SECRET);
    // A writer that would answer, not shown live: nothing is sent to it.
    let reached = Arc::new(AtomicBool::new(false));
    let mut handlers: Handlers = HashMap::new();
    handlers.insert(
        FETCH_ACTION.into(),
        Arc::new(Guarded::new(
            auth(&identity("writer-1", "eph"), SECRET),
            Odder(Odd::Garbage, Arc::clone(&reached)),
        )),
    );
    let listening = serve("writer-1", handlers);
    assert_eq!(asker(None).fetch("writer-1", &fetch_request()), None);
    assert!(!reached.load(Ordering::SeqCst), "not shown live: not asked");
    drop(listening);
    // Signed with the reader's secret, which the writer here does not share.
    let other = writer(OTHER);
    assert_eq!(
        asker(Some(other.address())).fetch("writer-1", &fetch_request()),
        None
    );
    // Nobody listening.
    assert_eq!(
        asker(Some("127.0.0.1:1".parse().expect("addr"))).fetch("writer-1", &fetch_request()),
        None
    );
    drop(server);
}

/// What a writer does with a fetch other than answering it.
#[derive(Clone, Copy)]
enum Odd {
    Silent,
    Fail,
    Garbage,
}

struct Odder(Odd, Arc<AtomicBool>);

impl Handler for Odder {
    fn handle(&self, _: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
        self.1.store(true, Ordering::SeqCst);
        match self.0 {
            Odd::Silent => std::thread::sleep(FETCH_TIMEOUT.saturating_mul(3)),
            Odd::Fail => return Err(RemoteException::status("failed", "INTERNAL_SERVER_ERROR")),
            Odd::Garbage => out.bytes(&[0xff, 0xff, 0xff]).expect("writes"),
        }
        Ok(())
    }
}

#[test]
fn a_writer_that_is_silent_fails_or_answers_garbage_is_no_answer() {
    for odd in [Odd::Silent, Odd::Fail, Odd::Garbage] {
        let reached = Arc::new(AtomicBool::new(false));
        let mut handlers: Handlers = HashMap::new();
        handlers.insert(
            FETCH_ACTION.into(),
            Arc::new(Guarded::new(
                auth(&identity("writer-1", "eph"), SECRET),
                Odder(odd, Arc::clone(&reached)),
            )),
        );
        let server = serve("writer-1", handlers);
        let started = std::time::Instant::now();
        assert_eq!(
            asker(Some(server.address())).fetch("writer-1", &fetch_request()),
            None
        );
        assert!(started.elapsed() < FETCH_TIMEOUT.saturating_mul(3));
        assert!(reached.load(Ordering::SeqCst), "it was asked");
    }
}

#[test]
fn the_fetch_handler_refuses_a_message_that_does_not_decode_and_a_shard_it_does_not_write_gets_no_grant()
 {
    // A node that does not write the shard.
    let handler = TailFetchHandler::new(Arc::new(TailServe::new(
        Arc::new(TailKeep::new()),
        Arc::new(Facts(false)),
        Arc::new(Clock),
    )));
    let variable = VariableHeader::default();
    let mut out = Vec::new();
    assert!(
        handler
            .handle(
                &request(FETCH_ACTION, &[0xff, 0xff, 0xff, 0xff], &variable),
                &mut Output::new(&mut out, 1024),
            )
            .is_err()
    );
    // A shard nothing was kept for: answered, without a grant it could hold.
    let server = {
        let mut handlers: Handlers = HashMap::new();
        handlers.insert(
            FETCH_ACTION.into(),
            Arc::new(Guarded::new(
                auth(&identity("writer-1", "eph"), SECRET),
                handler,
            )),
        );
        serve("writer-1", handlers)
    };
    let mut ask = fetch_request();
    ask.shard = shard_ref("unknown");
    let answer = asker(Some(server.address()))
        .fetch("writer-1", &ask)
        .expect("answered");
    assert_eq!(answer.subscription, oss_transport::actions::tail::NONE);
    assert!(answer.records.is_empty());
    assert_eq!(answer.claimed_through, i64::MAX, "no claim to compare");
}

#[test]
fn a_push_over_the_bound_or_to_another_process_is_a_link_error() {
    let held = Arc::new(TailHeld::new());
    held.track("u1", 0, -1);
    let server = reader(SECRET, Arc::clone(&held));
    let address = server.address().to_string();
    let right = link(&address, SECRET, false);
    let mut big = push_of(&["u1"]);
    big.shards[0].1[0].source = "x".repeat(oss_node::tail_link::MAX_PUSH_BYTES);
    assert_eq!(right.send("reader-1", &big), Err(LinkError), "too large");
    // The lease names another process than the one listening.
    let mut elsewhere = lease("reader-1", &address, false);
    elsewhere.ephemeral_id = "other".into();
    let wrong = TransportTailLink::new(
        client(),
        auth(&identity("writer-1", "eph"), SECRET),
        Fixed(vec![elsewhere].into()),
    );
    assert_eq!(wrong.send("reader-1", &push_of(&["u1"])), Err(LinkError));
    assert!(held.from("u1", 0, 0).is_empty());
}

#[test]
fn a_handler_refuses_a_message_over_its_bound() {
    let variable = VariableHeader::default();
    let held = Arc::new(TailHeld::new());
    let push = TailPushHandler::new(held, Arc::new(|| 5));
    let big = vec![0_u8; oss_node::tail_reader_link::MAX_PUSH_BYTES + 1];
    let mut out = Vec::new();
    assert!(
        push.handle(
            &request(PUSH_ACTION, &big, &variable),
            &mut Output::new(&mut out, 1024)
        )
        .is_err()
    );
    let fetch = TailFetchHandler::new(Arc::new(TailServe::new(
        Arc::new(TailKeep::new()),
        Arc::new(Facts(true)),
        Arc::new(Clock),
    )));
    let big = vec![0_u8; oss_node::tail_link::MAX_FETCH_BYTES + 1];
    assert!(
        fetch
            .handle(
                &request(FETCH_ACTION, &big, &variable),
                &mut Output::new(&mut out, 1024)
            )
            .is_err()
    );
}
