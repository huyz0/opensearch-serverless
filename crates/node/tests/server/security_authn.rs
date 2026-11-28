//! Authentication in front of a node's routes
//! (docs/specs/security-authn/SPEC.md).

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use oss_blob::emulator::faults::Faults;
use oss_blob::{BlobStore, Emulator, S3Op};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::security_auth::{AuthGuard, Authenticator};
use oss_node::security_route::{SecurityService, authinfo_route, config_routes, routes};
use oss_node::security_state::{Clock, SecurityState};
use oss_rest::{Router, Server};

const BUF: usize = 1 << 16;
pub const API: &str = "/_plugins/_security/api";
pub const ADMIN: &str = "admin:Adm1n!Password";
pub const INTERVAL: u64 = 10_000;

pub struct Rig {
    pub server: Server,
    pub store: Arc<Emulator>,
    pub now: Arc<AtomicU64>,
    pub state: Arc<SecurityState>,
    pub clock: Clock,
}

pub fn rig_over(store: Arc<Emulator>) -> Rig {
    let now = Arc::new(AtomicU64::new(1_000_000));
    let clock: Clock = {
        let now = Arc::clone(&now);
        Arc::new(move || now.load(Ordering::SeqCst))
    };
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let budget = MemoryBudget::new(
        &BudgetConfig::new(128 << 20)
            .with_pool(Pool::RequestBodies, 64 << 20)
            .expect("fits"),
    );
    let hash = bcrypt::hash("Adm1n!Password", 4).expect("hashes");
    let state = Arc::new(
        SecurityState::new(
            &blobs,
            "",
            BUF,
            INTERVAL,
            ("admin".to_owned(), hash),
            &budget,
            Arc::clone(&clock),
        )
        .expect("state"),
    );
    let service = Arc::new(
        SecurityService::new(&blobs, "", BUF, 4)
            .expect("service")
            .with_state(Arc::clone(&state)),
    );
    let auth = Arc::new(Authenticator::new(Arc::clone(&state), Arc::clone(&clock)));
    let router = routes(Router::new(), &service).expect("routes");
    let router = config_routes(router, &service, &state).expect("config routes");
    let router = authinfo_route(router, &state)
        .expect("authinfo")
        .with_guard(Arc::new(AuthGuard(Arc::clone(&auth))));
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig {
        server,
        store,
        now,
        state,
        clock,
    }
}

pub fn rig() -> Rig {
    rig_over(Arc::new(Emulator::new(1 << 28, 5)))
}

pub fn call(
    r: &Rig,
    auth: Option<&str>,
    method: &str,
    target: &str,
    body: Option<&str>,
) -> (u16, String) {
    let reply = support::request_as(
        r.server.local_addr(),
        method,
        target,
        body.map(str::as_bytes),
        auth,
    );
    (reply.status, reply.text())
}

pub fn make_user(r: &Rig, name: &str, password: &str) {
    let put = call(
        r,
        Some(ADMIN),
        "PUT",
        &format!("{API}/internalusers/{name}"),
        Some(&format!(r#"{{"password":"{password}"}}"#)),
    );
    assert!(put.0 == 200 || put.0 == 201, "{put:?}");
}

#[test]
fn an_assembled_node_refuses_every_request_without_valid_credentials_alike() {
    let r = rig();
    make_user(&r, "u", "Zz9!longEnough");
    let mut answers = Vec::new();
    for auth in [
        None,
        Some("u:wrong"),
        Some("nobody:Zz9!longEnough"),
        Some("raw:Basic !!!!"),
        Some("raw:Bearer abc"),
    ] {
        let reply = support::request_as(
            r.server.local_addr(),
            "GET",
            "/_plugins/_security/authinfo",
            None,
            auth,
        );
        assert_eq!(
            reply.header("www-authenticate"),
            Some("Basic realm=\"OpenSearch Security\"")
        );
        answers.push((
            reply.status,
            reply.text(),
            reply.header("content-type").map(str::to_owned),
        ));
    }
    assert!(answers.iter().all(|a| *a == answers[0]), "{answers:?}");
    assert_eq!(answers[0].0, 401);
    assert_eq!(answers[0].1, "Unauthorized");
    // A path nothing serves is refused first as well.
    assert_eq!(call(&r, None, "GET", "/nothing/here", None).0, 401);
}

#[test]
fn a_user_created_through_the_api_authenticates_at_once_on_that_node() {
    let r = rig();
    assert_eq!(
        call(
            &r,
            Some("u:Zz9!longEnough"),
            "GET",
            "/_plugins/_security/authinfo",
            None
        )
        .0,
        401
    );
    make_user(&r, "u", "Zz9!longEnough");
    let me = call(
        &r,
        Some("u:Zz9!longEnough"),
        "GET",
        "/_plugins/_security/authinfo",
        None,
    );
    assert_eq!(me.0, 200, "{}", me.1);
    assert!(me.1.contains(r#""user_name":"u""#), "{}", me.1);
    // Not an administrator: the management API is refused, and says why.
    let denied = call(
        &r,
        Some("u:Zz9!longEnough"),
        "GET",
        &format!("{API}/roles/x"),
        None,
    );
    assert_eq!(denied.0, 403, "{}", denied.1);
    assert!(
        denied
            .1
            .contains("does not have any role privileged for admin access")
    );
}

#[test]
fn a_node_that_cannot_read_the_configuration_answers_503() {
    let r = rig();
    r.store.set_faults(
        S3Op::Get,
        Faults {
            fail_ppm: 1_000_000,
            throttle_ppm: 0,
        },
    );
    let down = call(&r, Some(ADMIN), "GET", "/_plugins/_security/authinfo", None);
    assert_eq!(down.0, 503, "{}", down.1);
    r.store.set_faults(S3Op::Get, Faults::default());
    r.now.fetch_add(1_000, Ordering::SeqCst);
    assert_eq!(
        call(&r, Some(ADMIN), "GET", "/_plugins/_security/authinfo", None).0,
        200
    );
}

#[test]
fn a_snapshot_older_than_three_intervals_is_not_used() {
    let r = rig();
    assert_eq!(
        call(&r, Some(ADMIN), "GET", "/_plugins/_security/authinfo", None).0,
        200
    );
    r.store.set_faults(
        S3Op::Get,
        Faults {
            fail_ppm: 1_000_000,
            throttle_ppm: 0,
        },
    );
    r.now.fetch_add(INTERVAL + 1, Ordering::SeqCst);
    assert_eq!(
        call(&r, Some(ADMIN), "GET", "/_plugins/_security/authinfo", None).0,
        200,
        "a failed refresh keeps the last copy for three intervals"
    );
    r.now.fetch_add(INTERVAL * 3, Ordering::SeqCst);
    assert_eq!(
        call(&r, Some(ADMIN), "GET", "/_plugins/_security/authinfo", None).0,
        503
    );
}

// docs/specs/security-authn AC-8
#[test]
fn listing_a_kind_is_refused_to_a_user_and_served_to_an_administrator() {
    let r = rig();
    make_user(&r, "u", "Zz9!longEnough");
    let user = call(
        &r,
        Some("u:Zz9!longEnough"),
        "GET",
        &format!("{API}/roles"),
        None,
    );
    assert_eq!(user.0, 403, "{}", user.1);
    let admin = call(&r, Some(ADMIN), "GET", &format!("{API}/rolesmapping"), None);
    assert_eq!((admin.0, admin.1.as_str()), (200, "{}"));
}
