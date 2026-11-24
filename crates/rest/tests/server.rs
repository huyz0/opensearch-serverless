//! Routing, parameters and error shapes, over real sockets.

mod support;

use std::io::Write;

use oss_rest::{Exception, JSON, Length, Meta, Method, Request, Response, RestError, Router};
use serde_json::json;
use support::{Client, get, post, small, start};

fn ok(req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
    let index = req.path_param("index").map(|s| s.into_owned());
    let q = req.param("q").map(|s| s.into_owned());
    let host = req.header("HOST").map(str::to_owned);
    let body = json!({"index": index, "q": q, "method": req.method().as_str(), "raw": req.raw_path(), "host": host});
    resp.send(200, JSON, body.to_string().as_bytes())
}

fn exists(req: &mut Request<'_>, _: &mut Response<'_>) -> Result<(), RestError> {
    // The values the recorded fixture carries.
    let index = req
        .path_param("index")
        .map(|s| s.into_owned())
        .unwrap_or_default();
    let uuid = "oIemTVv4R3qQQhoAU-q9_g";
    Err(RestError::exception(
        400,
        Exception::new(
            "resource_already_exists_exception",
            format!("index [{index}/{uuid}] already exists"),
        )
        .with_meta("index", Meta::Str(index))
        .with_meta("index_uuid", Meta::Str(uuid.to_owned())),
    ))
}

fn router() -> Router {
    Router::new()
        .route(Method::Get, "/", &[], ok)
        .and_then(|r| r.route(Method::Get, "/_search", &["q", "size"], ok))
        .and_then(|r| r.route(Method::Post, "/_search", &["q", "size"], ok))
        .and_then(|r| r.route(Method::Get, "/{index}/_search", &["q", "size", "from"], ok))
        .and_then(|r| r.route(Method::Head, "/{index}/_search", &[], ok))
        .and_then(|r| r.route(Method::Put, "/{index}", &[], exists))
        .and_then(|r| {
            r.route(
                Method::Get,
                "/_fail_before",
                &[],
                |_: &mut Request<'_>, _: &mut Response<'_>| {
                    Err(RestError::illegal_argument("refused by the handler"))
                },
            )
        })
        .and_then(|r| {
            r.route(
                Method::Get,
                "/_fail_after",
                &[],
                |_: &mut Request<'_>, resp: &mut Response<'_>| {
                    let mut w = resp.start(200, Some(JSON), Length::Chunked, &[])?;
                    w.write_all(b"{\"partial\":")?;
                    w.flush()?;
                    Err(RestError::illegal_argument("too late"))
                },
            )
        })
        .and_then(|r| {
            r.route(
                Method::Get,
                "/_silent",
                &[],
                |_: &mut Request<'_>, _: &mut Response<'_>| Ok(()),
            )
        })
        .expect("routes")
}

#[test]
fn a_routed_request_sees_decoded_path_and_query_parameters() {
    let server = start(small(), router());
    let mut c = Client::connect(server.local_addr());
    let r = c.request(&get("/my%2Dlogs/_search?q=a+b%21&size=1"));
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-type"), Some(JSON));
    assert_eq!(
        r.json(),
        json!({"index": "my-logs", "q": "a b!", "method": "GET", "raw": "/my%2Dlogs/_search", "host": "t"})
    );
    // The connection stays open for the next request.
    let r = c.request(&get("/?pretty"));
    assert_eq!(r.status, 200);
    server.shutdown();
}

#[test]
fn an_unknown_path_is_400_no_handler_found() {
    let server = start(small(), router());
    let mut c = Client::connect(server.local_addr());
    let r = c.request(&get("/a/b/c?x=1"));
    assert_eq!(r.status, 400);
    assert_eq!(
        r.json(),
        json!({"error": "no handler found for uri [/a/b/c] and method [GET]"})
    );
    server.shutdown();
}

