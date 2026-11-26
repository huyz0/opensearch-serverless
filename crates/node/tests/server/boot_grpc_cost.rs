//! docs/specs/grpc-front AC-15: a gRPC call makes the blob requests its REST twin makes, counted
//! by the meter of an in-process S3 emulator behind a booted node.

#![cfg(all(
    feature = "edge",
    feature = "reader",
    feature = "writer",
    feature = "coordinator"
))]

use std::sync::Arc;
use std::time::Duration;

use oss_blob::emulator::http::S3Server;
use oss_blob::s3::sigv4::Credentials;
use oss_blob::{BlobStore, Emulator, S3Op};
use oss_node::boot::{Node, NodeConfig, start};
use serde_json::{Value, json};

use super::boot_grpc::{DOCUMENT, SEARCH, base64_free, grpc, index, rest};

const BUCKET: &str = "grpccost";
const REGION: &str = "us-east-1";
const ACCESS: &str = "grpcaccess";
const SECRET: &str = "grpcsecret";

fn boot(endpoint: &str) -> Node {
    let config = NodeConfig::from_lookup(|key| {
        Some(
            match key {
                "OSS_NODE_ID" => "n1",
                "OSS_STORE" => "s3",
                "OSS_S3_ENDPOINT" => endpoint,
                "OSS_S3_BUCKET" => BUCKET,
                "OSS_S3_REGION" => REGION,
                "OSS_S3_ACCESS_KEY" => ACCESS,
                "OSS_S3_SECRET_KEY" => SECRET,
                // A lease renewal inside a measured window is two PUTs that no call made.
                "OSS_LEASE_TTL_MS" => "3600000",
                // Nor may an index's periodic re-confirmation.
                "OSS_INDEX_CONFIRM_MS" => "3600000",
                "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" | "OSS_EDGE_GRPC_ADDR" => "127.0.0.1:0",
                _ => return None,
            }
            .to_owned(),
        )
    })
    .expect("valid");
    start(&config).expect("starts")
}

/// Requests of every kind the emulator has counted, in `S3Op::ALL` order.
fn counts(server: &S3Server) -> Vec<u64> {
    S3Op::ALL
        .iter()
        .map(|op| server.store().meter().count(*op))
        .collect()
}

/// Waits until the node has made no blob request for `idle`, so background work a call left
/// behind (a lazy publish, a refresh) does not land in a later measured window.
fn quiet(server: &S3Server, idle: Duration) {
    const STEP: Duration = Duration::from_millis(50);
    let mut last = counts(server);
    let mut still = Duration::ZERO;
    while still < idle {
        std::thread::sleep(STEP);
        let now = counts(server);
        still = if now == last {
            still.saturating_add(STEP)
        } else {
            Duration::ZERO
        };
        last = now;
    }
}

fn spent(server: &S3Server, work: impl FnOnce()) -> Vec<u64> {
    quiet(server, Duration::from_millis(250));
    let before = counts(server);
    work();
    counts(server)
        .iter()
        .zip(&before)
        .map(|(after, before)| after.saturating_sub(*before))
        .collect()
}

// AC-15
#[test]
fn a_grpc_bulk_and_search_make_the_blob_requests_their_rest_twins_make() {
    let server = S3Server::start(
        Arc::new(Emulator::new(1 << 28, 9)),
        BUCKET,
        REGION,
        Credentials::new(ACCESS, SECRET),
    )
    .expect("the S3 front end");
    let node = boot(&server.endpoint());
    for name in ["costw", "costr", "costg"] {
        index(&node, name);
        let seed = rest(
            &node,
            "PUT",
            &format!("/{name}/_doc/0?refresh=true"),
            Some("{\"n\":0,\"t\":\"seed\"}"),
        );
        assert_eq!(seed.0, 201, "{}", seed.1);
    }
    // The node's first search pays once for what it keeps afterwards; a third index takes it.
    let warm = json!({"query": {"match": {"t": "seed"}}}).to_string();
    assert_eq!(rest(&node, "POST", "/costw/_search", Some(&warm)).0, 200);
    let object = |s: &str| base64_free::encode(s.as_bytes());

    let rest_bulk = spent(&server, || {
        let body = concat!(
            "{\"index\":{\"_id\":\"1\"}}\n{\"n\":1,\"t\":\"alpha\"}\n",
            "{\"create\":{\"_id\":\"2\"}}\n{\"n\":2,\"t\":\"beta\"}\n",
            "{\"delete\":{\"_id\":\"0\"}}\n",
        );
        let (status, answer) = rest(&node, "POST", "/costr/_bulk?refresh=true", Some(body));
        assert_eq!(status, 200, "{answer}");
    });
    let grpc_bulk = spent(&server, || {
        let request = json!({
            "index": "costg",
            "refresh": "REFRESH_TRUE",
            "bulkRequestBody": [
                {"operationContainer": {"index": {"xId": "1"}}, "object": object("{\"n\":1,\"t\":\"alpha\"}")},
                {"operationContainer": {"create": {"xId": "2"}}, "object": object("{\"n\":2,\"t\":\"beta\"}")},
                {"operationContainer": {"delete": {"xId": "0"}}},
            ]
        });
        let got = grpc(&node, DOCUMENT, "BulkRequest", "BulkResponse", &request).expect("answers");
        assert!(got["errors"].is_null(), "no item failed: {got}");
    });
    assert!(rest_bulk.iter().sum::<u64>() > 0, "the bulk wrote blobs");
    assert_eq!(grpc_bulk, rest_bulk, "bulk, in S3Op::ALL order");

    // The writer publishes a write a second after it (timer pass every 250 ms), and the first
    // search after that publish opens the new commit: the descriptor, the manifest and the
    // segment, 2 GET and 1 ranged GET more, whichever front end sent it. Let the publishes
    // land, then search each index once over REST, so the windows hold what a search costs
    // once the node is caught up.
    quiet(&server, Duration::from_millis(1_600));
    for name in ["costr", "costg"] {
        let (status, answer) = rest(&node, "POST", &format!("/{name}/_search"), Some(&warm));
        assert_eq!(status, 200, "{answer}");
    }

    let rest_search = spent(&server, || {
        let body = json!({"query": {"match": {"t": "alpha"}}, "size": 10}).to_string();
        let (status, answer) = rest(&node, "POST", "/costr/_search", Some(&body));
        assert_eq!(status, 200, "{answer}");
    });
    let grpc_search = spent(&server, || {
        let request = json!({
            "index": ["costg"],
            "searchRequestBody": {"query": {"match": {"field": "t", "query": {"string": "alpha"}}}, "size": 10}
        });
        let got: Value =
            grpc(&node, SEARCH, "SearchRequest", "SearchResponse", &request).expect("answers");
        assert!(got["hits"].is_object(), "{got}");
    });
    assert_eq!(grpc_search, rest_search, "search, in S3Op::ALL order");
}
