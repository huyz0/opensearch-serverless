#![allow(missing_docs)]
//! `ServerStreamSearch` through the front: pages, ties, deadlines and malformed answers.

mod support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use oss_edge::{Edge, EdgeConfig};
use support::{State, WORKERS, grpc_call, grpc_frame, h2c, start};

const NO_HITS: &str = r#"{"took":1,"timed_out":false,"_shards":{"total":1,"successful":1,"failed":0},"hits":{"total":{"value":0,"relation":"eq"},"max_score":null,"hits":[]}}"#;

fn config(upstream: std::net::SocketAddr) -> EdgeConfig {
    let mut c = EdgeConfig::new(upstream, WORKERS);
    c.grpc = Some("127.0.0.1:0".parse().expect("addr"));
    c.grpc_max_message = 64 * 1024;
    c
}

const STREAM: &str = "/org.opensearch.protobufs.services.SearchService/ServerStreamSearch";

/// A `SearchRequest` for index `a`, as protojson in, wire bytes out.
fn search_request(body: serde_json::Value) -> Vec<u8> {
    use prost::Message;
    let json = serde_json::json!({"index": ["a"], "searchRequestBody": body});
    prost_reflect::DynamicMessage::deserialize(oss_edge::proto_message("SearchRequest"), json)
        .expect("protojson")
        .encode_to_vec()
}