#[test]
fn a_known_path_with_another_method_is_405_with_allow() {
    let server = start(small(), router());
    let mut c = Client::connect(server.local_addr());
    let r = c.request(b"DELETE /_search?q=x HTTP/1.1\r\n\r\n");
    assert_eq!(r.status, 405);
    // `/{index}` matches `_search` too, so PUT is allowed, as in OpenSearch.
    assert_eq!(r.header("allow"), Some("GET,POST,PUT"));
    assert_eq!(
        r.json(),
        json!({"error": "Incorrect HTTP method for uri [/_search?q=x] and method [DELETE], allowed: [GET, POST, PUT]", "status": 405})
    );
    let r = c.request(b"OPTIONS /_search HTTP/1.1\r\n\r\n");
    assert_eq!((r.status, r.header("allow")), (200, Some("GET,POST,PUT")));
    let r = c.request(b"OPTIONS /a/b/c HTTP/1.1\r\n\r\n");
    assert_eq!((r.status, r.header("allow")), (200, None));
    let r = c.request(b"BREW /_search HTTP/1.1\r\n\r\n");
    assert_eq!(r.status, 405);
    assert_eq!(
        r.json(),
        json!({"error": "Unexpected HTTP method, allowed: [GET, POST, PUT]", "status": 405})
    );
    let r = c.request(b"BREW /a/b/c HTTP/1.1\r\n\r\n");
    assert_eq!(
        r.json(),
        json!({"error": "Unexpected HTTP method", "status": 405})
    );
    server.shutdown();
}

#[test]
fn unknown_query_parameters_are_refused_as_opensearch_refuses_them() {
    let server = start(small(), router());
    let mut c = Client::connect(server.local_addr());
    let r = c.request(&get("/logs/_search?sise=1&zzz=2&q=x"));
    assert_eq!(r.status, 400);
    let reason = "request [/logs/_search] contains unrecognized parameters: [sise] -> did you mean [size]?, [zzz]";
    assert_eq!(
        r.json(),
        json!({
            "error": {
                "root_cause": [{"type": "illegal_argument_exception", "reason": reason}],
                "type": "illegal_argument_exception",
                "reason": reason
            },
            "status": 400
        })
    );
    // The global formatting parameters are taken everywhere.
    assert_eq!(
        c.request(&get("/logs/_search?pretty&human=true")).status,
        200
    );
    // A malformed escape is refused with RestUtils' message.
    let r = c.request(&get("/logs/_search?q=%zz"));
    assert_eq!(
        r.json()["error"]["reason"],
        "invalid escape sequence `%zz' at index 0 of: %zz"
    );
    let r = c.request(&get("/lo%g/_search"));
    assert_eq!(r.status, 400);
    server.shutdown();
}

/// The recorded fixture's 400 for creating an index twice, against the same
/// exception rendered by the edge: status, content type and body.
#[test]
fn an_exception_body_matches_the_recorded_opensearch_fixture() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/compat/index/create-index.json"
    );
    #[allow(clippy::disallowed_methods)]
    let text = std::fs::read_to_string(path).expect("the fixture exists");
    let fixture: serde_json::Value = serde_json::from_str(&text).expect("json");
    let step = fixture["steps"]
        .as_array()
        .and_then(|s| s.iter().find(|s| s["name"] == "create again"))
        .expect("the step");
    let want = &step["response"];

    let server = start(small(), router());
    let mut c = Client::connect(server.local_addr());
    let body = serde_json::to_vec(&step["request"]["body"]).expect("body");
    let mut raw = format!(
        "PUT /compat-create-index HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(&body);
    let r = c.request(&raw);
    assert_eq!(
        u64::from(r.status),
        want["status"].as_u64().expect("status")
    );
    assert_eq!(
        r.header("content-type"),
        want["headers"]["content-type"].as_str()
    );
    assert_eq!(r.json(), want["body"]);
    server.shutdown();
}

#[test]
fn handler_errors_before_and_after_the_response_starts() {
    let server = start(small(), router());
    let mut c = Client::connect(server.local_addr());
    let r = c.request(&get("/_fail_before"));
    assert_eq!(
        (r.status, r.json()["error"]["type"].as_str()),
        (400, Some("illegal_argument_exception"))
    );
    // An error after the status line went out can only close the connection.
    c.send(&get("/_fail_after")).expect("sends");
    assert!(c.reply().is_err(), "the chunked body never ends");
    let mut c = Client::connect(server.local_addr());
    let r = c.request(&get("/_silent"));
    assert_eq!(r.status, 500);
    assert!(c.is_closed());
    server.shutdown();
}

