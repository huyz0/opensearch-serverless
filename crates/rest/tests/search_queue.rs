//! docs/specs/search-queue: over-share searches wait off the workers in a
//! bounded queue (ADR 0056).

// ARITH: test counters; an underflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

mod support;

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use oss_rest::{
    Decision, JSON, Method, Request, Response, RestError, Router, SearchAdmission, ServerConfig,
};
use support::{Client, budget, get, post, small, start};

/// Two workers, one search a tenant (the share), one worker left for the
/// tenants under theirs.
struct Gate {
    share: usize,
    free_below: usize,
    state: Mutex<(usize, HashMap<String, usize>)>,
    decided: Mutex<usize>,
}

struct Permit {
    gate: Arc<Gate>,
    tenant: String,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut s = self.gate.state.lock().expect("lock");
        s.0 -= 1;
        let n = s.1.get_mut(&self.tenant).expect("tenant");
        *n -= 1;
        if *n == 0 {
            s.1.remove(&self.tenant);
        }
    }
}

impl Gate {
    fn new() -> Arc<Gate> {
        Arc::new(Gate {
            share: 1,
            free_below: 1,
            state: Mutex::new((0, HashMap::new())),
            decided: Mutex::new(0),
        })
    }

    fn running(&self) -> usize {
        self.state.lock().expect("lock").0
    }

    fn reserve(self: &Arc<Self>, tenant: &str) -> Option<Box<dyn Send>> {
        let mut s = self.state.lock().expect("lock");
        let own = s.1.get(tenant).copied().unwrap_or(0);
        if own >= self.share && s.0 >= self.free_below {
            return None;
        }
        s.0 += 1;
        *s.1.entry(tenant.to_owned()).or_insert(0) += 1;
        Some(Box::new(Permit {
            gate: Arc::clone(self),
            tenant: tenant.to_owned(),
        }))
    }
}

struct Hook(Arc<Gate>);

fn tenant_of(path: &str) -> Option<&str> {
    let rest = path.strip_prefix('/')?;
    let (index, tail) = rest.split_once('/')?;
    (tail == "_search" && !index.starts_with('_')).then_some(index)
}

impl SearchAdmission for Hook {
    fn decide(&self, _method: Option<Method>, path: &str) -> Decision {
        *self.0.decided.lock().expect("lock") += 1;
        let Some(tenant) = tenant_of(path) else {
            return Decision::Run(None);
        };
        match self.0.reserve(tenant) {
            Some(p) => Decision::Run(Some(p)),
            None => Decision::Defer(Arc::from(tenant)),
        }
    }

    fn try_reserve(&self, tenant: &str) -> Option<Box<dyn Send>> {
        self.0.reserve(tenant)
    }
}

/// `a`'s searches hold their worker until released; everything echoes its
/// index and body.
struct Hold {
    open: Mutex<bool>,
    changed: Condvar,
}

impl Hold {
    fn new() -> Arc<Hold> {
        Arc::new(Hold {
            open: Mutex::new(false),
            changed: Condvar::new(),
        })
    }

    fn release(&self) {
        *self.open.lock().expect("lock") = true;
        self.changed.notify_all();
    }

    fn wait(&self) {
        let mut open = self.open.lock().expect("lock");
        while !*open {
            open = self.changed.wait(open).expect("wait");
        }
    }
}

fn server_with(size: usize, wait: Duration, hold: &Arc<Hold>) -> (oss_rest::Server, Arc<Gate>) {
    server_with_workers(2, size, wait, hold)
}

fn server_with_workers(
    workers: usize,
    size: usize,
    wait: Duration,
    hold: &Arc<Hold>,
) -> (oss_rest::Server, Arc<Gate>) {
    let gate = Gate::new();
    let h = Arc::clone(hold);
    let handler = move |req: &mut Request<'_>, resp: &mut Response<'_>| -> Result<(), RestError> {
        let index = req
            .path_param("index")
            .map(|c| c.into_owned())
            .unwrap_or_default();
        let mut body = Vec::new();
        while let Some(c) = req.body().next_chunk()? {
            body.extend_from_slice(c);
        }
        if index == "a" || index == "d" {
            h.wait();
        }
        let out = format!(
            r#"{{"index":"{index}","body":"{}"}}"#,
            String::from_utf8_lossy(&body)
        );
        resp.send(200, JSON, out.as_bytes())
    };
    let router = Router::new()
        .route(Method::Post, "/{index}/_search", &[], handler)
        .expect("route")
        .route(
            Method::Get,
            "/ping",
            &[],
            |_: &mut Request<'_>, r: &mut Response<'_>| r.send(200, JSON, b"{}"),
        )
        .expect("route")
        .with_search_admission(Arc::new(Hook(Arc::clone(&gate))));
    let config = ServerConfig {
        workers,
        max_connections: 8,
        search_queue_size: size,
        search_queue_wait: wait,
        ..small()
    };
    (start(config, router), gate)
}

fn search(index: &str, body: &str) -> Vec<u8> {
    post(
        &format!("/{index}/_search"),
        "application/json",
        body.as_bytes(),
    )
}

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(10), "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

