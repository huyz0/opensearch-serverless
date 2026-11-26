#![allow(missing_docs)]
//! gRPC calls through the front to a real HTTP/1.1 server: framing, health, limits.

mod support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use bytes::Bytes;
use oss_edge::{Edge, EdgeConfig, EdgeError};
use oss_mem::Pool;
use support::{State, WORKERS, grpc_call, grpc_frame, h2c, start};

const HEALTH: &str = "/grpc.health.v1.Health/Check";
const SERVING: [u8; 2] = [0x08, 1];
const NOT_SERVING: [u8; 2] = [0x08, 2];

fn config(upstream: std::net::SocketAddr) -> EdgeConfig {
    let mut c = EdgeConfig::new(upstream, WORKERS);
    c.grpc = Some("127.0.0.1:0".parse().expect("addr"));
    c.grpc_max_message = 64 * 1024;
    c
}

#[tokio::test(flavor = "multi_thread")]
async fn no_grpc_address_opens_no_port_and_charges_nothing() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let before = budget.in_use(Pool::RequestBodies);
    let mut c = config(upstream);
    c.grpc = None;
    c.h2c = Some("127.0.0.1:0".parse().expect("addr"));
    let plain = c.charge();
    let edge = Edge::start(c, &budget).expect("starts");
    assert!(edge.grpc_addr().is_none());
    assert_eq!(budget.in_use(Pool::RequestBodies) - before, plain);
    let with = config(upstream).charge();
    assert!(with > plain, "a grpc listener is charged its messages");
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn health_check_tracks_the_node() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.grpc_addr().expect("bound")).await;
    let ok = grpc_call(&mut send, HEALTH, &[], grpc_frame(0, b""))
        .await
        .expect("reply");
    assert_eq!((ok.http_status, ok.grpc_status), (200, Some(0)));
    assert_eq!(ok.content_type.as_deref(), Some("application/grpc"));
    assert_eq!(ok.messages, vec![SERVING.to_vec()]);
    state.unhealthy.store(true, Ordering::Release);
    let down = grpc_call(&mut send, HEALTH, &[], grpc_frame(0, b""))
        .await
        .expect("reply");
    assert_eq!(down.messages, vec![NOT_SERVING.to_vec()]);
    state.unhealthy.store(false, Ordering::Release);
    let named = grpc_call(&mut send, HEALTH, &[], grpc_frame(0, b"\x0a\x03nop"))
        .await
        .expect("reply");
    assert_eq!(
        named.grpc_status,
        Some(5),
        "an unknown service is NOT_FOUND"
    );
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_methods_and_other_content_types_are_refused() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.grpc_addr().expect("bound")).await;
    let other = grpc_call(&mut send, "/nope.Service/Method", &[], grpc_frame(0, b""))
        .await
        .expect("reply");
    assert_eq!(other.grpc_status, Some(12));
    assert!(
        other
            .grpc_message
            .expect("message")
            .contains("/nope.Service/Method")
    );
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("http://node{HEALTH}"))
        .header("content-type", "application/json")
        .body(())
        .expect("request");
    let (response, _stream) = send.send_request(request, true).expect("sent");
    assert_eq!(response.await.expect("response").status(), 415);
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn big_and_compressed_messages_are_refused_before_buffering() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.grpc_addr().expect("bound")).await;
    let big = grpc_call(
        &mut send,
        HEALTH,
        &[],
        grpc_frame(0, &vec![0u8; 64 * 1024 + 1]),
    )
    .await
    .expect("reply");
    assert_eq!(big.grpc_status, Some(8));
    // Exactly the limit is read, and then decoded: health finds it malformed, which is not a size error.
    let exact = grpc_call(&mut send, HEALTH, &[], grpc_frame(0, &vec![0u8; 64 * 1024]))
        .await
        .expect("reply");
    assert_eq!(exact.grpc_status, Some(3));
    // A request that ends inside its message is not served as if it were whole.
    let mut cut = vec![0u8, 0, 0, 0, 5];
    cut.truncate(5);
    let cut = grpc_call(&mut send, HEALTH, &[], Bytes::from(cut))
        .await
        .expect("reply");
    assert_eq!(cut.grpc_status, Some(3));
    let flagged = grpc_call(&mut send, HEALTH, &[], grpc_frame(1, b""))
        .await
        .expect("reply");
    assert_eq!(flagged.grpc_status, Some(12));
    let encoded = grpc_call(
        &mut send,
        HEALTH,
        &[("grpc-encoding", "gzip")],
        grpc_frame(0, b""),
    )
    .await
    .expect("reply");
    assert_eq!(encoded.grpc_status, Some(12));
    let two = [grpc_frame(0, b""), grpc_frame(0, b"")].concat();
    let two = grpc_call(&mut send, HEALTH, &[], Bytes::from(two))
        .await
        .expect("reply");
    assert_eq!(two.grpc_status, Some(3), "a unary call takes one message");
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deadline_ends_the_call_and_frees_its_slot() {
    let state = Arc::new(State::default());
    state.hold_health.store(true, Ordering::Release);
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.grpc_addr().expect("bound")).await;
    let began = Instant::now();
    let late = grpc_call(
        &mut send,
        HEALTH,
        &[("grpc-timeout", "50m")],
        grpc_frame(0, b""),
    )
    .await
    .expect("reply");
    assert_eq!(late.grpc_status, Some(4));
    assert!(began.elapsed() < Duration::from_secs(2));
    state.release.store(true, Ordering::Release);
    let again = grpc_call(&mut send, HEALTH, &[], grpc_frame(0, b""))
        .await
        .expect("reply");
    assert_eq!(again.grpc_status, Some(0));
    let bad = grpc_call(
        &mut send,
        HEALTH,
        &[("grpc-timeout", "5x")],
        grpc_frame(0, b""),
    )
    .await
    .expect("reply");
    assert_eq!(bad.grpc_status, Some(3));
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn calls_past_max_streams_are_unavailable() {
    let state = Arc::new(State::default());
    state.hold_health.store(true, Ordering::Release);
    let (server, budget, upstream) = start(&state);
    let mut c = config(upstream);
    c.max_streams = 1;
    let edge = Edge::start(c, &budget).expect("starts");
    let addr = edge.grpc_addr().expect("bound");
    let mut first = h2c(addr).await;
    let held =
        tokio::spawn(async move { grpc_call(&mut first, HEALTH, &[], grpc_frame(0, b"")).await });
    // The call is in flight once its handler waits; give it time to get there.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut second = h2c(addr).await;
    let refused = grpc_call(&mut second, HEALTH, &[], grpc_frame(0, b""))
        .await
        .expect("reply");
    assert_eq!(refused.grpc_status, Some(14));
    state.release.store(true, Ordering::Release);
    assert_eq!(
        held.await.expect("joined").expect("reply").grpc_status,
        Some(0)
    );
    edge.shutdown();
    server.shutdown();
}

