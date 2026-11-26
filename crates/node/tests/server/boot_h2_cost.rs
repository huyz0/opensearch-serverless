//! docs/specs/http2-front AC-11: a request over clear-text HTTP/2 makes the blob requests its HTTP/1.1 twin
//! makes, counted by the meter of an in-process S3 emulator behind a booted node.

#![cfg(all(
    feature = "edge",
    feature = "reader",
    feature = "writer",
    feature = "coordinator"
))]

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use oss_blob::emulator::http::S3Server;
use oss_blob::s3::sigv4::Credentials;
use oss_blob::{BlobStore, Emulator, S3Op};
use oss_node::boot::{Node, NodeConfig, start};
use serde_json::Value;

use super::boot_grpc::rest;

const BUCKET: &str = "h2cost";
const REGION: &str = "us-east-1";
const ACCESS: &str = "h2access";
const SECRET: &str = "h2secret";

fn boot(endpoint: &str) -> Node {
    let config = NodeConfig::from_lookup(|key_name| {
        Some(
            match key_name {
                "OSS_NODE_ID" => "n1",
                "OSS_STORE" => "s3",
                "OSS_S3_ENDPOINT" => endpoint,
                "OSS_S3_BUCKET" => BUCKET,
                "OSS_S3_REGION" => REGION,
                "OSS_S3_ACCESS_KEY" => ACCESS,
                "OSS_S3_SECRET_KEY" => SECRET,
                "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" | "OSS_EDGE_H2C_ADDR" => "127.0.0.1:0",
                _ => return None,
            }
            .to_owned(),
        )
    })
    .expect("valid");
    start(&config).expect("starts")
}

/// One request over clear-text HTTP/2 to `addr`, on its own connection.
fn h2(addr: SocketAddr, method: &str, target: &str, body: &str) -> (u16, Value) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let tcp = tokio::net::TcpStream::connect(addr)
            .await
            .expect("connects");
        let (mut send, conn) = h2::client::handshake(tcp).await.expect("h2");
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("http://localhost{target}"))
            .header("content-type", "application/json")
            .body(())
            .expect("request");
        let (response, mut stream) = send.send_request(request, body.is_empty()).expect("sent");
        if !body.is_empty() {
            stream
                .send_data(Bytes::copy_from_slice(body.as_bytes()), true)
                .expect("body");
        }
        let response = response.await.expect("response");
        let status = response.status().as_u16();
        let mut recv = response.into_body();
        let mut out = Vec::new();
        while let Some(data) = recv.data().await {
            let data = data.expect("data");
            let _ = recv.flow_control().release_capacity(data.len());
            out.extend_from_slice(&data);
        }
        (status, serde_json::from_slice(&out).unwrap_or(Value::Null))
    })
}

fn counts(server: &S3Server) -> Vec<u64> {
    S3Op::ALL
        .iter()
        .map(|op| server.store().meter().count(*op))
        .collect()
}

fn spent(server: &S3Server, work: impl FnOnce()) -> Vec<u64> {
    let before = counts(server);
    work();
    counts(server)
        .iter()
        .zip(&before)
        .map(|(after, before)| after.saturating_sub(*before))
        .collect()
}

// AC-13
#[test]
fn an_http2_bulk_and_search_make_the_blob_requests_their_http1_twins_make() {
    let server = S3Server::start(
        Arc::new(Emulator::new(1 << 28, 9)),
        BUCKET,
        REGION,
        Credentials::new(ACCESS, SECRET),
    )
    .expect("the S3 front end");
    let node = boot(&server.endpoint());
    let addr = node.h2c_addr().expect("an h2c listener");

    for name in ["costw", "costr", "costq"] {
        assert_eq!(rest(&node, "PUT", &format!("/{name}"), None).0, 200);
        let seed = rest(
            &node,
            "PUT",
            &format!("/{name}/_doc/0?refresh=true"),
            Some("{\"n\":0,\"t\":\"seed\"}"),
        );
        assert_eq!(seed.0, 201, "{}", seed.1);
    }
    // The node's first search pays once for what it keeps afterwards; a third index takes it.
    let warm = "{\"query\":{\"match\":{\"t\":\"seed\"}}}";
    assert_eq!(rest(&node, "POST", "/costw/_search", Some(warm)).0, 200);

    let bulk = "{\"index\":{\"_id\":\"1\"}}\n{\"n\":1,\"t\":\"alpha\"}\n{\"create\":{\"_id\":\"2\"}}\n{\"n\":2,\"t\":\"beta\"}\n{\"delete\":{\"_id\":\"0\"}}\n";
    let over1_bulk = spent(&server, || {
        let (status, answer) = rest(&node, "POST", "/costr/_bulk?refresh=true", Some(bulk));
        assert_eq!(status, 200, "{answer}");
    });
    let over2_bulk = spent(&server, || {
        let (status, answer) = h2(addr, "POST", "/costq/_bulk?refresh=true", bulk);
        assert_eq!(status, 200, "{answer}");
    });
    assert!(over1_bulk.iter().sum::<u64>() > 0, "the bulk wrote blobs");
    assert_eq!(over2_bulk, over1_bulk, "bulk, in S3Op::ALL order");

    let query = "{\"query\":{\"match\":{\"t\":\"alpha\"}},\"size\":10}";
    // Each index pays its first search's one-time cost here, so the two measured searches are
    // both steady-state and the order they run in does not matter.
    for name in ["costr", "costq"] {
        for _ in 0..2 {
            assert_eq!(
                rest(&node, "POST", &format!("/{name}/_search"), Some(query)).0,
                200
            );
        }
    }
    let over1_search = spent(&server, || {
        let (status, answer) = rest(&node, "POST", "/costr/_search", Some(query));
        assert_eq!(status, 200, "{answer}");
    });
    let over2_search = spent(&server, || {
        let (status, answer) = h2(addr, "POST", "/costq/_search", query);
        assert_eq!(status, 200, "{answer}");
        assert!(answer["hits"].is_object(), "{answer}");
    });
    assert_eq!(over2_search, over1_search, "search, in S3Op::ALL order");
}
