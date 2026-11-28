//! A linked-in plugin served over HTTP (docs/specs/plugin-system AC-5, AC-7,
//! AC-10): its routes mount, a request reaches it with no credentials, its
//! answer is validated, and a plugin that fails is answered and fenced.

use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use oss_ext::CallError;
use oss_ext::native::NativeInstance;
use oss_ext::rest::{RestRequest, RestResponse};
use oss_mem::MemoryBudget;
use oss_node::plugins::{Policy, Registry, mount};
use oss_rest::{Router, Server};

use crate::support;

const MANIFEST: &str = r#"{"plugin":{"id":"echo","version":"1.0.0","api":"1.0"},
"points":[{"name":"rest","version":1}],
"routes":[
{"method":"POST","path":"/_plugins/echo/{word}","action":"echo","query":["size"]},
{"method":"GET","path":"/_plugins/echo/{word}","action":"echo","query":["size"]}]}"#;

/// Echoes the call back as JSON; `panic` panics, `garbage` answers bytes that
/// are not a response, `big` answers a response over the ceiling's reach.
struct Echo {
    calls: AtomicUsize,
}

impl NativeInstance for Echo {
    fn manifest(&self) -> Vec<u8> {
        MANIFEST.as_bytes().to_vec()
    }

    fn call(&self, _point: &str, input: &[u8], _max: usize) -> Result<Vec<u8>, CallError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let req = RestRequest::decode(input, 1 << 20).expect("the host sends a valid request");
        match req.body.as_slice() {
            b"panic" => panic!("boom"),
            b"garbage" => return Ok(vec![9, 9, 9]),
            _ => {}
        }
        let text = |pairs: &[(String, String)]| {
            pairs
                .iter()
                .map(|(a, b)| format!("{a}={b}"))
                .collect::<Vec<_>>()
                .join(",")
        };
        let body = format!(
            "word:{};query:{};headers:{};body:{}",
            text(&req.path_params),
            text(&req.query),
            text(&req.headers),
            String::from_utf8_lossy(&req.body)
        );
        Ok(RestResponse {
            status: 201,
            headers: vec![
                ("x-plugin".into(), "yes".into()),
                ("content-type".into(), "text/plain".into()),
                ("set-cookie".into(), "a=b".into()),
            ],
            body: body.into_bytes(),
        }
        .encode())
    }
}

fn node() -> (Server, Arc<Echo>) {
    let echo = Arc::new(Echo {
        calls: AtomicUsize::new(0),
    });
    let linked: HashMap<String, Arc<dyn NativeInstance>> =
        HashMap::from([("echo".to_owned(), echo.clone() as Arc<dyn NativeInstance>)]);
    let policy = Policy::parse(r#"{"plugins":[{"id":"echo","tier":"native"}]}"#).expect("policy");
    let registry = Registry::load(&policy, &linked, &[]);
    let router = mount(Router::new(), &registry, || 0).expect("mounts");
    let budget = MemoryBudget::new(&support::budget_config());
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    (server, echo)
}

#[test]
fn a_request_reaches_the_plugin_without_credentials_and_its_answer_is_sent() {
    let (server, _) = node();
    let r = support::request_as(
        server.local_addr(),
        "POST",
        "/_plugins/echo/oslo?size=3",
        Some(b"hi"),
        Some("raw:Bearer secret"),
    );
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(
        r.text(),
        "word:word=oslo;query:size=3;headers:content-type=application/json;body:hi"
    );
    assert_eq!(r.header("x-plugin"), Some("yes"));
    assert_eq!(r.header("content-type"), Some("text/plain"));
    assert_eq!(r.header("set-cookie"), None, "a plugin cannot set a cookie");
    server.shutdown();
}

#[test]
fn a_query_parameter_the_manifest_does_not_name_is_refused_by_the_router() {
    let (server, echo) = node();
    let r = support::request(server.local_addr(), "GET", "/_plugins/echo/x?other=1", None);
    assert_eq!(r.status, 400, "{}", r.text());
    assert_eq!(echo.calls.load(Ordering::SeqCst), 0);
    server.shutdown();
}

#[test]
fn a_panic_answers_500_and_the_next_request_is_served() {
    let (server, _) = node();
    let r = support::request(
        server.local_addr(),
        "POST",
        "/_plugins/echo/x",
        Some(b"panic"),
    );
    assert_eq!(r.status, 500, "{}", r.text());
    assert!(r.text().contains("plugin_failed"), "{}", r.text());
    let r = support::request(server.local_addr(), "POST", "/_plugins/echo/x", Some(b"ok"));
    assert_eq!(r.status, 201, "{}", r.text());
    server.shutdown();
}

#[test]
fn a_malformed_answer_is_a_502_and_never_reaches_the_client() {
    let (server, _) = node();
    let r = support::request(
        server.local_addr(),
        "POST",
        "/_plugins/echo/x",
        Some(b"garbage"),
    );
    assert_eq!(r.status, 502, "{}", r.text());
    assert!(r.text().contains("plugin_malformed"), "{}", r.text());
    server.shutdown();
}

#[test]
fn five_failures_open_the_breaker_and_the_plugin_is_not_called_again() {
    let (server, echo) = node();
    for _ in 0..5 {
        let r = support::request(
            server.local_addr(),
            "POST",
            "/_plugins/echo/x",
            Some(b"panic"),
        );
        assert_eq!(r.status, 500);
    }
    let before = echo.calls.load(Ordering::SeqCst);
    let r = support::request(server.local_addr(), "POST", "/_plugins/echo/x", Some(b"ok"));
    assert_eq!(r.status, 503, "{}", r.text());
    assert!(r.text().contains("plugin_unavailable"), "{}", r.text());
    assert_eq!(
        echo.calls.load(Ordering::SeqCst),
        before,
        "no call was made"
    );
    server.shutdown();
}

#[test]
fn a_body_over_the_points_ceiling_is_a_413_before_the_plugin_is_called() {
    let (server, echo) = node();
    let body = vec![b'a'; (1 << 20) + 1];
    let r = support::request(server.local_addr(), "POST", "/_plugins/echo/x", Some(&body));
    assert_eq!(r.status, 413, "{}", r.text());
    assert_eq!(echo.calls.load(Ordering::SeqCst), 0);
    server.shutdown();
}
