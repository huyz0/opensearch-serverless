//! docs/specs/grpc-front: gRPC calls against a booted node, each compared with the REST call
//! it stands for, on the same node.

// ARITH: the base64 helper shifts small values.
#![allow(clippy::arithmetic_side_effects)]
#![cfg(all(
    feature = "edge",
    feature = "reader",
    feature = "writer",
    feature = "coordinator"
))]

use crate::support;

use bytes::Bytes;
use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;
use prost::Message;
use prost_reflect::DynamicMessage;
use serde_json::{Value, json};

const DOCUMENT: &str = "/org.opensearch.protobufs.services.DocumentService/Bulk";
const SEARCH: &str = "/org.opensearch.protobufs.services.SearchService/Search";

fn boot(root: &std::path::Path) -> Node {
    let root = root.display().to_string();
    let config = NodeConfig::from_lookup(|key| match key {
        "OSS_NODE_ID" => Some("n1".to_owned()),
        "OSS_STORE" => Some("fs".to_owned()),
        "OSS_FS_ROOT" => Some(root.clone()),
        "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" | "OSS_EDGE_GRPC_ADDR" => {
            Some("127.0.0.1:0".to_owned())
        }
        _ => None,
    })
    .expect("valid");
    start(&config).expect("starts")
}

fn rest(node: &Node, method: &str, target: &str, body: Option<&str>) -> (u16, Value) {
    let reply = support::request(node.addr(), method, target, body.map(str::as_bytes));
    let json = serde_json::from_str(&reply.text()).unwrap_or(Value::Null);
    (reply.status, json)
}

/// One unary call: `request` is protojson of `request_type`; the answer is protojson, or the
/// `grpc-status` and message.
fn grpc(
    node: &Node,
    path: &str,
    request_type: &str,
    response_type: &str,
    request: &Value,
) -> Result<Value, (u8, String)> {
    let desc = oss_edge::proto_message(request_type);
    let msg = DynamicMessage::deserialize(desc, request).expect("protojson request");
    let encoded = msg.encode_to_vec();
    let mut framed = vec![0u8];
    framed.extend_from_slice(&u32::try_from(encoded.len()).expect("fits").to_be_bytes());
    framed.extend_from_slice(&encoded);
    let addr = node.grpc_addr().expect("a grpc listener");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let (status, message, raw) = rt.block_on(async {
        let tcp = tokio::net::TcpStream::connect(addr)
            .await
            .expect("connects");
        let (mut send, conn) = h2::client::handshake(tcp).await.expect("handshake");
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("http://node{path}"))
            .header("content-type", "application/grpc")
            .header("te", "trailers")
            .body(())
            .expect("request");
        let (response, mut stream) = send.send_request(request, false).expect("sent");
        stream.send_data(Bytes::from(framed), true).expect("body");
        let response = response.await.expect("response");
        let text = |h: &http::HeaderMap, n: &str| {
            h.get(n).and_then(|v| v.to_str().ok()).map(str::to_owned)
        };
        let mut status = text(response.headers(), "grpc-status");
        let mut message = text(response.headers(), "grpc-message");
        let mut body = response.into_body();
        let mut raw = Vec::new();
        while let Some(data) = body.data().await {
            let data = data.expect("data");
            body.flow_control()
                .release_capacity(data.len())
                .expect("credit");
            raw.extend_from_slice(&data);
        }
        if let Some(t) = body.trailers().await.expect("trailers") {
            status = text(&t, "grpc-status");
            message = text(&t, "grpc-message");
        }
        (status, message, raw)
    });
    let code: u8 = status.expect("a grpc-status").parse().expect("number");
    if code != 0 {
        return Err((code, message.unwrap_or_default()));
    }
    let body = raw.get(5..).expect("one message");
    let reply =
        DynamicMessage::decode(oss_edge::proto_message(response_type), body).expect("decodes");
    Ok(serde_json::to_value(&reply).expect("protojson"))
}

fn index(node: &Node, name: &str) {
    assert_eq!(rest(node, "PUT", &format!("/{name}"), None).0, 200);
}

