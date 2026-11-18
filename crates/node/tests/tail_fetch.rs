//! docs/specs/node-tail-links: a reader's fetch of a writer's tail over a
//! real socket, signed, and the handlers' bounds.

#![cfg(all(feature = "reader", feature = "writer"))]
// ARITH: test sequence numbers are small constants.
#![allow(clippy::arithmetic_side_effects)]

mod tail_rig;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use oss_cluster::lease::LeaseClock;
use oss_format::WalRecord;
use oss_node::newest_ask::{LiveWriter, LiveWriters};
use oss_node::tail_held::TailHeld;
use oss_node::tail_keep::TailKeep;
use oss_node::tail_link::{MAX_FETCH_BYTES, TailFetchHandler};
use oss_node::tail_reader_link::{
    FETCH_TIMEOUT, MAX_PUSH_BYTES, TailPushHandler, TransportTailFetch,
};
use oss_node::tail_serve::{ShardFacts, TailServe};
use oss_transport::actions::tail::{FETCH_ACTION, FetchRequest, NEW, PUSH_ACTION};
use oss_transport::auth::{Guarded, identity};
use oss_transport::exception::RemoteException;
use oss_transport::frame::VariableHeader;
use oss_transport::server::{Handler, Handlers, Request, Server};
use oss_transport::stream::Output;
use tail_rig::{NOW, OTHER, SECRET, auth, client, request, serve, shard_ref};

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

/// A valid message padded with zeros to `len` bytes.
fn padded(message: &impl Encodes, len: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    message.encode(&mut bytes);
    assert!(bytes.len() <= len);
    bytes.resize(len, 0);
    bytes
}

trait Encodes {
    fn encode(&self, into: &mut Vec<u8>);
}

impl Encodes for oss_transport::actions::tail::PushRequest {
    fn encode(&self, into: &mut Vec<u8>) {
        self.write(&mut Output::new(into, 1024)).expect("encodes");
    }
}

impl Encodes for FetchRequest {
    fn encode(&self, into: &mut Vec<u8>) {
        self.write(&mut Output::new(into, 1024)).expect("encodes");
    }
}

#[test]
fn a_handler_takes_a_message_at_its_bound_and_refuses_one_over_it() {
    let variable = VariableHeader::default();
    let held = Arc::new(TailHeld::new());
    let push = TailPushHandler::new(held, Arc::new(|| 5));
    let fetch = TailFetchHandler::new(Arc::new(TailServe::new(
        Arc::new(TailKeep::new()),
        Arc::new(Facts(true)),
        Arc::new(Clock),
    )));
    let take = |handler: &dyn Handler, action: &str, message: &[u8]| {
        let mut out = Vec::new();
        handler
            .handle(
                &request(action, message, &variable),
                &mut Output::new(&mut out, 1 << 20),
            )
            .is_ok()
    };
    let empty_push = oss_transport::actions::tail::PushRequest::default();
    assert!(take(
        &push,
        PUSH_ACTION,
        &padded(&empty_push, MAX_PUSH_BYTES)
    ));
    assert!(!take(
        &push,
        PUSH_ACTION,
        &padded(&empty_push, MAX_PUSH_BYTES + 1)
    ));
    let ask = FetchRequest {
        shard: shard_ref("u1"),
        ..FetchRequest::default()
    };
    assert!(take(&fetch, FETCH_ACTION, &padded(&ask, MAX_FETCH_BYTES)));
    assert!(!take(
        &fetch,
        FETCH_ACTION,
        &padded(&ask, MAX_FETCH_BYTES + 1)
    ));
}

#[test]
fn the_builders_fetcher_trait_reaches_the_same_writer() {
    let server = writer(SECRET);
    let asker = asker(Some(server.address()));
    let fetcher: &dyn oss_node::overlay_builder::TailFetcher = &asker;
    let answer = fetcher
        .fetch("writer-1", &fetch_request())
        .expect("answered");
    assert_eq!(answer.records.len(), 2);
}