#[test]
fn a_grpc_message_limit_of_zero_is_a_start_error() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let mut c = config(upstream);
    c.grpc_max_message = 0;
    assert!(matches!(Edge::start(c, &budget), Err(EdgeError::Config(_))));
    server.shutdown();
}

#[test]
fn a_certificate_is_loaded_for_grpc_and_only_when_a_tls_or_grpc_listener_needs_it() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let missing = Some(std::path::PathBuf::from("/nonexistent/cert.pem"));
    // grpc alone with a certificate: the certificate is read, so a bad one refuses to start.
    let mut c = config(upstream);
    c.cert = missing.clone();
    c.key = missing.clone();
    assert!(matches!(Edge::start(c, &budget), Err(EdgeError::Tls(_))));
    // h2c alone: no listener uses the certificate, so it is not read.
    let mut c = EdgeConfig::new(upstream, WORKERS);
    c.h2c = Some("127.0.0.1:0".parse().expect("addr"));
    c.cert = missing.clone();
    c.key = missing;
    Edge::start(c, &budget).expect("starts").shutdown();
    server.shutdown();
}

const BULK: &str = "/org.opensearch.protobufs.services.DocumentService/Bulk";

/// `BulkRequest { refresh = WAIT_FOR; one index item with id "1" and source `{}` }`.
fn bulk_request() -> Vec<u8> {
    let mut m = vec![0x30, 3, 0x62, 11];
    m.extend_from_slice(&[0x0a, 5, 0x0a, 3, 0x0a, 1, b'1', 0x1a, 2, b'{', b'}']);
    m
}

#[tokio::test(flavor = "multi_thread")]
async fn bulk_is_mapped_to_rest_and_its_answer_back() {
    let state = Arc::new(State::default());
    *state.bulk_answer.lock().expect("lock") = r#"{"took":3,"errors":false,"items":[{"index":{"_index":"a","_id":"1","_version":1,"result":"created","_shards":{"total":2,"successful":1,"failed":0},"_seq_no":0,"_primary_term":1,"status":201}}]}"#.to_owned();
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.grpc_addr().expect("bound")).await;
    let reply = grpc_call(&mut send, BULK, &[], grpc_frame(0, &bulk_request()))
        .await
        .expect("reply");
    assert_eq!(reply.grpc_status, Some(0), "{:?}", reply.grpc_message);
    let seen = state.bulks.lock().expect("lock").clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "/_bulk?refresh=wait_for");
    assert_eq!(seen[0].1, b"{\"index\":{\"_id\":\"1\"}}\n{}\n");
    let message = &reply.messages[0];
    assert!(
        message.windows(2).any(|w| w == [0x18, 3]),
        "took = 3 is in the message"
    );
    assert!(message.windows(7).any(|w| w == b"created"));
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_field_outside_the_subset_never_reaches_the_node() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.grpc_addr().expect("bound")).await;
    // `type = "t"`, which is deprecated upstream and not covered.
    let reply = grpc_call(&mut send, BULK, &[], grpc_frame(0, b"\x52\x01t"))
        .await
        .expect("reply");
    assert_eq!(reply.grpc_status, Some(12));
    assert!(
        reply
            .grpc_message
            .expect("message")
            .contains("BulkRequest.type")
    );
    assert!(state.bulks.lock().expect("lock").is_empty());
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_answer_key_the_table_does_not_know_is_internal() {
    let state = Arc::new(State::default());
    *state.bulk_answer.lock().expect("lock") =
        r#"{"took":1,"errors":false,"items":[],"surprise":1}"#.to_owned();
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.grpc_addr().expect("bound")).await;
    let reply = grpc_call(&mut send, BULK, &[], grpc_frame(0, &bulk_request()))
        .await
        .expect("reply");
    assert_eq!(reply.grpc_status, Some(13));
    assert!(reply.grpc_message.expect("message").contains("surprise"));
    *state.bulk_answer.lock().expect("lock") = "not json".to_owned();
    let reply = grpc_call(&mut send, BULK, &[], grpc_frame(0, &bulk_request()))
        .await
        .expect("reply");
    assert_eq!(reply.grpc_status, Some(13));
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rest_error_keeps_its_body_in_the_status() {
    let state = Arc::new(State::default());
    state.bulk_status.store(429, Ordering::Release);
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.grpc_addr().expect("bound")).await;
    let reply = grpc_call(&mut send, BULK, &[], grpc_frame(0, &bulk_request()))
        .await
        .expect("reply");
    assert_eq!(reply.grpc_status, Some(8));
    let text = reply.grpc_message.expect("message");
    assert!(text.contains("circuit_breaking_exception"), "{text}");
    assert!(reply.messages.is_empty());
    edge.shutdown();
    server.shutdown();
}

