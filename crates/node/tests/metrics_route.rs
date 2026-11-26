//! docs/specs/prometheus-metrics: the metrics route's body.

#![cfg(all(feature = "reader", feature = "writer"))]

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::usage::Ledger;
use oss_blob::{BlobStore, S3Op};
use oss_mem::{MemoryBudget, Pool};
use oss_node::usage_metrics::routes;
use oss_node::usage_route::UsageService;
use oss_rest::{Router, Server};

struct Logs;

impl oss_blob::usage::Classifier for Logs {
    fn index_of<'a>(&self, key: &'a str) -> Option<&'a str> {
        key.strip_prefix("i/")?.split('/').next()
    }
}

/// A node with one PUT of two bytes metered to `logs`, and a budget with
/// `held` bytes of the transport pool reserved while the scrape runs.
fn scrape(held: usize) -> (support::Reply, Arc<Ledger>) {
    let ledger = Arc::new(Ledger::new(4));
    let store = oss_blob::UsageStore::new(
        oss_blob::Emulator::new(1 << 20, 1),
        Arc::clone(&ledger),
        Arc::new(Logs),
    );
    store
        .put(
            "i/logs/a",
            &mut oss_blob::SliceSource::new(b"xy"),
            &mut [0u8; 16],
        )
        .expect("put");
    let config = oss_mem::BudgetConfig::new(1 << 20)
        .with_pool(Pool::Transport, 1 << 19)
        .expect("fits");
    let budget = MemoryBudget::new(&config);
    let _reservation = budget.try_reserve(Pool::Transport, held).expect("room");
    let service = UsageService::new(Arc::clone(&ledger), "n1", 5, Arc::new(|| 9))
        .with_memory_budget(budget.clone());
    let router = routes(Router::new(), Arc::new(service)).expect("route");
    let server = Server::start(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        router,
        &MemoryBudget::new(&support::budget_config()),
    )
    .expect("starts");
    let reply = support::request(server.local_addr(), "GET", "/_prometheus/metrics", None);
    (reply, ledger)
}

/// The samples of a body as `name{labels}` to value, checking the grammar:
/// every sample's family has a HELP and a TYPE line before it.
fn samples(body: &str) -> HashMap<String, f64> {
    let (mut helped, mut typed) = (Vec::new(), Vec::new());
    let mut out = HashMap::new();
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("# HELP ") {
            helped.push(rest.split(' ').next().expect("name").to_owned());
        } else if let Some(rest) = line.strip_prefix("# TYPE ") {
            let mut parts = rest.split(' ');
            let name = parts.next().expect("name").to_owned();
            assert!(matches!(parts.next(), Some("counter" | "gauge")), "{line}");
            assert!(helped.contains(&name), "TYPE before HELP: {line}");
            typed.push(name);
        } else if !line.is_empty() {
            let (series, value) = line.rsplit_once(' ').expect("name and value");
            let family = series.split('{').next().expect("family");
            assert!(typed.iter().any(|t| t == family), "no TYPE for {line}");
            let value: f64 = value.parse().expect("a number");
            assert!(value.is_finite(), "{line}");
            out.insert(series.to_owned(), value);
        }
    }
    out
}

// AC-1, AC-2
#[test]
fn the_body_is_prometheus_text_with_the_ledger_s_totals() {
    let (reply, ledger) = scrape(0);
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(
        reply.header("content-type"),
        Some("text/plain; version=0.0.4; charset=utf-8")
    );
    let s = samples(&reply.text());
    assert_eq!(ledger.total_requests(S3Op::Put), 1_000);
    assert_eq!(s["oss_s3_requests_total{op=\"put\"}"], 1.0);
    assert_eq!(s["oss_s3_requests_total{op=\"get\"}"], 0.0);
    assert_eq!(s["oss_s3_requests_total{op=\"delete\"}"], 0.0);
    assert_eq!(s["oss_s3_bytes_total{direction=\"written\"}"], 2.0);
    assert_eq!(s["oss_s3_bytes_total{direction=\"read\"}"], 0.0);
    assert_eq!(s["oss_s3_cost_units_total"], 1.0);
}

// AC-3
#[test]
fn a_held_reservation_raises_its_pool_s_in_use_only() {
    let (idle, _) = scrape(0);
    let (held, _) = scrape(4096);
    let (idle, held) = (samples(&idle.text()), samples(&held.text()));
    let in_use = |s: &HashMap<String, f64>, pool: &str| {
        s[&format!("oss_memory_pool_in_use_bytes{{pool=\"{pool}\"}}")]
    };
    assert_eq!(in_use(&idle, "transport"), 0.0);
    assert_eq!(in_use(&held, "transport"), 4096.0);
    assert_eq!(in_use(&held, "page_cache"), in_use(&idle, "page_cache"));
    assert!(held["oss_memory_pool_limit_bytes{pool=\"transport\"}"] >= 4096.0);
    for pool in Pool::ALL {
        assert!(held.contains_key(&format!(
            "oss_memory_pool_limit_bytes{{pool=\"{}\"}}",
            pool.name()
        )));
    }
}