// AC-1
#[test]
fn with_the_queue_off_no_hook_is_called_and_requests_are_served_as_before() {
    let hold = Hold::new();
    let (server, gate) = server_with(0, Duration::from_secs(10), &hold);
    hold.release();
    let mut c = Client::connect(server.local_addr());
    let r = c.request(&search("b", "x"));
    assert_eq!((r.status, r.json()["body"].clone()), (200, "x".into()));
    assert_eq!(*gate.decided.lock().expect("lock"), 0);
    server.shutdown();
}

// AC-2, AC-5 (no permit leaks)
#[test]
fn an_over_share_search_is_parked_off_the_worker_and_answered_with_its_body_when_a_worker_frees() {
    let hold = Hold::new();
    let (server, gate) = server_with(2, Duration::from_secs(10), &hold);
    let addr = server.local_addr();
    let first = std::thread::spawn(move || Client::connect(addr).request(&search("a", "one")));
    until("the first holds a worker", || gate.running() == 1);
    // The second `a` search is over its share and the node is at its free bound.
    let second = std::thread::spawn(move || Client::connect(addr).request(&search("a", "two")));
    until("the second is parked", || server.search_queue().parked == 1);
    // Both workers are free: a quiet tenant is answered at once.
    let started = Instant::now();
    let quiet = Client::connect(addr).request(&search("b", "q"));
    assert_eq!(quiet.status, 200);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(server.search_queue().parked, 1, "still waiting");
    hold.release();
    let (one, two) = (first.join().expect("one"), second.join().expect("two"));
    assert_eq!((one.status, two.status), (200, 200));
    assert_eq!(two.json()["body"], "two", "the body survives the replay");
    let stats = server.search_queue();
    assert_eq!((stats.deferred, stats.resumed, stats.parked), (1, 1, 0));
    until("every permit returned", || gate.running() == 0);
    server.shutdown();
}

// AC-3
#[test]
fn a_full_queue_answers_429_with_retry_after_and_closes() {
    let hold = Hold::new();
    let (server, gate) = server_with(1, Duration::from_secs(10), &hold);
    let addr = server.local_addr();
    let held = std::thread::spawn(move || Client::connect(addr).request(&search("a", "1")));
    until("held", || gate.running() == 1);
    let parked = std::thread::spawn(move || Client::connect(addr).request(&search("a", "2")));
    until("parked", || server.search_queue().parked == 1);
    let mut c = Client::connect(addr);
    let refused = c.request(&search("a", "3"));
    assert_eq!(refused.status, 429, "{:?}", refused.body);
    assert_eq!(refused.header("retry-after"), Some("1"));
    assert!(
        String::from_utf8_lossy(&refused.body).contains("es_rejected_execution_exception"),
        "{:?}",
        String::from_utf8_lossy(&refused.body)
    );
    assert!(c.is_closed());
    assert_eq!(server.search_queue().refused, 1);
    hold.release();
    assert_eq!(held.join().expect("held").status, 200);
    assert_eq!(parked.join().expect("parked").status, 200);
    server.shutdown();
}

// AC-4
#[test]
fn a_parked_search_past_the_wait_is_answered_429_and_its_counts_return() {
    let hold = Hold::new();
    let (server, gate) = server_with(2, Duration::from_millis(300), &hold);
    let addr = server.local_addr();
    let held = std::thread::spawn(move || Client::connect(addr).request(&search("a", "1")));
    until("held", || gate.running() == 1);
    let started = Instant::now();
    let waited = Client::connect(addr).request(&search("a", "2"));
    assert_eq!(waited.status, 429);
    assert!(started.elapsed() >= Duration::from_millis(300));
    let stats = server.search_queue();
    assert_eq!((stats.expired, stats.parked), (1, 0));
    assert_eq!(gate.running(), 1, "the parked search held no permit");
    hold.release();
    assert_eq!(held.join().expect("held").status, 200);
    server.shutdown();
}

