//! docs/specs/notifications AC-8 and AC-9: the in-process sender's cache, its
//! request counts and its memory.

use crate::notifications_send::{open_policy, sink};
use crate::support;

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_node::notifications_route::{
    Accounts, Deliverer, Limits, NotificationsService, Notifier, Senders, routes, system_lookup,
};
use oss_rest::{Router, Server};
use serde_json::{Value, json};

const ROOT: &str = "/_plugins/_notifications";
const TTL: Duration = Duration::from_millis(400);

struct Rig {
    server: Server,
    store: Arc<Emulator>,
    notifier: Arc<Notifier>,
}

fn rig_with(budget: MemoryBudget, port: u16) -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let service = Arc::new(NotificationsService::new(&blobs, "", 1 << 16));
    let router = routes(Router::new(), &service).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let server = Server::start(
        listener,
        support::server_config(),
        router,
        &MemoryBudget::new(&support::budget_config()),
    )
    .expect("starts");
    let deliverer = Arc::new(Deliverer::new(
        system_lookup(),
        open_policy(port),
        Limits::default(),
    ));
    let senders = Arc::new(Senders::new(deliverer, Accounts::default()));
    let notifier = Arc::new(Notifier::new(&blobs, "", budget, senders, TTL));
    Rig {
        server,
        store,
        notifier,
    }
}

fn rig(port: u16) -> Rig {
    rig_with(MemoryBudget::new(&support::budget_config()), port)
}

fn call(r: &Rig, method: &str, target: &str, body: Option<Value>) -> (u16, Value) {
    let bytes = body.map(|b| b.to_string().into_bytes());
    let reply = support::request(r.server.local_addr(), method, target, bytes.as_deref());
    (
        reply.status,
        serde_json::from_slice(&reply.body).unwrap_or(Value::Null),
    )
}

fn hook(url: &str, enabled: bool) -> Value {
    json!({"config": {"name": "w", "config_type": "webhook", "is_enabled": enabled,
        "webhook": {"url": url}}})
}

fn create(r: &Rig, url: &str) -> String {
    let (status, body) = call(r, "POST", &format!("{ROOT}/configs"), Some(hook(url, true)));
    assert_eq!(status, 200, "{body}");
    body["config_id"].as_str().unwrap().to_owned()
}

fn gets(r: &Rig) -> u64 {
    r.store.meter().count(S3Op::Get)
}

fn past_ttl() {
    std::thread::sleep(TTL.saturating_add(Duration::from_millis(150)));
}

// AC-8
#[test]
fn a_send_inside_the_ttl_makes_no_request_and_one_past_it_makes_one_marker_get() {
    let t = sink("200 OK", "", "", Duration::ZERO);
    let r = rig(t.addr.port());
    let id = create(&r, &format!("http://127.0.0.1:{}/a", t.addr.port()));

    let before = gets(&r);
    assert_eq!(r.notifier.send(&id, "one").status_code, 200);
    assert_eq!(
        gets(&r).saturating_sub(before),
        3,
        "a first send reads the set and the config"
    );
    let before = gets(&r);
    for _ in 0..20 {
        assert_eq!(r.notifier.send(&id, "again").status_code, 200);
    }
    assert_eq!(gets(&r), before, "inside the TTL nothing is read");

    past_ttl();
    let before = gets(&r);
    assert_eq!(r.notifier.send(&id, "past").status_code, 200);
    assert_eq!(
        gets(&r).saturating_sub(before),
        1,
        "past the TTL one marker read, no change"
    );
    assert_eq!(t.seen.lock().unwrap().len(), 22);
}

