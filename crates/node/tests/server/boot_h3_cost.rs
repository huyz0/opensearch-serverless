//! docs/specs/http3-front AC-13: a request over HTTP/3 makes the blob requests its HTTP/1.1 twin
//! makes, counted by the meter of an in-process S3 emulator behind a booted node.

#![cfg(all(
    feature = "edge",
    feature = "reader",
    feature = "writer",
    feature = "coordinator"
))]

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::{Buf, Bytes};
use oss_blob::emulator::http::S3Server;
use oss_blob::s3::sigv4::Credentials;
use oss_blob::{BlobStore, Emulator, S3Op};
use oss_node::boot::{Node, NodeConfig, start};
use oss_testkit::TempDir;
use serde_json::Value;

use super::boot_grpc::rest;

const BUCKET: &str = "h3cost";
const REGION: &str = "us-east-1";
const ACCESS: &str = "h3access";
const SECRET: &str = "h3secret";

fn boot(endpoint: &str, cert: &std::path::Path, key: &std::path::Path) -> Node {
    let (cert, key) = (cert.display().to_string(), key.display().to_string());
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
                "OSS_EDGE_TLS_CERT" => cert.as_str(),
                "OSS_EDGE_TLS_KEY" => key.as_str(),
                "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" | "OSS_EDGE_H3_ADDR" => "127.0.0.1:0",
                _ => return None,
            }
            .to_owned(),
        )
    })
    .expect("valid");
    start(&config).expect("starts")
}

/// One request over HTTP/3 to `addr`, on its own connection.
fn h3(
    addr: SocketAddr,
    root: &rustls::pki_types::CertificateDer<'static>,
    method: &str,
    target: &str,
    body: &str,
) -> (u16, Value) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(root.clone()).expect("root");
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut crypto = rustls::ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .expect("versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
        crypto.alpn_protocols = vec![b"h3".to_vec()];
        let mut endpoint =
            quinn::Endpoint::client("127.0.0.1:0".parse().expect("addr")).expect("endpoint");
        endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(crypto).expect("quic"),
        )));
        let conn = endpoint
            .connect(addr, "localhost")
            .expect("connecting")
            .await
            .expect("connected");
        let (mut driver, mut send) = h3::client::new(h3_quinn::Connection::new(conn))
            .await
            .expect("h3");
        tokio::spawn(async move {
            let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://localhost{target}"))
            .header("content-type", "application/json")
            .body(())
            .expect("request");
        let mut stream = send.send_request(request).await.expect("sent");
        if !body.is_empty() {
            stream
                .send_data(Bytes::copy_from_slice(body.as_bytes()))
                .await
                .expect("body");
        }
        stream.finish().await.expect("finished");
        let response = stream.recv_response().await.expect("response");
        let mut out = Vec::new();
        while let Some(mut data) = stream.recv_data().await.expect("data") {
            let n = data.remaining();
            out.extend_from_slice(&data.copy_to_bytes(n));
        }
        (
            response.status().as_u16(),
            serde_json::from_slice(&out).unwrap_or(Value::Null),
        )
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
fn an_http3_bulk_and_search_make_the_blob_requests_their_http1_twins_make() {
    let server = S3Server::start(
        Arc::new(Emulator::new(1 << 28, 9)),
        BUCKET,
        REGION,
        Credentials::new(ACCESS, SECRET),
    )
    .expect("the S3 front end");
    let dir = TempDir::new("h3-cost");
    let made = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).expect("cert");
    let (cert, key) = (dir.path().join("cert.pem"), dir.path().join("key.pem"));
    std::fs::write(&cert, made.cert.pem()).expect("cert file");
    std::fs::write(&key, made.key_pair.serialize_pem()).expect("key file");
    let node = boot(&server.endpoint(), &cert, &key);
    let addr = node.h3_addr().expect("an h3 listener");
    let root = made.cert.der().clone();

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
    let over3_bulk = spent(&server, || {
        let (status, answer) = h3(addr, &root, "POST", "/costq/_bulk?refresh=true", bulk);
        assert_eq!(status, 200, "{answer}");
    });
    assert!(over1_bulk.iter().sum::<u64>() > 0, "the bulk wrote blobs");
    assert_eq!(over3_bulk, over1_bulk, "bulk, in S3Op::ALL order");

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
    let over3_search = spent(&server, || {
        let (status, answer) = h3(addr, &root, "POST", "/costq/_search", query);
        assert_eq!(status, 200, "{answer}");
        assert!(answer["hits"].is_object(), "{answer}");
    });
    assert_eq!(over3_search, over1_search, "search, in S3Op::ALL order");
}
