//! docs/specs/tenant-levels-wiring: a node's tenant levels from its usage
//! ledger.

#![cfg(all(feature = "reader", feature = "writer"))]
#![allow(clippy::arithmetic_side_effects)] // ARITH: test clock arithmetic.

#[path = "support/mod.rs"]
mod support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::BlobStore;
use oss_blob::guard::Mode;
use oss_blob::usage::Ledger;
use oss_mem::MemoryBudget;
use oss_node::tenants::TenantLevels;
use oss_node::usage_route::{UsageService, routes};
use oss_rest::{Router, Server};

struct Logs;

impl oss_blob::usage::Classifier for Logs {
    fn index_of<'a>(&self, key: &'a str) -> Option<&'a str> {
        key.strip_prefix("i/")?.split('/').next()
    }
}

/// A store whose puts are counted on a ledger of `index`es.
fn rig() -> (Arc<Ledger>, oss_blob::UsageStore<oss_blob::Emulator>) {
    let ledger = Arc::new(Ledger::new(16));
    let store = oss_blob::UsageStore::new(
        oss_blob::Emulator::new(1 << 20, 1),
        Arc::clone(&ledger),
        Arc::new(Logs),
    );
    (ledger, store)
}

fn put(store: &oss_blob::UsageStore<oss_blob::Emulator>, index: &str, n: usize) {
    for i in 0..n {
        store
            .put(
                &format!("i/{index}/{i}"),
                &mut oss_blob::SliceSource::new(b"x"),
                &mut [0u8; 16],
            )
            .expect("put");
    }
}

/// Ticks every 10 s for `seconds`, putting `puts` to `index` before each.
fn run(
    levels: &TenantLevels,
    store: &oss_blob::UsageStore<oss_blob::Emulator>,
    index: &[&str],
    puts: usize,
    seconds: i64,
) {
    levels.tick(0);
    let mut at = 10;
    while at <= seconds {
        for name in index {
            put(store, name, puts);
        }
        levels.tick(at * 1_000);
        at += 10;
    }
}

// AC-1
#[test]
fn the_indices_of_a_group_are_one_tenant() {
    let (ledger, store) = rig();
    let levels = TenantLevels::new(ledger, Mode::Observe, 1);
    levels.note_group("a", "acme");
    levels.note_group("b", "acme");
    assert_eq!(levels.tenant_of("a"), "acme");
    assert_eq!(levels.tenant_of("c"), "c");
    run(&levels, &store, &["a", "b"], 2, 80);
    let report = levels.report(80_000);
    let names: Vec<&str> = report.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["acme"]);
    // Four puts every ten seconds: 24 cost units a minute.
    assert!(
        (report[0].1.cost_per_minute - 24.0).abs() < 0.01,
        "{report:?}"
    );
}

// AC-2
#[test]
fn a_tenant_over_its_budget_for_a_minute_is_at_level_one() {
    let (ledger, store) = rig();
    let levels = TenantLevels::new(ledger, Mode::Observe, 10);
    run(&levels, &store, &["busy"], 2, 80);
    assert_eq!(levels.report(80_000)[0].1.level, 1);
    let (ledger, store) = rig();
    let levels = TenantLevels::new(ledger, Mode::Observe, 1_000);
    run(&levels, &store, &["quiet"], 2, 80);
    assert_eq!(levels.report(80_000)[0].1.level, 0);
}

// AC-3
#[test]
fn a_group_that_is_the_index_or_empty_is_not_remembered() {
    let (ledger, _) = rig();
    let levels = TenantLevels::new(ledger, Mode::Observe, 1);
    levels.note_group("a", "acme");
    levels.note_group("a", "");
    assert_eq!(levels.tenant_of("a"), "a");
    levels.note_group("a", "acme");
    levels.note_group("a", "a");
    assert_eq!(levels.tenant_of("a"), "a");
}

// AC-4
#[test]
fn with_levels_off_nothing_is_judged() {
    let (ledger, store) = rig();
    let levels = TenantLevels::new(ledger, Mode::Off, 1);
    run(&levels, &store, &["busy"], 2, 80);
    assert!(levels.report(80_000).is_empty());
}

