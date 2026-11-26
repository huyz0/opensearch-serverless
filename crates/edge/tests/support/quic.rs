//! A throwaway certificate and an HTTP/3 client for the tests.

#![allow(dead_code, missing_docs)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use bytes::{Buf, Bytes};
use quinn::crypto::rustls::QuicClientConfig;
use rustls::{ClientConfig, RootCertStore};

use super::Answer;

pub struct Pair {
    pub cert: PathBuf,
    pub key: PathBuf,
    pub der: rustls::pki_types::CertificateDer<'static>,
}

pub fn pair(tag: &str) -> Pair {
    let made = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).expect("cert");
    let dir = std::env::temp_dir().join(format!("oss-edge-h3-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
    std::fs::write(&cert, made.cert.pem()).expect("cert file");
    std::fs::write(&key, made.key_pair.serialize_pem()).expect("key file");
    Pair {
        cert,
        key,
        der: made.cert.der().clone(),
    }
}

/// A QUIC client endpoint trusting `p`, offering `alpn`.
pub fn endpoint(p: &Pair, alpn: &[&[u8]]) -> quinn::Endpoint {
    let mut roots = RootCertStore::empty();
    roots.add(p.der.clone()).expect("root");
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut c = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    c.alpn_protocols = alpn.iter().map(|a| a.to_vec()).collect();
    c.enable_early_data = true;
    let mut e = quinn::Endpoint::client("127.0.0.1:0".parse().expect("addr")).expect("endpoint");
    e.set_default_client_config(quinn::ClientConfig::new(Arc::new(
        QuicClientConfig::try_from(c).expect("quic client"),
    )));
    e
}

pub type Send = h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>;

/// An HTTP/3 connection to `addr`; the driver runs in the background.
pub async fn connect(endpoint: &quinn::Endpoint, addr: SocketAddr) -> (quinn::Connection, Send) {
    let conn = endpoint
        .connect(addr, "localhost")
        .expect("connecting")
        .await
        .expect("connected");
    let (mut driver, send) = h3::client::new(h3_quinn::Connection::new(conn.clone()))
        .await
        .expect("h3");
    tokio::spawn(async move {
        let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
    });
    (conn, send)
}

/// One request; a `None` body sends no DATA.
pub async fn request(
    send: &mut Send,
    method: &str,
    uri: &str,
    body: Option<Bytes>,
) -> Result<Answer, String> {
    request_with(send, method, uri, &[], body).await
}

/// As [`request`], with extra headers; returns the answer and its headers.
pub async fn request_with(
    send: &mut Send,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<Bytes>,
) -> Result<Answer, String> {
    let (answer, _) = request_full(send, method, uri, headers, body).await?;
    Ok(answer)
}

pub async fn request_full(
    send: &mut Send,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<Bytes>,
) -> Result<(Answer, http::HeaderMap), String> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    for (k, v) in headers {
        builder = builder.header(*k, *v);
    }
    let mut stream = send
        .send_request(builder.body(()).expect("request"))
        .await
        .map_err(|e| e.to_string())?;
    if let Some(body) = body {
        stream.send_data(body).await.map_err(|e| e.to_string())?;
    }
    stream.finish().await.map_err(|e| e.to_string())?;
    let response = stream.recv_response().await.map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .map(|v| v.to_str().expect("text").to_ascii_lowercase());
    let headers = response.headers().clone();
    let mut out = Vec::new();
    while let Some(mut data) = stream.recv_data().await.map_err(|e| e.to_string())? {
        let n = data.remaining();
        out.extend_from_slice(&data.copy_to_bytes(n));
    }
    Ok((
        Answer {
            status,
            content_type,
            body: out,
        },
        headers,
    ))
}
