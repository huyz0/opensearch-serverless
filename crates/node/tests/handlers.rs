//! docs/specs/node-handlers: the registry of a node's signed actions.

#![cfg(feature = "reader")]
// ARITH: test sequence numbers are small constants.
#![allow(clippy::arithmetic_side_effects)]

mod tail_rig;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use oss_node::handlers::{Duplicate, Registry};
use oss_node::tail_held::TailHeld;
use oss_node::tail_link::TransportTailLink;
use oss_node::tail_push::TailLink;
use oss_node::tail_reader_link::TailPushHandler;
use oss_transport::actions::tail::{FETCH_ACTION, PUSH_ACTION, PushRequest};
use oss_transport::auth::identity;
use oss_transport::exception::RemoteException;
use oss_transport::frame::VariableHeader;
use oss_transport::server::{Handler, Request};
use oss_transport::stream::Output;
use tail_rig::{Fixed, SECRET, auth, client, lease, record, request, serve, shard_ref};

/// Remembers that it ran.
struct Probe(Arc<AtomicBool>);

impl Handler for Probe {
    fn handle(&self, _: &Request<'_>, _: &mut Output<'_>) -> Result<(), RemoteException> {
        self.0.store(true, Ordering::SeqCst);
        Ok(())
    }
}

fn registry() -> Registry {
    Registry::new(auth(&identity("reader-1", "eph"), SECRET))
}

#[test]
fn an_unsigned_request_never_reaches_a_handler() {
    let ran = Arc::new(AtomicBool::new(false));
    let held = Arc::new(TailHeld::new());
    let mut r = registry();
    r.add("probe", Probe(Arc::clone(&ran))).unwrap();
    r.add(PUSH_ACTION, TailPushHandler::new(held, Arc::new(|| 5)))
        .unwrap();
    let handlers = r.into_handlers();
    let variable = VariableHeader::default();
    for action in ["probe", PUSH_ACTION] {
        let mut out = Vec::new();
        let mut output = Output::new(&mut out, 1 << 16);
        let refused = handlers[action].handle(&request(action, b"", &variable), &mut output);
        assert!(refused.is_err(), "{action} ran unsigned");
    }
    assert!(!ran.load(Ordering::SeqCst), "the inner handler ran");
}

#[test]
fn a_signed_tail_push_reaches_the_held_writes() {
    let held = Arc::new(TailHeld::new());
    held.track("u1", 0, -1);
    let mut r = registry();
    r.add(
        PUSH_ACTION,
        TailPushHandler::new(Arc::clone(&held), Arc::new(|| 5)),
    )
    .unwrap();
    let server = serve("reader-1", r.into_handlers());
    let link = TransportTailLink::new(
        client(),
        auth(&identity("writer-1", "eph"), SECRET),
        Fixed(vec![lease("reader-1", &server.address().to_string(), false)].into()),
    );
    let push = PushRequest {
        shards: vec![(shard_ref("u1"), vec![record("a", 10)])],
        ..PushRequest::default()
    };
    link.send("reader-1", &push).expect("sent");
    assert_eq!(held.from("u1", 0, 0).len(), 1);
}

#[test]
fn a_second_handler_for_an_action_is_refused() {
    let first = Arc::new(TailHeld::new());
    let second = Arc::new(TailHeld::new());
    for held in [&first, &second] {
        held.track("u1", 0, -1);
    }
    let mut r = registry();
    r.add(
        PUSH_ACTION,
        TailPushHandler::new(Arc::clone(&first), Arc::new(|| 5)),
    )
    .unwrap();
    let again = r.add(
        PUSH_ACTION,
        TailPushHandler::new(Arc::clone(&second), Arc::new(|| 5)),
    );
    assert_eq!(again, Err(Duplicate(PUSH_ACTION.into())));
    assert_eq!(
        Duplicate("a".into()).to_string(),
        "a handler is registered for [a] already"
    );
    let server = serve("reader-1", r.into_handlers());
    let link = TransportTailLink::new(
        client(),
        auth(&identity("writer-1", "eph"), SECRET),
        Fixed(vec![lease("reader-1", &server.address().to_string(), false)].into()),
    );
    let push = PushRequest {
        shards: vec![(shard_ref("u1"), vec![record("a", 10)])],
        ..PushRequest::default()
    };
    link.send("reader-1", &push).expect("sent");
    assert_eq!(first.from("u1", 0, 0).len(), 1, "the first stays");
    assert!(second.from("u1", 0, 0).is_empty());
}

#[test]
fn the_registry_lists_what_it_holds() {
    let mut r = registry();
    assert!(r.actions().is_empty());
    r.add(PUSH_ACTION, Probe(Arc::default())).unwrap();
    r.add(FETCH_ACTION, Probe(Arc::default())).unwrap();
    r.add("a/first", Probe(Arc::default())).unwrap();
    assert_eq!(r.actions(), ["a/first", FETCH_ACTION, PUSH_ACTION]);
    let handlers = r.into_handlers();
    assert_eq!(handlers.len(), 3);
    assert!(handlers.contains_key(FETCH_ACTION));
}