/// The hit ids of one `SearchResponse` message.
fn ids(message: &[u8]) -> Vec<String> {
    let reply =
        prost_reflect::DynamicMessage::decode(oss_edge::proto_message("SearchResponse"), message)
            .expect("decodes");
    let json = serde_json::to_value(&reply).expect("protojson");
    json["hits"]["hits"]
        .as_array()
        .map(|h| {
            h.iter()
                .map(|h| h["xId"].as_str().expect("id").to_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn stream_body(page: u32) -> serde_json::Value {
    serde_json::json!({"size": page, "sort": [{"field": "n"}]})
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stream_search_delivers_every_hit_a_page_a_message() {
    let state = Arc::new(State::default());
    state.corpus.store(25, Ordering::Release);
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.grpc_addr().expect("bound")).await;
    let reply = grpc_call(
        &mut send,
        STREAM,
        &[],
        grpc_frame(0, &search_request(stream_body(10))),
    )
    .await
    .expect("reply");
    assert_eq!(reply.grpc_status, Some(0), "{:?}", reply.grpc_message);
    let pages: Vec<Vec<String>> = reply.messages.iter().map(|m| ids(m)).collect();
    let all: Vec<String> = (1..=25).map(|i| i.to_string()).collect();
    assert_eq!(
        pages.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![10, 10, 5]
    );
    assert_eq!(pages.concat(), all);
    let seen = state.searches.lock().expect("lock").clone();
    assert_eq!(seen.len(), 3);
    let body =
        |i: usize| -> serde_json::Value { serde_json::from_slice(&seen[i].2).expect("json") };
    assert_eq!(body(0), serde_json::json!({"size": 11, "sort": ["n"]}));
    assert_eq!(body(1)["search_after"], serde_json::json!([10]));
    assert_eq!(body(2)["search_after"], serde_json::json!([20]));
    // A result that fits one page is one message, and an empty one is one empty message.
    state.searches.lock().expect("lock").clear();
    let reply = grpc_call(
        &mut send,
        STREAM,
        &[],
        grpc_frame(0, &search_request(stream_body(25))),
    )
    .await
    .expect("reply");
    assert_eq!(reply.messages.len(), 1);
    assert_eq!(state.searches.lock().expect("lock").len(), 1);
    state.corpus.store(0, Ordering::Release);
    *state.search_answer.lock().expect("lock") = NO_HITS.to_owned();
    let reply = grpc_call(
        &mut send,
        STREAM,
        &[],
        grpc_frame(0, &search_request(stream_body(10))),
    )
    .await
    .expect("reply");
    assert_eq!((reply.grpc_status, reply.messages.len()), (Some(0), 1));
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tie_across_a_page_boundary_ends_the_stream_instead_of_skipping_hits() {
    let state = Arc::new(State::default());
    state.corpus.store(40, Ordering::Release);
    state.tie_at.store(20, Ordering::Release);
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.grpc_addr().expect("bound")).await;
    let reply = grpc_call(
        &mut send,
        STREAM,
        &[],
        grpc_frame(0, &search_request(stream_body(10))),
    )
    .await
    .expect("reply");
    assert_eq!(reply.grpc_status, Some(9), "{:?}", reply.grpc_message);
    assert_eq!(
        reply.messages.len(),
        1,
        "the first page, whose boundary was clean, was delivered"
    );
    assert!(reply.grpc_message.expect("message").contains("unique"));
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stream_search_needs_a_sort_and_takes_no_from() {
    let state = Arc::new(State::default());
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.grpc_addr().expect("bound")).await;
    for (body, code) in [
        (serde_json::json!({"size": 5}), 3),
        (
            serde_json::json!({"size": 5, "from": 2, "sort": [{"field": "n"}]}),
            12,
        ),
        (serde_json::json!({"size": 0, "sort": [{"field": "n"}]}), 3),
        (
            serde_json::json!({"size": 10001, "sort": [{"field": "n"}]}),
            3,
        ),
    ] {
        let reply = grpc_call(
            &mut send,
            STREAM,
            &[],
            grpc_frame(0, &search_request(body.clone())),
        )
        .await
        .expect("reply");
        assert_eq!(
            reply.grpc_status,
            Some(code),
            "{body}: {:?}",
            reply.grpc_message
        );
    }
    assert!(state.searches.lock().expect("lock").is_empty());
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stream_search_past_its_deadline_ends_with_deadline_exceeded() {
    let state = Arc::new(State::default());
    state.corpus.store(25, Ordering::Release);
    state.search_delay_ms.store(400, Ordering::Release);
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    let mut send = h2c(edge.grpc_addr().expect("bound")).await;
    let started = Instant::now();
    let reply = grpc_call(
        &mut send,
        STREAM,
        &[("grpc-timeout", "500m")],
        grpc_frame(0, &search_request(stream_body(10))),
    )
    .await
    .expect("reply");
    // The first page took 400 ms, the second would reach 800: one message, then the deadline.
    assert_eq!(reply.grpc_status, Some(4), "{:?}", reply.grpc_message);
    assert_eq!(reply.messages.len(), 1);
    assert!(started.elapsed() < Duration::from_secs(2));
    edge.shutdown();
    server.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_page_answers_end_the_stream_as_internal() {
    let state = Arc::new(State::default());
    state.corpus.store(25, Ordering::Release);
    let (server, budget, upstream) = start(&state);
    let edge = Edge::start(config(upstream), &budget).expect("starts");
    // An answer that is not JSON where a page was expected: INTERNAL, before any message.
    state.corpus.store(0, Ordering::Release);
    *state.search_answer.lock().expect("lock") = "not json".to_owned();
    let mut send = h2c(edge.grpc_addr().expect("bound")).await;
    let reply = grpc_call(
        &mut send,
        STREAM,
        &[],
        grpc_frame(0, &search_request(stream_body(10))),
    )
    .await
    .expect("reply");
    assert_eq!((reply.grpc_status, reply.messages.len()), (Some(13), 0));
    // JSON without hits, and a hit without sort values.
    for answer in [
        r#"{"took":1}"#,
        r#"{"hits":{"hits":[{"_id":"1"},{"_id":"2"}]}}"#,
    ] {
        *state.search_answer.lock().expect("lock") = answer.to_owned();
        let reply = grpc_call(
            &mut send,
            STREAM,
            &[],
            grpc_frame(0, &search_request(stream_body(1))),
        )
        .await
        .expect("reply");
        assert_eq!(reply.grpc_status, Some(13), "{answer}");
    }
    edge.shutdown();
    server.shutdown();
}
