//! docs/specs/metric-aggs/SPEC.md AC-6: a search with metric aggregations,
//! held to docs/request-budgets.md.

use std::net::TcpListener;
use std::time::Duration;

use oss_blob::{BlobStore, Emulator, Rounds};
use oss_mem::MemoryBudget;
use oss_node::search::start_reader;
use oss_testkit::budget;

use crate::support::{budget_config, loaded, request, server_config};

/// Each call is held this long, so calls sent together overlap.
const DELAY: Duration = Duration::from_millis(20);

#[test]
fn a_cold_search_with_metric_aggregations_then_a_warm_one() {
    let store = loaded(Rounds::new(Emulator::new(64 << 20, 1)));
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let memory = MemoryBudget::new(&budget_config());
    let (server, _service) = start_reader(
        listener,
        server_config(),
        store.clone(),
        "",
        "node-1",
        &memory,
    )
    .expect("starts");
    let rounds = store.store();
    let search = || {
        let r = request(
            server.local_addr(),
            "POST",
            "/compat-term/_search",
            Some(
                br#"{"query":{"term":{"title":"brown"}},"aggs":{"y":{"stats":{"field":"year"}},
                    "p":{"max":{"field":"published"}},
                    "t":{"terms":{"field":"year"}}}}"#,
            ),
        );
        assert_eq!(r.status, 200, "{}", r.text());
        assert!(r.text().contains(r#""aggregations":"#), "{}", r.text());
    };

    rounds.reset(DELAY);
    let before = rounds.meter().total();
    search();
    let detail = rounds.detail();
    budget::check(
        "search.metric_aggs.cold.requests",
        rounds.meter().total() - before,
        &detail,
    );
    budget::check(
        "search.metric_aggs.cold.round_trips",
        rounds.depth() as u64,
        &detail,
    );

    rounds.reset(DELAY);
    let before = rounds.meter().total();
    search();
    budget::check(
        "search.metric_aggs.warm.requests",
        rounds.meter().total() - before,
        &rounds.detail(),
    );
}
