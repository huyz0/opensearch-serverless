//! An authorizer decides between finding a route and reading anything of the
//! request (docs/specs/security-authz/SPEC.md).

mod support;

use std::sync::Arc;

use oss_rest::{
    Authorization, Authorizer, Denial, JSON, Method, Request, Response, RestError, Router,
};
use support::{Client, small, start};

/// Allows only a `GET` of the pattern `/open/{index}` whose index decodes to `ok`.
struct Rules;

impl Authorizer for Rules {
    fn authorize(&self, req: &Authorization<'_>) -> Option<Denial> {
        let allowed = req.method() == Some(Method::Get)
            && req.pattern() == "/open/{index}"
            && req.path_param("index").flatten().as_deref() == Some("ok");
        (!allowed).then(|| Denial {
            status: 403,
            content_type: "text/plain; charset=UTF-8",
            body: format!("denied {}", req.pattern()),
            challenge: None,
        })
    }
}

fn ok(_: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
    resp.send(200, JSON, b"{}")
}

fn router() -> Router {
    Router::new()
        .route(Method::Get, "/open/{index}", &["pretty"], ok)
        .expect("route")
        .route(Method::Post, "/open/{index}", &[], ok)
        .expect("route")
        .with_authorizer(Arc::new(Rules))
}

fn get(path: &str) -> Vec<u8> {
    format!("GET {path} HTTP/1.1\r\nHost: t\r\n\r\n").into_bytes()
}

// docs/specs/security-authz AC-8
#[test]
fn a_route_the_authorizer_allows_runs_and_one_it_refuses_does_not() {
    let server = start(small(), router());
    let mut c = Client::connect(server.local_addr());
    assert_eq!(c.request(&get("/open/ok")).status, 200);
    let r = c.request(&get("/open/%6Fk"));
    assert_eq!(r.status, 200, "the authorizer sees the decoded name");
    let r = c.request(&get("/open/secret"));
    assert_eq!(
        (r.status, r.body.as_slice()),
        (403, &b"denied /open/{index}"[..])
    );
    server.shutdown();
}

// docs/specs/security-authz AC-8
#[test]
fn a_refusal_comes_before_the_parameters_and_the_content_type_are_looked_at() {
    let server = start(small(), router());
    let mut c = Client::connect(server.local_addr());
    let r = c.request(&get("/open/secret?no_such_parameter=1"));
    assert_eq!(r.status, 403, "not the 400 of an unknown parameter");
    let r = c.request(
        b"POST /open/secret HTTP/1.1\r\nHost: t\r\nContent-Type: text/xml\r\nContent-Length: 2\r\n\r\n{}",
    );
    assert_eq!(r.status, 403, "not the 406 of a content type");
    server.shutdown();
}

// docs/specs/security-authz AC-8
#[test]
fn a_refused_request_with_a_body_closes_the_connection() {
    let server = start(small(), router());
    let mut c = Client::connect(server.local_addr());
    let r = c.request(
        b"POST /open/secret HTTP/1.1\r\nHost: t\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
    );
    assert_eq!(r.status, 403);
    assert!(c.is_closed());
    server.shutdown();
}

// docs/specs/security-authz AC-6
#[test]
fn routes_are_listed_with_their_methods_and_prefix_routes_as_any() {
    let router = router().prefix("/plugins/x", ok).expect("prefix");
    assert_eq!(
        router.routes(),
        vec![
            (Some(Method::Get), "/open/{index}"),
            (Some(Method::Post), "/open/{index}"),
            (None, "/plugins/x"),
        ],
        "routes in registration order, the prefix routes last"
    );
    assert!(router.has_authorizer());
    assert!(!Router::new().has_authorizer());
}

/// Admits everyone as `carol` and refuses nothing.
struct Carol;

impl oss_rest::Guard for Carol {
    fn admit(&self, _: &oss_rest::Admission<'_>) -> oss_rest::Verdict {
        oss_rest::Verdict::Allow(Some(Arc::from("carol")))
    }
}

/// Refuses every request, saying who it acted as and whether its route is a prefix route.
struct Says;

impl Authorizer for Says {
    fn authorize(&self, req: &Authorization<'_>) -> Option<Denial> {
        Some(Denial {
            status: 403,
            content_type: "text/plain; charset=UTF-8",
            body: format!("{:?} prefix={}", req.principal(), req.is_prefix()),
            challenge: None,
        })
    }
}

// docs/specs/security-authz AC-8
#[test]
fn the_authorizer_is_told_who_the_guard_admitted_and_whether_the_route_is_a_prefix() {
    let router = Router::new()
        .route(Method::Get, "/open/{index}", &[], ok)
        .expect("route")
        .prefix("/plugins/x", ok)
        .expect("prefix")
        .with_guard(Arc::new(Carol))
        .with_authorizer(Arc::new(Says));
    let server = start(small(), router);
    let mut c = Client::connect(server.local_addr());
    let r = c.request(&get("/open/a"));
    assert_eq!(r.body.as_slice(), &b"Some(\"carol\") prefix=false"[..]);
    let r = c.request(&get("/plugins/x/y"));
    assert_eq!(r.body.as_slice(), &b"Some(\"carol\") prefix=true"[..]);
    server.shutdown();

    let bare = Router::new()
        .route(Method::Get, "/open/{index}", &[], ok)
        .expect("route")
        .with_authorizer(Arc::new(Says));
    let server = start(small(), bare);
    let mut c = Client::connect(server.local_addr());
    let r = c.request(&get("/open/a"));
    assert_eq!(
        r.body.as_slice(),
        &b"None prefix=false"[..],
        "no guard, no name"
    );
    server.shutdown();
}

// docs/specs/security-authz AC-8
#[test]
fn a_refusal_keeps_the_connection_only_when_the_client_and_the_request_allow_it() {
    let server = start(small(), router());
    let mut c = Client::connect(server.local_addr());
    assert_eq!(c.request(&get("/open/secret")).status, 403);
    assert_eq!(
        c.request(&get("/open/ok")).status,
        200,
        "and serves the next"
    );
    let r = c.request(b"GET /open/secret HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
    assert_eq!(r.status, 403);
    {
        use std::io::Write;
        let _ = c.writer().write_all(&get("/open/ok"));
    }
    assert!(
        c.reply().is_err(),
        "the client asked to close, so nothing more is served"
    );
    server.shutdown();
}