const SEARCH: &str = "/org.opensearch.protobufs.services.SearchService/Search";
const CREATE_PIT: &str = "/org.opensearch.protobufs.services.SearchService/CreatePit";
const DELETE_PIT: &str = "/org.opensearch.protobufs.services.SearchService/DeletePit";
const NO_HITS: &str = r#"{"took":1,"timed_out":false,"_shards":{"total":1,"successful":1,"failed":0},"hits":{"total":{"value":0,"relation":"eq"},"max_score":null,"hits":[]}}"#;

#[tokio::test(flavor = "multi_thread")]
async fn search_and_pit_calls_are_mapped_to_their_rest_twins() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.grpc_addr().expect("bound")).await;
    *state.search_answer.lock().expect("lock") = NO_HITS.to_owned();
    // SearchRequest { index: "a" }
    let reply = grpc_call(&mut send, SEARCH, &[], grpc_frame(0, b"\x0a\x01a"))
        .await
        .expect("reply");
    assert_eq!(reply.grpc_status, Some(0), "{:?}", reply.grpc_message);
    assert_eq!(reply.messages.len(), 1);
    *state.search_answer.lock().expect("lock") =
        r#"{"pit_id":"abc","_shards":{"total":1,"successful":1,"failed":0},"creation_time":5}"#
            .to_owned();
    // CreatePitRequest { index: "a", keep_alive: "1m" }
    let reply = grpc_call(
        &mut send,
        CREATE_PIT,
        &[],
        grpc_frame(0, b"\x0a\x01a\x12\x021m"),
    )
    .await
    .expect("reply");
    assert_eq!(reply.grpc_status, Some(0), "{:?}", reply.grpc_message);
    assert!(reply.messages[0].windows(3).any(|w| w == b"abc"));
    *state.search_answer.lock().expect("lock") =
        r#"{"pits":[{"pit_id":"abc","successful":true}]}"#.to_owned();
    // DeletePitRequest { pit_id: ["abc"] }
    let reply = grpc_call(&mut send, DELETE_PIT, &[], grpc_frame(0, b"\x0a\x03abc"))
        .await
        .expect("reply");
    assert_eq!(reply.grpc_status, Some(0), "{:?}", reply.grpc_message);
    let seen = state.searches.lock().expect("lock").clone();
    assert_eq!(seen.len(), 3);
    assert_eq!(
        (seen[0].0.as_str(), seen[0].1.as_str()),
        ("Post", "/a/_search?")
    );
    assert_eq!(seen[0].2, b"{}");
    assert_eq!(seen[1].1, "/a/_search/point_in_time?keep_alive=1m");
    assert_eq!(
        (seen[2].0.as_str(), seen[2].2.as_slice()),
        ("Delete", &br#"{"pit_id":["abc"]}"#[..])
    );
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn aggregations_are_refused_and_never_reach_the_node() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.grpc_addr().expect("bound")).await;
    // SearchRequest { search_request_body { aggregations { "a": {} } } }
    let message = [
        0x82, 0x02, 0x08, 0xAA, 0x02, 0x05, 0x0a, 0x01, b'a', 0x12, 0x00,
    ];
    let reply = grpc_call(&mut send, SEARCH, &[], grpc_frame(0, &message))
        .await
        .expect("reply");
    assert_eq!(reply.grpc_status, Some(12));
    assert!(
        reply
            .grpc_message
            .expect("message")
            .contains("SearchRequest.search_request_body.aggregations")
    );
    assert!(state.searches.lock().expect("lock").is_empty());
    edge.shutdown();
    server.shutdown();
}