#[test]
fn head_requests_get_the_length_without_the_body() {
    let server = start(small(), router());
    let mut c = Client::connect(server.local_addr());
    c.send(b"HEAD /logs/_search HTTP/1.1\r\n\r\n")
        .expect("sends");
    let r = c.reply_head().expect("reply");
    assert_eq!(r.status, 200);
    let n: usize = r
        .header("content-length")
        .expect("length")
        .parse()
        .expect("number");
    assert!(n > 0);
    // Nothing but the next reply follows.
    assert_eq!(c.request(&get("/")).status, 200);
    server.shutdown();
}

#[test]
fn pipelined_requests_are_answered_in_order_and_ids_are_echoed() {
    let server = start(small(), router());
    let mut c = Client::connect(server.local_addr());
    let mut raw = b"GET /?pretty HTTP/1.1\r\nX-Opaque-Id: one\r\n\r\n".to_vec();
    raw.extend_from_slice(&post("/_search?q=two", "application/json", b"{}"));
    raw.extend_from_slice(
        b"GET /_search?q=three HTTP/1.1\r\nX-Request-Id: r3\r\nConnection: close\r\n\r\n",
    );
    c.send(&raw).expect("sends");
    let a = c.reply().expect("first");
    assert_eq!(a.header("x-opaque-id"), Some("one"));
    let b = c.reply().expect("second");
    assert_eq!(b.json()["q"], "two");
    let d = c.reply().expect("third");
    assert_eq!(
        (
            d.json()["q"].as_str(),
            d.header("x-request-id"),
            d.header("connection")
        ),
        (Some("three"), Some("r3"), Some("close"))
    );
    assert!(c.is_closed());
    server.shutdown();
}

#[test]
fn http_1_0_closes_unless_kept_alive() {
    let server = start(small(), router());
    let mut c = Client::connect(server.local_addr());
    let r = c.request(b"GET / HTTP/1.0\r\nConnection: keep-alive\r\n\r\n");
    assert_eq!(r.header("connection"), Some("keep-alive"));
    let r = c.request(b"GET / HTTP/1.0\r\n\r\n");
    assert_eq!(r.header("connection"), Some("close"));
    assert!(c.is_closed());
    server.shutdown();
}

#[test]
fn malformed_heads_get_400_and_close() {
    let server = start(small(), router());
    for raw in [
        &b"GET /\r\n\r\n"[..],
        b"GET / HTTP/1.1\r\nbad header\r\n\r\n",
        b"GET / HTTP/1.1\r\nTransfer-Encoding: gzip\r\n\r\n",
    ] {
        let mut c = Client::connect(server.local_addr());
        let r = c.request(raw);
        assert_eq!(r.status, 400, "{raw:?}");
        assert_eq!(r.json()["error"]["type"], "illegal_argument_exception");
        assert!(c.is_closed());
    }
    let mut c = Client::connect(server.local_addr());
    let r = c.request(b"GET / HTTP/1.1\r\nExpect: tea\r\n\r\n");
    assert_eq!((r.status, r.body.len()), (417, 0));
    server.shutdown();
}

// ARITH: small fixed sizes from the test configuration; no overflow.
#[allow(clippy::arithmetic_side_effects)]
#[test]
fn the_server_charges_every_buffer_at_start_and_refuses_a_pool_one_byte_short() {
    use oss_mem::Pool;
    use oss_rest::{ServerConfig, StartError};
    use std::net::TcpListener;
    let config = small();
    let head = config.max_initial_line + config.max_header_bytes + 4;
    let need = config.workers * (2 * head + config.write_buffer_size)
        + config.body_buffers * config.body_buffer_size;
    let exact = support::budget(need);
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let server =
        oss_rest::Server::start(listener, config.clone(), router(), &exact).expect("exactly fits");
    assert_eq!(exact.in_use(Pool::RequestBodies), need);
    server.shutdown();
    assert_eq!(
        exact.in_use(Pool::RequestBodies),
        0,
        "released when the server stops"
    );
    let short = support::budget(need - 1);
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let refused = oss_rest::Server::start(listener, config, router(), &short);
    assert!(matches!(refused, Err(StartError::Memory(_))));
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let bad = ServerConfig {
        workers: 0,
        ..small()
    };
    assert!(matches!(
        oss_rest::Server::start(listener, bad, router(), &short),
        Err(StartError::Config(_))
    ));
}

