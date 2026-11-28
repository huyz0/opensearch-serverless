//! docs/specs/alerting AC-7 and AC-8: a monitor runs once a slot across
//! nodes, and an idle tick makes no request.

use crate::alerting_routes::{BASE, call, monitor, rig};
use crate::support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use oss_blob::{BlobStore, S3Op};
use oss_node::ad_route::{Timing, WindowSearch};
use oss_node::alerting_route::AlertingService;
use oss_node::alerting_run::Scheduler;
use oss_rest::RestError;
use serde_json::{Value, json};

/// The period of the test monitor: five minutes.
const PERIOD: i64 = 5 * 60_000;

struct Counted(Arc<AtomicUsize>);

impl WindowSearch for Counted {
    fn search(&self, _: &[String], _: &Value) -> Result<Value, RestError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(json!({
            "took": 1, "timed_out": false,
            "_shards": {"total": 1, "successful": 1, "skipped": 0, "failed": 0},
            "hits": {"total": {"value": 4, "relation": "eq"}, "max_score": null, "hits": []},
        }))
    }
}

fn node(r: &crate::alerting_routes::Rig, name: &str, searches: &Arc<AtomicUsize>) -> Scheduler {
    let store = Arc::clone(&r.store) as Arc<dyn BlobStore>;
    let service =
        AlertingService::new(&store, "").with_search(Arc::new(Counted(Arc::clone(searches))));
    Scheduler::new(&store, "", service, name, 60_000, Timing::DEFAULT).expect("scheduler")
}

#[test]
fn a_monitor_runs_once_a_slot_across_three_nodes_and_an_idle_tick_is_free() {
    let _switch = support::PAINLESS.lock().unwrap_or_else(|e| e.into_inner());
    oss_reader::painless_switch::set_native_compile(true);
    let r = rig();
    let (status, made) = call(&r, "POST", BASE, Some(&monitor("run", true)));
    assert_eq!(status, 201, "{made}");
    let id = made["_id"].as_str().expect("an id").to_owned();
    let searches = Arc::new(AtomicUsize::new(0));
    let nodes: Vec<Scheduler> = ["a", "b", "c"]
        .iter()
        .map(|n| node(&r, n, &searches))
        .collect();
    let live = ["a", "b", "c"];
    // Before the first slot has ended and the wait has passed, nothing runs.
    for n in &nodes {
        let report = n.tick(PERIOD + 6_999, &live).expect("tick");
        assert!(
            report.ran.is_empty() && report.lost.is_empty(),
            "{report:?}"
        );
    }
    assert_eq!(searches.load(Ordering::SeqCst), 0);
    // At slot 0's due time every node ticks; exactly one runs it.
    let now = PERIOD + Timing::DEFAULT.max_skew_ms + Timing::DEFAULT.visibility_lag_ms;
    let reports: Vec<_> = nodes
        .iter()
        .map(|n| n.tick(now, &live).expect("tick"))
        .collect();
    let ran: usize = reports.iter().map(|r| r.ran.len()).sum();
    assert_eq!(ran, 1, "{reports:?}");
    assert_eq!(searches.load(Ordering::SeqCst), 1);
    assert!(
        reports
            .iter()
            .filter(|r| !r.ran.is_empty() || !r.lost.is_empty())
            .count()
            <= 2,
        "only the top two nodes try the claim: {reports:?}"
    );
    let (_, alerts) = call(&r, "GET", &format!("{BASE}/alerts?monitorId={id}"), None);
    assert_eq!(alerts["totalAlerts"], 1, "{alerts}");
    // The same slot again, and a tick with nothing due, send no request.
    let meter = r.store.meter();
    let total = || S3Op::ALL.iter().map(|op| meter.count(*op)).sum::<u64>();
    let at = total();
    for n in &nodes {
        let report = n.tick(now + 1_000, &live).expect("tick");
        assert!(report.ran.is_empty() && !report.refreshed, "{report:?}");
    }
    assert_eq!(total(), at, "an idle tick sends no request");
    // The next slot runs once more.
    let next = now + PERIOD;
    let ran: usize = nodes
        .iter()
        .map(|n| n.tick(next, &live).expect("tick").ran.len())
        .sum();
    assert_eq!(ran, 1);
    assert_eq!(searches.load(Ordering::SeqCst), 2);
    oss_reader::painless_switch::set_native_compile(false);
}