// AC-4
#[test]
fn a_parked_search_expires_while_new_connections_keep_arriving() {
    let hold = Hold::new();
    let (server, gate) = server_with(2, Duration::from_millis(300), &hold);
    let addr = server.local_addr();
    let held = std::thread::spawn(move || Client::connect(addr).request(&search("a", "1")));
    until("held", || gate.running() == 1);
    let mut parked = Client::connect(addr);
    parked.send(&search("a", "2")).expect("sends");
    until("parked", || server.search_queue().parked == 1);
    // The idle timeout never fires while connections keep coming.
    let started = Instant::now();
    while server.search_queue().expired == 0 {
        assert!(started.elapsed() < Duration::from_secs(5), "never expired");
        assert_eq!(Client::connect(addr).request(&get("/ping")).status, 200);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(parked.reply().expect("a reply").status, 429);
    hold.release();
    assert_eq!(held.join().expect("held").status, 200);
    server.shutdown();
}

// AC-5
#[test]
fn a_peer_that_closes_while_parked_is_dropped_and_its_counts_return() {
    let hold = Hold::new();
    let (server, gate) = server_with(2, Duration::from_secs(30), &hold);
    let addr = server.local_addr();
    let held = std::thread::spawn(move || Client::connect(addr).request(&search("a", "1")));
    until("held", || gate.running() == 1);
    let mut gone = Client::connect(addr);
    gone.send(&search("a", "2")).expect("sends");
    until("parked", || server.search_queue().parked == 1);
    drop(gone);
    until("dropped", || server.search_queue().dropped == 1);
    assert_eq!(server.search_queue().parked, 0);
    hold.release();
    assert_eq!(held.join().expect("held").status, 200);
    until("every permit returned", || gate.running() == 0);
    server.shutdown();
}

// AC-6
#[test]
fn shutdown_answers_a_parked_entry_503_and_closes_it() {
    let hold = Hold::new();
    let (server, gate) = server_with(2, Duration::from_secs(30), &hold);
    let addr = server.local_addr();
    let held = std::thread::spawn(move || Client::connect(addr).request(&search("a", "1")));
    until("held", || gate.running() == 1);
    let mut parked = Client::connect(addr);
    parked.send(&search("a", "2")).expect("sends");
    until("parked", || server.search_queue().parked == 1);
    let release = Arc::clone(&hold);
    let stopper = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        release.release();
    });
    server.shutdown();
    stopper.join().expect("stopper");
    let reply = parked.reply().expect("a reply");
    assert_eq!(reply.status, 503);
    let _ = held.join();
}

// AC-7
#[test]
fn an_offer_reaches_workers_idle_in_the_channel_and_a_tenant_over_its_share_runs_when_the_node_is_under_its_bound()
 {
    let hold = Hold::new();
    let (server, gate) = server_with(2, Duration::from_secs(30), &hold);
    let addr = server.local_addr();
    let held = std::thread::spawn(move || Client::connect(addr).request(&search("a", "1")));
    until("held", || gate.running() == 1);
    let mut second = Client::connect(addr);
    second.send(&search("a", "2")).expect("sends");
    until("parked", || server.search_queue().parked == 1);
    // Free the first: the worker that finishes it offers the parked entry,
    // and the other worker is idle in the channel.
    hold.release();
    let reply = second.reply().expect("a reply");
    assert_eq!(reply.status, 200);
    assert_eq!(held.join().expect("held").status, 200);
    until("every permit returned", || gate.running() == 0);
    server.shutdown();
}

// AC-3 (memory)
#[test]
fn a_queue_the_budget_cannot_hold_refuses_to_start_and_no_queue_needs_none() {
    let gate = Gate::new();
    let router = |g: &Arc<Gate>| {
        Router::new()
            .route(
                Method::Get,
                "/ping",
                &[],
                |_: &mut Request<'_>, r: &mut Response<'_>| r.send(200, JSON, b"{}"),
            )
            .expect("route")
            .with_search_admission(Arc::new(Hook(Arc::clone(g))))
    };
    let config = |size: usize| ServerConfig {
        search_queue_size: size,
        ..small()
    };
    let listener = || std::net::TcpListener::bind("127.0.0.1:0").expect("binds");
    let tight = || budget(512 << 10);
    assert!(oss_rest::Server::start(listener(), config(100_000), router(&gate), &tight()).is_err());
    let ok =
        oss_rest::Server::start(listener(), config(0), router(&gate), &tight()).expect("starts");
    ok.shutdown();
}

// AC-2 (a tenant's turn survives another tenant leaving)
#[test]
fn a_tenant_stays_in_the_rotation_when_another_tenants_entry_is_dropped() {
    let hold = Hold::new();
    let (server, gate) = server_with_workers(3, 4, Duration::from_secs(30), &hold);
    let addr = server.local_addr();
    let held_a = std::thread::spawn(move || Client::connect(addr).request(&search("a", "1")));
    let held_d = std::thread::spawn(move || Client::connect(addr).request(&search("d", "0")));
    until("both held", || gate.running() == 2);
    let mut stays = Client::connect(addr);
    stays.send(&search("a", "2")).expect("sends");
    let mut gone = Client::connect(addr);
    gone.send(&search("d", "3")).expect("sends");
    until("both parked", || server.search_queue().parked == 2);
    drop(gone);
    until("dropped", || server.search_queue().dropped == 1);
    hold.release();
    assert_eq!(held_a.join().expect("held a").status, 200);
    assert_eq!(held_d.join().expect("held d").status, 200);
    let r = stays.reply().expect("a reply");
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["body"], "2");
    server.shutdown();
}

// AC-1 (an idle kept-alive connection stays when nothing waits)
#[test]
fn an_idle_kept_alive_connection_stays_open_when_nothing_waits() {
    let hold = Hold::new();
    hold.release();
    let (server, _gate) = server_with(2, Duration::from_secs(10), &hold);
    let mut c = Client::connect(server.local_addr());
    assert_eq!(c.request(&get("/ping")).status, 200);
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(c.request(&get("/ping")).status, 200);
    server.shutdown();
}