// AC-4
#[test]
fn the_search_gate_and_queue_families_appear_only_when_they_are_on() {
    let (reply, _) = scrape(0);
    let body = reply.text();
    assert!(!body.contains("oss_search_slots_"), "{body}");
    assert!(!body.contains("oss_search_queue_"), "{body}");
}

// AC-6
#[test]
fn a_scrape_makes_no_blob_request() {
    let (_, ledger) = scrape(0);
    let before = ledger.total_requests(S3Op::Get) + ledger.total_requests(S3Op::Put);
    let service = UsageService::new(Arc::clone(&ledger), "n1", 5, Arc::new(|| 9));
    let router = routes(Router::new(), Arc::new(service)).expect("route");
    let server = Server::start(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        router,
        &MemoryBudget::new(&support::budget_config()),
    )
    .expect("starts");
    let reply = support::request(server.local_addr(), "GET", "/_prometheus/metrics", None);
    assert_eq!(reply.status, 200);
    let after = ledger.total_requests(S3Op::Get) + ledger.total_requests(S3Op::Put);
    assert_eq!(before, after);
}

// AC-4
#[test]
fn a_gate_that_is_on_is_reported_as_gauges_and_counters() {
    let ledger = Arc::new(Ledger::new(4));
    let gate = Arc::new(oss_node::search_slots::SearchSlots::new(
        &oss_node::search_slots::Config {
            slots: 3,
            fair_share_pct: 100,
            wait_millis: 10,
            waiter_cap: 2,
        },
    ));
    let service = UsageService::new(ledger, "n1", 5, Arc::new(|| 9)).with_search_slots(gate);
    let router = routes(Router::new(), Arc::new(service)).expect("route");
    let server = Server::start(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        router,
        &MemoryBudget::new(&support::budget_config()),
    )
    .expect("starts");
    let reply = support::request(server.local_addr(), "GET", "/_prometheus/metrics", None);
    let s = samples(&reply.text());
    assert_eq!(s["oss_search_slots_slots"], 3.0);
    assert_eq!(s["oss_search_slots_in_use"], 0.0);
    assert_eq!(s["oss_search_slots_admitted_total"], 0.0);
    assert!(!reply.text().contains("oss_search_queue_"));
    assert!(!reply.text().contains("oss_memory_pool_"));
}

// AC-4
#[test]
fn a_queue_that_is_on_reports_what_it_has_done() {
    use oss_rest::{QueueEvent, SearchAdmission};
    let queue = oss_node::search_queue::QueueAdmission::new(
        &oss_node::search_queue::Config {
            size: 4,
            share_pct: 50,
            reserved: 1,
            wait_millis: 10,
            max_connections: 8,
        },
        4,
    );
    let hook = oss_node::search_queue::Hook(Arc::clone(&queue));
    for event in [
        QueueEvent::Deferred,
        QueueEvent::Deferred,
        QueueEvent::Deferred,
        QueueEvent::Resumed(7),
        QueueEvent::Refused,
        QueueEvent::Expired,
    ] {
        hook.note(event);
    }
    let service = UsageService::new(Arc::new(Ledger::new(4)), "n1", 5, Arc::new(|| 9))
        .with_search_queue(queue);
    let router = routes(Router::new(), Arc::new(service)).expect("route");
    let server = Server::start(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        router,
        &MemoryBudget::new(&support::budget_config()),
    )
    .expect("starts");
    let reply = support::request(server.local_addr(), "GET", "/_prometheus/metrics", None);
    let s = samples(&reply.text());
    assert_eq!(s["oss_search_queue_deferred_total"], 3.0);
    assert_eq!(s["oss_search_queue_resumed_total"], 1.0);
    assert_eq!(s["oss_search_queue_refused_total"], 1.0);
    assert_eq!(s["oss_search_queue_expired_total"], 1.0);
    assert_eq!(s["oss_search_queue_dropped_total"], 0.0);
    // Three deferred, one resumed and one expired leave one parked.
    assert_eq!(s["oss_search_queue_parked"], 1.0);
    assert_eq!(s["oss_search_queue_running"], 0.0);
    assert_eq!(s["oss_search_queue_longest_wait_millis"], 7.0);
    assert!(!reply.text().contains("oss_search_slots_"));
}