// AC-8
#[test]
fn a_replace_a_disable_and_a_delete_are_seen_within_the_ttl() {
    let a = sink("200 OK", "", "", Duration::ZERO);
    let b = sink("200 OK", "", "", Duration::ZERO);
    let r = rig(a.addr.port());
    let id = create(&r, &format!("http://127.0.0.1:{}/a", a.addr.port()));
    assert_eq!(r.notifier.send(&id, "x").status_code, 200);
    assert_eq!(a.seen.lock().unwrap().len(), 1);

    // The replace goes to the second target; the first send still holds the old.
    let url = format!("http://127.0.0.1:{}/b", b.addr.port());
    let (s, _) = call(
        &r,
        "PUT",
        &format!("{ROOT}/configs/{id}"),
        Some(hook(&url, true)),
    );
    assert_eq!(s, 200);
    past_ttl();
    // `a`'s port is the only one allowed to this notifier, so the new target is
    // refused by the guard: the config was seen, and it was not sent to `a`.
    let d = r.notifier.send(&id, "y");
    assert_ne!(d.status_text, "Success", "{d:?}");
    assert_eq!(
        a.seen.lock().unwrap().len(),
        1,
        "the old target is not sent to"
    );

    let (s, _) = call(
        &r,
        "PUT",
        &format!("{ROOT}/configs/{id}"),
        Some(hook(&url, false)),
    );
    assert_eq!(s, 200);
    past_ttl();
    assert_eq!(r.notifier.send(&id, "z").status_text, "disabled");

    let (s, _) = call(&r, "DELETE", &format!("{ROOT}/configs/{id}"), None);
    assert_eq!(s, 200);
    past_ttl();
    assert_eq!(r.notifier.send(&id, "z").status_text, "not found");
    assert_eq!(r.notifier.counts().cached, 0, "a deleted config is dropped");
}

// AC-8
#[test]
fn many_sends_past_the_ttl_share_one_flight_and_an_idle_notifier_reads_nothing() {
    let t = sink("200 OK", "", "", Duration::ZERO);
    let r = Arc::new(rig(t.addr.port()));
    let id = create(&r, &format!("http://127.0.0.1:{}/a", t.addr.port()));
    assert_eq!(r.notifier.send(&id, "warm").status_code, 200);
    past_ttl();
    let before = gets(&r);
    let handles: Vec<_> = (0..64)
        .map(|_| {
            let (r, id) = (Arc::clone(&r), id.clone());
            std::thread::spawn(move || r.notifier.send(&id, "burst").status_code)
        })
        .collect();
    for h in handles {
        assert_eq!(h.join().unwrap(), 200);
    }
    assert_eq!(
        gets(&r).saturating_sub(before),
        1,
        "64 sends past the TTL are one marker read"
    );

    let idle = gets(&r);
    std::thread::sleep(TTL.saturating_mul(3));
    assert_eq!(gets(&r), idle, "no timer, no poll");
}

// AC-9
#[test]
fn a_send_past_the_pool_is_a_skipped_delivery_and_what_is_cached_is_charged() {
    let t = sink("200 OK", "", "", Duration::ZERO);
    let tiny = BudgetConfig::new(64 << 20)
        .with_pool(Pool::OpenIndexes, 1 << 10)
        .expect("fits");
    let r = rig_with(MemoryBudget::new(&tiny), t.addr.port());
    let id = create(&r, &format!("http://127.0.0.1:{}/a", t.addr.port()));
    let d = r.notifier.send(&id, "x");
    assert_eq!((d.status_code, d.status_text.as_str()), (503, "skipped"));
    assert_eq!(r.notifier.counts().skipped, 1);
    assert_eq!(t.seen.lock().unwrap().len(), 0, "nothing was sent");

    let budget = MemoryBudget::new(&support::budget_config());
    let r = rig_with(budget.clone(), t.addr.port());
    let id = create(&r, &format!("http://127.0.0.1:{}/a", t.addr.port()));
    assert_eq!(budget.in_use(Pool::OpenIndexes), 0);
    assert_eq!(r.notifier.send(&id, "x").status_code, 200);
    let held = budget.in_use(Pool::OpenIndexes);
    assert!(held > 0, "a cached config is charged to the pool");
    assert!(held < 1 << 20, "and only its own size: {held}");
}

// AC-8: the node hands the one sender to the plugins that call it.
#[test]
fn an_assembled_node_holds_the_sender_unless_notifications_are_off() {
    let config = crate::assemble::config();
    let (node, _, _) = crate::assemble::assembled_with(&config, 16 << 20);
    let notifier = node.notifier.expect("on by default");
    let d = notifier.send("nope", "x");
    assert_eq!((d.status_code, d.status_text.as_str()), (404, "not found"));

    let mut off = crate::assemble::config();
    off.notifications = false;
    let (node, _, _) = crate::assemble::assembled_with(&off, 16 << 20);
    assert!(node.notifier.is_none());
}