// AC-3
#[test]
fn bulk_over_grpc_answers_what_rest_bulk_answers() {
    let dir = TempDir::new("grpc-bulk");
    let node = boot(dir.path());
    index(&node, "viag");
    index(&node, "viar");
    let (status, answer) = rest(
        &node,
        "POST",
        "/viar/_bulk?refresh=true",
        Some(concat!(
            "{\"index\":{\"_id\":\"1\"}}\n{\"n\":1,\"t\":\"alpha\"}\n",
            "{\"create\":{\"_id\":\"2\"}}\n{\"n\":2,\"t\":\"beta\"}\n",
            "{\"create\":{\"_id\":\"2\"}}\n{\"n\":2,\"t\":\"beta\"}\n",
            "{\"update\":{\"_id\":\"1\"}}\n{\"doc\":{\"n\":10}}\n",
            "{\"delete\":{\"_id\":\"2\"}}\n",
        )),
    );
    assert_eq!(status, 200, "{answer}");
    let object = |s: &str| -> String {
        use base64_free::encode;
        encode(s.as_bytes())
    };
    let item = |op: Value, extra: Value| {
        let mut o = json!({"operationContainer": op});
        for (k, v) in extra.as_object().expect("object") {
            o[k] = v.clone();
        }
        o
    };
    let request = json!({
        "index": "viag",
        "refresh": "REFRESH_TRUE",
        "bulkRequestBody": [
            item(json!({"index": {"xId": "1"}}), json!({"object": object("{\"n\":1,\"t\":\"alpha\"}")})),
            item(json!({"create": {"xId": "2"}}), json!({"object": object("{\"n\":2,\"t\":\"beta\"}")})),
            item(json!({"create": {"xId": "2"}}), json!({"object": object("{\"n\":2,\"t\":\"beta\"}")})),
            item(json!({"update": {"xId": "1"}}), json!({"updateAction": {"doc": object("{\"n\":10}")}})),
            item(json!({"delete": {"xId": "2"}}), json!({})),
        ]
    });
    let got = grpc(&node, DOCUMENT, "BulkRequest", "BulkResponse", &request).expect("answers");
    assert_eq!(got["errors"], answer["errors"]);
    let rest_items = answer["items"].as_array().expect("items");
    let grpc_items = got["items"].as_array().expect("items");
    assert_eq!(rest_items.len(), grpc_items.len());
    for (r, g) in rest_items.iter().zip(grpc_items) {
        let (op, r) = r.as_object().expect("item").iter().next().expect("one op");
        let g = &g[op];
        assert_eq!(
            g["status"].as_i64(),
            r["status"].as_i64(),
            "{op}: {g} vs {r}"
        );
        assert_eq!(g["result"], r["result"], "{op}");
        assert_eq!(g["xId"], r["_id"], "{op}");
        assert_eq!(
            g["xVersion"].as_str().map(|v| v.parse::<i64>().expect("n")),
            r["_version"].as_i64()
        );
        assert_eq!(r["_index"], "viar");
        assert_eq!(g["xIndex"], "viag");
    }
    // What the gRPC call wrote is readable, as what the REST call wrote is.
    let (_, via_rest) = rest(&node, "GET", "/viar/_doc/1", None);
    let (_, via_grpc) = rest(&node, "GET", "/viag/_doc/1", None);
    assert_eq!(via_grpc["_source"], via_rest["_source"]);
    assert!(via_grpc["_source"].is_object(), "{via_grpc}");
}

/// Base64 for the request bytes, without a dependency for a test helper.
mod base64_free {
    pub fn encode(input: &[u8]) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in input.chunks(3) {
            let n =
                chunk.iter().fold(0u32, |a, b| (a << 8) | u32::from(*b)) << (8 * (3 - chunk.len()));
            for i in 0..=chunk.len() {
                out.push(char::from(T[((n >> (18 - 6 * i)) & 63) as usize]));
            }
            for _ in chunk.len()..3 {
                out.push('=');
            }
        }
        out
    }
}

