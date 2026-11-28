//! A guard admits or refuses every request before its route is found
//! (docs/specs/security-authn/SPEC.md).

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use oss_rest::{
    Admission, Denial, Guard, JSON, Method, Request, Response, RestError, Router, Verdict,
};
use support::{Client, get, small, start};

struct Token(AtomicUsize);

impl Guard for Token {
    fn admit(&self, req: &Admission<'_>) -> Verdict {
        self.0.fetch_add(1, Ordering::SeqCst);
        match req.authorization() {
            Some("Basic YWxpY2U6cHc=") => {
                Verdict::Allow(Some(oss_rest::Identity::stored(Arc::from("alice"))))
            }
            Some("Basic Ym9iOm5v") => Verdict::Deny(Denial {
                status: 429,
                content_type: "text/plain; charset=UTF-8",
                body: "slow down".to_owned(),
                challenge: None,
            }),
            _ => Verdict::Deny(Denial {
                status: 401,
                content_type: "text/plain; charset=UTF-8",
                body: "Unauthorized".to_owned(),
                challenge: Some("Basic realm=\"test\""),
            }),
        }
    }
}

fn who(req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
    let body = format!(r#"{{"user":"{}"}}"#, req.principal().unwrap_or("none"));
    resp.send(200, JSON, body.as_bytes())
}

fn guarded() -> (oss_rest::Server, Arc<Token>) {
    let guard = Arc::new(Token(AtomicUsize::new(0)));
    let router = Router::new()
        .route(Method::Get, "/who", &[], who)
        .expect("route")
        .with_guard(Arc::clone(&guard) as Arc<dyn Guard>);
    (start(small(), router), guard)
}

fn with_auth(path: &str, auth: &str) -> Vec<u8> {
    format!("GET {path} HTTP/1.1\r\nHost: t\r\nAuthorization: {auth}\r\n\r\n").into_bytes()
}

#[test]
fn an_admitted_request_reaches_its_handler_knowing_who_it_is() {
    let (server, _) = guarded();
    let mut c = Client::connect(server.local_addr());
    let r = c.request(&with_auth("/who", "Basic YWxpY2U6cHc="));
    assert_eq!((r.status, r.json()["user"].clone()), (200, "alice".into()));
    server.shutdown();
}

#[test]
fn a_refused_request_gets_the_guards_answer_and_a_challenge() {
    let (server, _) = guarded();
    let mut c = Client::connect(server.local_addr());
    let r = c.request(&get("/who"));
    assert_eq!(r.status, 401);
    assert_eq!(r.body, b"Unauthorized");
    assert_eq!(r.header("www-authenticate"), Some("Basic realm=\"test\""));
    let r = c.request(&with_auth("/who", "Basic Ym9iOm5v"));
    assert_eq!((r.status, r.body.as_slice()), (429, &b"slow down"[..]));
    assert_eq!(r.header("www-authenticate"), None);
    server.shutdown();
}

#[test]
fn a_path_no_route_serves_is_refused_before_it_is_found() {
    let (server, guard) = guarded();
    let mut c = Client::connect(server.local_addr());
    assert_eq!(c.request(&get("/nothing/here")).status, 401);
    assert_eq!(c.request(&get("/who")).status, 401);
    assert_eq!(guard.0.load(Ordering::SeqCst), 2);
    let r = c.request(&with_auth("/nothing/here", "Basic YWxpY2U6cHc="));
    assert_eq!(
        r.status, 400,
        "an admitted request to no route is the usual refusal"
    );
    server.shutdown();
}

#[test]
fn a_refused_request_with_a_body_closes_the_connection() {
    let (server, _) = guarded();
    let mut c = Client::connect(server.local_addr());
    let r = c.request(
        b"POST /who HTTP/1.1\r\nHost: t\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
    );
    assert_eq!(r.status, 401);
    assert!(c.is_closed());
    server.shutdown();
}
