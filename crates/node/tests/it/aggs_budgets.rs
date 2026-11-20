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
                    "t":{"terms":{"field":"year"}},
                    "h":{"histogram":{"field":"year","interval":10},
                        "aggs":{"m":{"max":{"field":"published"}}}},
                    "r":{"range":{"field":"year","ranges":[{"to":2000},{"from":2000}]},
                        "aggs":{"m":{"max":{"field":"published"}},
                                "g":{"terms":{"field":"tag"}}}},
                    "f":{"filters":{"other_bucket":true,"filters":{"old":{"range":{"year":{"lt":2005}}},"new":{"term":{"tag":"manual"}}}},
                        "aggs":{"c":{"value_count":{"field":"year"}}}}}}"#,
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

#[test]
fn a_cold_search_with_a_keyword_terms_then_a_warm_one() {
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
                    "t":{"terms":{"field":"tag"},"aggs":{"m":{"max":{"field":"year"}}}}}}"#,
            ),
        );
        assert_eq!(r.status, 200, "{}", r.text());
        assert!(
            r.text()
                .contains(r#""buckets":[{"key":"animal","doc_count":1,"m":{"value":"#),
            "{}",
            r.text()
        );
    };

    rounds.reset(DELAY);
    let before = rounds.meter().total();
    search();
    let detail = rounds.detail();
    budget::check(
        "search.keyword_terms.cold.requests",
        rounds.meter().total() - before,
        &detail,
    );
    budget::check(
        "search.keyword_terms.cold.round_trips",
        rounds.depth() as u64,
        &detail,
    );

    rounds.reset(DELAY);
    let before = rounds.meter().total();
    search();
    budget::check(
        "search.keyword_terms.warm.requests",
        rounds.meter().total() - before,
        &rounds.detail(),
    );
}

/// Cold GET count of `body` on a fresh node over a built index of two
/// keyword fields.
fn cold_gets(body: &[u8]) -> u64 {
    let node = crate::support::start();
    let spec = oss_testkit::index::Spec::new("nest", 2_000)
        .segments_of(500)
        .with_keywords(1);
    crate::support::put_built(&node.store, &spec);
    let before = crate::support::counts(&node.store);
    let r = request(node.addr(), "POST", "/nest/_search", Some(body));
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.text().contains(r#""doc_count""#), "{}", r.text());
    let after = crate::support::counts(&node.store);
    before.iter().zip(after).map(|(b, a)| a - b).sum()
}

// docs/specs/keyword-terms-agg AC-14: a keyword `terms` of a second field
// under a keyword `terms` adds no request to a cold search.
#[test]
fn a_nested_keyword_terms_adds_no_request_to_a_cold_search() {
    let plain = cold_gets(br#"{"size":0,"aggs":{"t":{"terms":{"field":"tag"}}}}"#);
    let nested = cold_gets(
        br#"{"size":0,"aggs":{"t":{"terms":{"field":"tag"},
            "aggs":{"k":{"terms":{"field":"k0"},"aggs":{"hi":{"max":{"field":"n"}}}}}}}}"#,
    );
    println!("plain {plain} nested {nested}");
    assert_eq!(nested, plain);
}