fn ids_of_rest(answer: &Value) -> Vec<String> {
    answer["hits"]["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|h| h["_id"].as_str().expect("id").to_owned())
        .collect()
}

fn ids_of_grpc(answer: &Value) -> Vec<String> {
    answer["hits"]["hits"]
        .as_array()
        .map(|hits| {
            hits.iter()
                .map(|h| h["xId"].as_str().expect("id").to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// The covered query types, each as the REST body and as the protojson body; the table is read by
/// the test below so a row cannot be removed without its type leaving the covered list.
fn query_table() -> Vec<(&'static str, Value, Value)> {
    let term = |f: &str, v: &str| json!({"term": {"field": f, "value": {"string": v}}});
    vec![
        (
            "match_all",
            json!({"match_all": {}}),
            json!({"matchAll": {}}),
        ),
        (
            "term",
            json!({"term": {"t": "alpha"}}),
            json!({"term": {"field": "t", "value": {"string": "alpha"}}}),
        ),
        (
            "terms",
            json!({"terms": {"t": ["alpha", "gamma"]}}),
            json!({"terms": {"terms": {"t": {"value": {"fieldValueArray": [{"string": "alpha"}, {"string": "gamma"}]}}}}}),
        ),
        (
            "match",
            json!({"match": {"t": "beta"}}),
            json!({"match": {"field": "t", "query": {"string": "beta"}}}),
        ),
        (
            "match_phrase",
            json!({"match_phrase": {"t": "delta echo"}}),
            json!({"matchPhrase": {"field": "t", "query": "delta echo"}}),
        ),
        (
            "range",
            json!({"range": {"n": {"gte": 2, "lt": 5}}}),
            json!({"range": {"numberRangeQuery": {"field": "n", "gte": 2.0, "lt": 5.0}}}),
        ),
        (
            "exists",
            json!({"exists": {"field": "n"}}),
            json!({"exists": {"field": "n"}}),
        ),
        (
            "ids",
            json!({"ids": {"values": ["2", "4"]}}),
            json!({"ids": {"values": ["2", "4"]}}),
        ),
        (
            "prefix",
            json!({"prefix": {"t": "al"}}),
            json!({"prefix": {"field": "t", "value": "al"}}),
        ),
        (
            "bool",
            json!({"bool": {"must": [{"range": {"n": {"gte": 1}}}], "must_not": [{"term": {"t": "beta"}}],
                            "should": [{"term": {"t": "alpha"}}, {"term": {"t": "gamma"}}], "minimum_should_match": 1}}),
            json!({"bool": {"must": [{"range": {"numberRangeQuery": {"field": "n", "gte": 1.0}}}],
                            "mustNot": [term("t", "beta")],
                            "should": [term("t", "alpha"), term("t", "gamma")], "minimumShouldMatch": {"int32": 1}}}),
        ),
    ]
}

// AC-4
#[test]
fn search_over_grpc_answers_the_same_hits_as_rest_search() {
    let dir = TempDir::new("grpc-search");
    let node = boot(dir.path());
    index(&node, "docs");
    let docs = [
        ("1", json!({"n": 1, "t": "alpha"})),
        ("2", json!({"n": 2, "t": "beta"})),
        ("3", json!({"n": 3, "t": "gamma"})),
        ("4", json!({"n": 4, "t": "delta echo"})),
        ("5", json!({"n": 5, "t": "alpha beta"})),
    ];
    for (id, doc) in &docs {
        let put = rest(
            &node,
            "PUT",
            &format!("/docs/_doc/{id}?refresh=true"),
            Some(&doc.to_string()),
        );
        assert_eq!(put.0, 201, "{}", put.1);
    }
    let mut table: Vec<&str> = query_table().iter().map(|(n, _, _)| *n).collect();
    let mut covered = oss_edge::COVERED_QUERIES.to_vec();
    table.sort_unstable();
    covered.sort_unstable();
    assert_eq!(table, covered, "a query type and its table row go together");
    for (name, rest_query, grpc_query) in query_table() {
        let (status, via_rest) = rest(
            &node,
            "POST",
            "/docs/_search",
            Some(&json!({"query": rest_query, "sort": [{"n": "asc"}], "size": 10}).to_string()),
        );
        assert_eq!(status, 200, "{name}: {via_rest}");
        let request = json!({
            "index": ["docs"],
            "searchRequestBody": {
                "query": grpc_query,
                "sort": [{"fieldWithOrder": {"fieldSortMap": {"n": {"order": "SORT_ORDER_ASC"}}}}],
                "size": 10
            }
        });
        let via_grpc = grpc(&node, SEARCH, "SearchRequest", "SearchResponse", &request)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(ids_of_grpc(&via_grpc), ids_of_rest(&via_rest), "{name}");
        assert!(
            !ids_of_rest(&via_rest).is_empty() || name == "never",
            "{name} matches something"
        );
    }
}