// AC-5
#[test]
fn the_route_reports_the_mode_and_each_tenant() {
    let (ledger, store) = rig();
    let levels = Arc::new(TenantLevels::new(Arc::clone(&ledger), Mode::Observe, 10));
    run(&levels, &store, &["busy"], 2, 80);
    let service = UsageService::new(ledger, "n1", 5, Arc::new(|| 80_000)).with_tenants(levels);
    let router = routes(Router::new(), Arc::new(service)).expect("route");
    let server = Server::start(
        TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        router,
        &MemoryBudget::new(&support::budget_config()),
    )
    .expect("starts");
    let reply = support::request(server.local_addr(), "GET", "/_serverless/usage", None);
    let body = reply.text();
    assert!(
        body.contains("\"tenant_levels\":{\"mode\":\"observe\",\"tenants\":{\"busy\":{\"level\":1,\"local_level\":1,\"set_level\":0,\"cost_per_minute\":12.000,"),
        "{body}"
    );
    assert!(body.ends_with("\"refused\":0}}}}"), "{body}");
}

// AC-7
#[test]
fn the_ticker_steps_the_levels_until_the_ledger_is_gone() {
    let (ledger, store) = rig();
    let levels = Arc::new(TenantLevels::new(Arc::clone(&ledger), Mode::Observe, 10));
    put(&store, "busy", 1);
    levels.spawn_ticker(std::time::Duration::from_millis(40), Arc::new(|| 1_000));
    let started = std::time::Instant::now();
    while levels.report(1_000).is_empty() {
        assert!(started.elapsed().as_secs() < 5, "never ticked");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(levels.report(1_000)[0].0, "busy");
}

// AC-9
#[test]
fn a_bucket_group_is_written_and_read_back_from_descriptor_settings() {
    use oss_node::writer::{bucket_group_of, group_settings};
    for group in ["acme", "a \"b\\", "ünï"] {
        assert_eq!(
            bucket_group_of(&group_settings(group)).as_deref(),
            Some(group)
        );
    }
    assert_eq!(
        bucket_group_of(br#"{"index":{"serverless":{"bucket_group":"nested"}}}"#).as_deref(),
        Some("nested")
    );
    for none in [
        &b"{}"[..],
        br#"{"index.serverless.bucket_group":""}"#,
        b"nope",
    ] {
        assert_eq!(bucket_group_of(none), None);
    }
}

// docs/specs/tenant-publish-stretch AC-2
#[test]
fn an_enforced_tenant_at_level_one_publishes_four_times_less_often() {
    let (ledger, _) = rig();
    for (mode, want) in [(Mode::Enforce, 4), (Mode::Observe, 1), (Mode::Off, 1)] {
        let levels = TenantLevels::new(Arc::clone(&ledger), mode, 1);
        levels.note_group("a", "acme");
        levels.set_levels("acme=1,other=0");
        assert_eq!(levels.publish_stretch("a", 0), want, "{mode:?}");
        assert_eq!(levels.publish_stretch("unrelated", 0), 1, "{mode:?}");
    }
    let levels = TenantLevels::new(ledger, Mode::Enforce, 1);
    assert_eq!(levels.publish_stretch("a", 0), 1, "no level set");
}

/// A tenant over its budget with documents, held at level 3 from outside,
/// at 80 s: 5 documents a second fit its budget, a burst of 20.
fn paced(mode: Mode) -> TenantLevels {
    let (ledger, store) = rig();
    let levels = TenantLevels::new(ledger, mode, 6);
    levels.tick(0);
    for step in 1..=8_i64 {
        put(&store, "busy", 2);
        assert_eq!(levels.admit_write("busy", 100, step * 10_000), Ok(0));
        levels.tick(step * 10_000);
    }
    levels.set_levels("busy=3");
    levels
}

// AC-1
#[test]
fn documents_written_are_counted_per_tenant_and_reported() {
    let levels = paced(Mode::Observe);
    let report = levels.report(80_000);
    assert!(
        (report[0].1.docs_per_minute - 600.0).abs() < 0.01,
        "{report:?}"
    );
}

// AC-2
#[test]
fn an_enforced_tenant_at_level_three_is_held_to_its_budget_and_refused_past_the_longest_wait() {
    let levels = paced(Mode::Enforce);
    assert_eq!(
        levels.admit_write("busy", 10, 80_000),
        Ok(0),
        "within the burst"
    );
    assert_eq!(
        levels.admit_write("busy", 30, 80_000),
        Ok(4_000),
        "held for the 20 it lacks"
    );
    assert_eq!(
        levels.admit_write("other", 1_000, 80_000),
        Ok(0),
        "another tenant"
    );
    let refused = levels
        .admit_write("busy", 100, 80_000)
        .expect_err("refused");
    assert!(refused > 5_000, "{refused}");
    let observed = paced(Mode::Observe);
    for docs in [10, 30, 100] {
        assert_eq!(observed.admit_write("busy", docs, 80_000), Ok(0));
    }
}
