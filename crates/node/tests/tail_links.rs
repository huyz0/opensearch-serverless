//! docs/specs/node-tail-links: the writer's push to a reader over a real
//! socket, signed.

#![cfg(all(feature = "reader", feature = "writer"))]
// ARITH: test sequence numbers are small constants.
#![allow(clippy::arithmetic_side_effects)]

mod tail_rig;

use std::collections::HashMap;
use std::sync::Arc;

use oss_node::push_sender::LinkError;
use oss_node::tail_held::TailHeld;
use oss_node::tail_link::TransportTailLink;
use oss_node::tail_push::TailLink;
use oss_node::tail_reader_link::TailPushHandler;
use oss_transport::actions::tail::{PUSH_ACTION, PushRequest};
use oss_transport::auth::{Guarded, identity};
use oss_transport::frame::VariableHeader;
use oss_transport::server::{Handler, Handlers, Server};
use oss_transport::stream::Output;
use tail_rig::{Fixed, OTHER, SECRET, auth, client, lease, record, request, serve, shard_ref};

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
