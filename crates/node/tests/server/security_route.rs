//! The security configuration API over a node's document sets
//! (docs/specs/security-config/SPEC.md).

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::DocKind;
use oss_mem::MemoryBudget;
use oss_meta::DocSet;
use oss_node::security_route::{SecurityService, routes};
use oss_rest::{Router, Server};

const BUF: usize = 1 << 16;
const API: &str = "/_plugins/_security/api";

struct Rig {
    server: Server,
    store: Arc<Emulator>,
}

fn rig() -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let service = Arc::new(SecurityService::new(&blobs, "", BUF, 4).expect("service"));
    let router = routes(Router::new(), &service).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig { server, store }
}

fn call(r: &Rig, method: &str, target: &str, body: Option<&str>) -> (u16, String) {
    let reply = support::request(
        r.server.local_addr(),
        method,
        target,
        body.map(str::as_bytes),
    );
    (
        reply.status,
        String::from_utf8_lossy(&reply.body).into_owned(),
    )
}

fn stored(r: &Rig, kind: DocKind, name: &str) -> String {
    let set = DocSet::new(Arc::clone(&r.store) as Arc<dyn BlobStore>, "", kind).expect("set");
    let (mut marker, mut doc) = (vec![0u8; BUF], vec![0u8; BUF]);
    let len = set
        .get(name, &mut marker, &mut doc)
        .expect("reads")
        .expect("stored");
    String::from_utf8(doc[..len].to_vec()).expect("utf8")
}

fn total(r: &Rig, ops: &[S3Op]) -> u64 {
    ops.iter().map(|op| r.store.meter().count(*op)).sum()
}

#[test]
fn a_password_is_stored_only_as_its_hash() {
    let r = rig();
    let put = call(
        &r,
        "PUT",
        &format!("{API}/internalusers/u"),
        Some(r#"{"password":"Zz9!longEnough","backend_roles":["b"]}"#),
    );
    assert_eq!(put.0, 201, "{}", put.1);
    let doc = stored(&r, DocKind::SecurityInternalUsers, "u");
    assert!(!doc.contains("Zz9!longEnough"), "{doc}");
    let hash = doc
        .split(r#""hash":""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("a hash is stored")
        .to_owned();
    assert!(bcrypt::verify("Zz9!longEnough", &hash).expect("verifies"));
    assert!(!bcrypt::verify("another", &hash).expect("verifies"));
    // An update without a password keeps the hash.
    let again = call(
        &r,
        "PUT",
        &format!("{API}/internalusers/u"),
        Some(r#"{"backend_roles":["c"]}"#),
    );
    assert_eq!(again.0, 200, "{}", again.1);
    assert!(stored(&r, DocKind::SecurityInternalUsers, "u").contains(&hash));
    // A short password is refused and changes nothing.
    let weak = call(
        &r,
        "PUT",
        &format!("{API}/internalusers/w"),
        Some(r#"{"password":"short"}"#),
    );
    assert_eq!(
        weak,
        (
            400,
            r#"{"status":"error","reason":"Weak password"}"#.to_owned()
        )
    );
}

#[test]
fn an_answer_never_carries_a_hash_or_a_password() {
    let r = rig();
    call(
        &r,
        "PUT",
        &format!("{API}/internalusers/u"),
        Some(r#"{"password":"Zz9!longEnough"}"#),
    );
    let read = call(&r, "GET", &format!("{API}/internalusers/u"), None);
    assert_eq!(read.0, 200);
    assert!(read.1.contains(r#""hash":"""#), "{}", read.1);
    assert!(
        !read.1.contains("Zz9!") && !read.1.contains("$2"),
        "{}",
        read.1
    );
}

#[test]
fn a_mapping_for_an_absent_role_is_refused() {
    let r = rig();
    let refused = call(
        &r,
        "PUT",
        &format!("{API}/rolesmapping/ghost"),
        Some(r#"{"users":["u"]}"#),
    );
    assert_eq!(refused.0, 404, "{}", refused.1);
    assert_eq!(
        call(&r, "GET", &format!("{API}/rolesmapping/ghost"), None).0,
        404,
        "nothing was written"
    );
    call(&r, "PUT", &format!("{API}/roles/ghost"), Some("{}"));
    assert_eq!(
        call(
            &r,
            "PUT",
            &format!("{API}/rolesmapping/ghost"),
            Some(r#"{"users":["u"]}"#)
        )
        .0,
        201
    );
}

#[test]
fn security_requests_per_operation() {
    let r = rig();
    let gets = [S3Op::Get, S3Op::GetRange, S3Op::Head];
    let (g0, p0) = (total(&r, &gets), total(&r, &[S3Op::Put]));
    assert_eq!(
        call(&r, "PUT", &format!("{API}/roles/a"), Some("{}")).0,
        201
    );
    assert_eq!(
        (total(&r, &gets) - g0, total(&r, &[S3Op::Put]) - p0),
        (2, 2),
        "a first put: the existence read, the marker read, bytes and marker"
    );
    let (g1, p1) = (total(&r, &gets), total(&r, &[S3Op::Put]));
    assert_eq!(call(&r, "GET", &format!("{API}/roles/a"), None).0, 200);
    assert_eq!(
        (total(&r, &gets) - g1, total(&r, &[S3Op::Put]) - p1),
        (2, 0)
    );
    assert_eq!(r.store.meter().count(S3Op::List), 0);
}

#[test]
fn the_1001st_document_of_a_kind_is_refused() {
    let r = rig();
    for i in 0..1000 {
        let put = call(&r, "PUT", &format!("{API}/tenants/t{i}"), Some("{}"));
        assert_eq!(put.0, 201, "{i}: {}", put.1);
    }
    let over = call(&r, "PUT", &format!("{API}/tenants/over"), Some("{}"));
    assert_eq!(over.0, 400, "{}", over.1);
    assert_eq!(call(&r, "GET", &format!("{API}/tenants/over"), None).0, 404);
    assert_eq!(
        call(
            &r,
            "PUT",
            &format!("{API}/tenants/t0"),
            Some(r#"{"description":"x"}"#)
        )
        .0,
        200,
        "a full set still replaces"
    );
}