fn prefixed() -> Router {
    router()
        .prefix("/_plugins/_ml", ok)
        .and_then(|r| r.prefix("/_ltr", ok))
        .expect("prefixes")
}

#[test]
fn a_prefix_route_takes_every_method_path_and_parameter_under_it() {
    let server = start(small(), prefixed());
    let mut c = Client::connect(server.local_addr());
    for path in [
        "/_plugins/_ml",
        "/_plugins/_ml/",
        "/_plugins/_ml/models/_search?anything=1",
        "/_ltr/_featureset/a",
    ] {
        let r = c.request(&get(path));
        assert_eq!(r.status, 200, "{path}");
        assert_eq!(r.json()["raw"], path.split('?').next().unwrap());
    }
    let r = c.request(&post("/_plugins/_ml/models", "application/json", b"{}"));
    assert_eq!((r.status, r.json()["method"].clone()), (200, json!("POST")));
    let r = c.request(b"DELETE /_plugins/_ml/models/1 HTTP/1.1\r\nhost: t\r\n\r\n");
    assert_eq!(
        (r.status, r.json()["method"].clone()),
        (200, json!("DELETE"))
    );
    server.shutdown();
}

#[test]
fn a_prefix_matches_whole_segments_only() {
    let server = start(small(), prefixed());
    let mut c = Client::connect(server.local_addr());
    for path in ["/_plugins/_mlx", "/_plugins/other/x", "/_ltrx/a", "/_plugins/_ml_"] {
        let r = c.request(&get(path));
        assert_eq!(r.status, 400, "{path}");
    }
    server.shutdown();
}

#[test]
fn an_ordinary_route_beats_a_prefix_and_keeps_its_own_405() {
    let router = router()
        .route(Method::Get, "/_plugins/_ml/native", &["x"], ok)
        .and_then(|r| r.prefix("/_plugins/_ml", ok))
        .expect("routes");
    let server = start(small(), router);
    let mut c = Client::connect(server.local_addr());
    // The ordinary route answers, and refuses a parameter it does not take.
    let r = c.request(&get("/_plugins/_ml/native?x=1"));
    assert_eq!(r.json()["raw"], "/_plugins/_ml/native");
    let r = c.request(&get("/_plugins/_ml/native?nope=1"));
    assert_eq!(r.status, 400);
    // Another method on the ordinary route's path falls to the prefix.
    let r = c.request(&post("/_plugins/_ml/native", "application/json", b"{}"));
    assert_eq!((r.status, r.json()["method"].clone()), (200, json!("POST")));
    server.shutdown();
}

#[test]
fn a_prefix_may_not_be_empty_a_pattern_or_registered_twice() {
    assert!(Router::new().prefix("/", ok).is_err());
    assert!(Router::new().prefix("/{a}", ok).is_err());
    assert!(
        Router::new()
            .prefix("/a", ok)
            .and_then(|r| r.prefix("/a", ok))
            .is_err()
    );
}

#[test]
fn a_request_lists_its_headers_in_order() {
    fn echo(req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let all: Vec<_> = req.headers().map(|(n, v)| json!([n, v])).collect();
        resp.send(
            200,
            JSON,
            json!({"headers": all, "query": req.raw_query()})
                .to_string()
                .as_bytes(),
        )
    }
    let router = Router::new()
        .route(Method::Get, "/h", &["a"], echo)
        .expect("route");
    let server = start(small(), router);
    let mut c = Client::connect(server.local_addr());
    let r = c.request(b"GET /h?a=1 HTTP/1.1\r\nhost: t\r\nX-A: 1\r\nx-a: 2\r\n\r\n");
    assert_eq!(
        r.json(),
        json!({"headers": [["host", "t"], ["X-A", "1"], ["x-a", "2"]], "query": "a=1"})
    );
    server.shutdown();
}
