//! docs/specs/anomaly-detection slice 2: the detector store and its routes.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_mem::MemoryBudget;
use oss_node::ad_route::{AdService, WindowSearch, routes};
use oss_rest::{Router, Server};
use serde_json::{Value, json};

const BUF: usize = 1 << 16;
pub(super) const ROOT: &str = "/_plugins/_anomaly_detection";

pub(super) fn detector() -> Value {
    json!({
        "name": "cpu-anomaly",
        "description": "cpu of the web tier",
        "time_field": "timestamp",
        "indices": ["metrics"],
        "feature_attributes": [{
            "feature_name": "cpu_max",
            "feature_enabled": true,
            "aggregation_query": {"cpu_max": {"max": {"field": "cpu"}}}
        }],
        "detection_interval": {"period": {"interval": 10, "unit": "Minutes"}},
        "window_delay": {"period": {"interval": 1, "unit": "Minutes"}}
    })
}

pub(super) struct Rig {
    pub(super) server: Server,
    pub(super) store: Arc<Emulator>,
}

fn rig() -> Rig {
    rig_with(None)
}

/// A rig whose previews read their windows through `search`.
pub(super) fn rig_with(search: Option<Arc<dyn WindowSearch>>) -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let mut service = AdService::new(&blobs, "", BUF);
    if let Some(search) = search {
        service = service.with_search(search);
    }
    let service = Arc::new(service);
    let router = routes(Router::new(), &service).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig { server, store }
}

pub(super) fn call(r: &Rig, method: &str, target: &str, body: Option<&Value>) -> support::Reply {
    let bytes = body.map(|b| b.to_string().into_bytes());
    support::request(r.server.local_addr(), method, target, bytes.as_deref())
}

pub(super) fn json_of(reply: &support::Reply) -> Value {
    serde_json::from_str(&reply.text()).unwrap_or_else(|_| panic!("not json: {}", reply.text()))
}

pub(super) fn total(r: &Rig, op: S3Op) -> u64 {
    r.store.meter().count(op)
}

fn create(r: &Rig, body: &Value) -> String {
    let made = call(r, "POST", &format!("{ROOT}/detectors"), Some(body));
    assert_eq!(made.status, 201, "{}", made.text());
    json_of(&made)["_id"].as_str().expect("an id").to_owned()
}

// AC-4
#[test]
fn a_detector_is_created_read_replaced_searched_counted_and_deleted() {
    let r = rig();
    let made = call(&r, "POST", &format!("{ROOT}/detectors"), Some(&detector()));
    assert_eq!(made.status, 201, "{}", made.text());
    let made = json_of(&made);
    let id = made["_id"].as_str().expect("an id").to_owned();
    assert_eq!(
        (made["_version"].as_i64(), made["_seq_no"].as_i64()),
        (Some(1), Some(0))
    );
    let d = &made["anomaly_detector"];
    assert_eq!(d["shingle_size"], 8, "the default shingle");
    assert_eq!(
        d["filter_query"],
        json!({"match_all": {}}),
        "the default filter"
    );
    assert_eq!(d["detector_type"], "SINGLE_ENTITY");
    assert_eq!(d["detection_interval"]["period"]["interval"], 10);
    assert!(
        d["feature_attributes"][0]["feature_id"]
            .as_str()
            .is_some_and(|f| !f.is_empty()),
        "a feature gets an id"
    );

    let got = call(&r, "GET", &format!("{ROOT}/detectors/{id}"), None);
    assert_eq!(got.status, 200, "{}", got.text());
    assert_eq!(json_of(&got)["anomaly_detector"]["name"], "cpu-anomaly");

    // a replace without a count, then with a stale and with the right one
    let mut changed = detector();
    changed["shingle_size"] = json!(4);
    let put = call(&r, "PUT", &format!("{ROOT}/detectors/{id}"), Some(&changed));
    assert_eq!(put.status, 200, "{}", put.text());
    assert_eq!(json_of(&put)["_seq_no"], 1);
    let stale = call(
        &r,
        "PUT",
        &format!("{ROOT}/detectors/{id}?if_seq_no=0&if_primary_term=1"),
        Some(&changed),
    );
    assert_eq!(stale.status, 409, "{}", stale.text());
    let right = call(
        &r,
        "PUT",
        &format!("{ROOT}/detectors/{id}?if_seq_no=1&if_primary_term=1"),
        Some(&changed),
    );
    assert_eq!(right.status, 200, "{}", right.text());
    assert_eq!(json_of(&right)["anomaly_detector"]["shingle_size"], 4);
    let lone = call(
        &r,
        "PUT",
        &format!("{ROOT}/detectors/{id}?if_seq_no=2"),
        Some(&changed),
    );
    assert_eq!(lone.status, 400, "a count and a term go together");
    let nowhere = call(
        &r,
        "PUT",
        &format!("{ROOT}/detectors/nowhere"),
        Some(&changed),
    );
    assert_eq!(nowhere.status, 404, "{}", nowhere.text());

    let found = call(
        &r,
        "POST",
        &format!("{ROOT}/detectors/_search"),
        Some(&json!({})),
    );
    assert_eq!(found.status, 200, "{}", found.text());
    let found = json_of(&found);
    assert_eq!(found["hits"]["total"]["value"], 1);
    assert_eq!(found["hits"]["hits"][0]["_id"], id.as_str());
    assert_eq!(found["hits"]["hits"][0]["_source"]["name"], "cpu-anomaly");
    let counted = call(&r, "GET", &format!("{ROOT}/detectors/_count"), None);
    assert_eq!(json_of(&counted)["count"], 1);

    let gone = call(&r, "DELETE", &format!("{ROOT}/detectors/{id}"), None);
    assert_eq!(gone.status, 200, "{}", gone.text());
    assert_eq!(json_of(&gone)["result"], "deleted");
    let missing = call(&r, "GET", &format!("{ROOT}/detectors/{id}"), None);
    assert_eq!(missing.status, 404);
    assert!(
        missing.text().contains("Can't find anomaly detector"),
        "{}",
        missing.text()
    );
    assert_eq!(
        call(&r, "DELETE", &format!("{ROOT}/detectors/{id}"), None).status,
        404
    );
    // the legacy prefix is the same tree
    let legacy = call(
        &r,
        "GET",
        "/_opendistro/_anomaly_detection/detectors/_count",
        None,
    );
    assert_eq!(json_of(&legacy)["count"], 0);
}

// AC-5
#[test]
fn an_invalid_detector_is_refused_naming_the_field() {
    let r = rig();
    let with = |key: &str, value: Value| {
        let mut d = detector();
        d[key] = value;
        d
    };
    let feature = |name: &str, aggs: Value| json!({"feature_name": name, "feature_enabled": true, "aggregation_query": aggs});
    let six: Vec<Value> = (0..6)
        .map(|i| feature(&format!("f{i}"), json!({"a": {"max": {"field": "x"}}})))
        .collect();
    let mut no_indices = detector();
    no_indices.as_object_mut().unwrap().remove("indices");
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "no features",
            with("feature_attributes", json!([])),
            "between 1 and 5 features",
        ),
        (
            "six features",
            with("feature_attributes", json!(six)),
            "between 1 and 5 features",
        ),
        (
            "two aggregations",
            with(
                "feature_attributes",
                json!([feature(
                    "f",
                    json!({"a": {"max": {"field": "x"}}, "b": {"min": {"field": "x"}}})
                )]),
            ),
            "exactly one aggregation",
        ),
        (
            "a bucket aggregation",
            with(
                "feature_attributes",
                json!([feature("f", json!({"a": {"terms": {"field": "x"}}}))]),
            ),
            "[terms] is not supported",
        ),
        (
            "an interval in seconds",
            with(
                "detection_interval",
                json!({"period": {"interval": 30, "unit": "Seconds"}}),
            ),
            "must be Minutes, Hours or Days",
        ),
        (
            "an interval of zero",
            with(
                "detection_interval",
                json!({"period": {"interval": 0, "unit": "Minutes"}}),
            ),
            "at least 1 minute",
        ),
        (
            "a shingle of 61",
            with("shingle_size", json!(61)),
            "[shingle_size]",
        ),
        (
            "a shingle of 0",
            with("shingle_size", json!(0)),
            "[shingle_size]",
        ),
        (
            "three category fields",
            with("category_field", json!(["a", "b", "c"])),
            "[category_field]",
        ),
        (
            "a bad result index",
            with("result_index", json!("mine")),
            "[result_index]",
        ),
        (
            "an unsupported filter",
            with("filter_query", json!({"script": {"script": "1"}})),
            "[filter_query]",
        ),
        (
            "rules",
            with("rules", json!([])),
            "suppression rules are not supported",
        ),
        ("no indices", no_indices, "[indices]"),
        ("a bad name", with("name", json!("a/b")), "[name]"),
        ("an empty name", with("name", json!("")), "[name]"),
        (
            "duplicate feature names",
            with(
                "feature_attributes",
                json!([
                    feature("f", json!({"a": {"max": {"field": "x"}}})),
                    feature("f", json!({"b": {"min": {"field": "x"}}}))
                ]),
            ),
            "unique",
        ),
        (
            "only disabled features",
            with(
                "feature_attributes",
                json!([{"feature_name": "f", "feature_enabled": false,
                        "aggregation_query": {"a": {"max": {"field": "x"}}}}]),
            ),
            "at least one feature must be enabled",
        ),
    ];
    for (what, body, reason) in cases {
        let made = call(&r, "POST", &format!("{ROOT}/detectors"), Some(&body));
        assert_eq!(made.status, 400, "{what}: {}", made.text());
        assert!(
            made.text().contains(reason),
            "{what}: wanted [{reason}] in {}",
            made.text()
        );
        assert!(made.text().contains("illegal_argument_exception"), "{what}");
    }
    let junk = support::request(
        r.server.local_addr(),
        "POST",
        &format!("{ROOT}/detectors"),
        Some(b"{"),
    );
    assert_eq!(junk.status, 400);
    // a valid one with the optional parts is kept
    let mut full = detector();
    full["category_field"] = json!(["host"]);
    full["result_index"] = json!("opensearch-ad-plugin-result-web");
    let id = create(&r, &full);
    let got = json_of(&call(&r, "GET", &format!("{ROOT}/detectors/{id}"), None));
    assert_eq!(got["anomaly_detector"]["detector_type"], "MULTI_ENTITY");
    assert_eq!(
        got["anomaly_detector"]["result_index"],
        "opensearch-ad-plugin-result-web"
    );
    // a body over the bound
    let huge = "x".repeat(1 << 21);
    let over = call(
        &r,
        "POST",
        &format!("{ROOT}/detectors"),
        Some(&with("description", json!(huge))),
    );
    assert_eq!(over.status, 400, "{}", over.text());
}

// AC-4: search
#[test]
fn a_search_answers_queries_the_ids_can_answer_and_refuses_the_rest() {
    let r = rig();
    let ids: Vec<String> = (0..3)
        .map(|i| {
            let mut d = detector();
            d["name"] = json!(format!("d{i}"));
            create(&r, &d)
        })
        .collect();
    let mut sorted = ids.clone();
    sorted.sort();
    let search = |body: Value| {
        call(
            &r,
            "POST",
            &format!("{ROOT}/detectors/_search"),
            Some(&body),
        )
    };
    let hit_ids = |reply: &support::Reply| -> Vec<String> {
        json_of(reply)["hits"]["hits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["_id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(
        hit_ids(&search(json!({"query": {"match_all": {}}}))),
        sorted
    );
    assert_eq!(
        hit_ids(&search(json!({"query": {"ids": {"values": [ids[1]]}}}))),
        vec![ids[1].clone()]
    );
    assert_eq!(
        hit_ids(&search(json!({"query": {"term": {"_id": ids[2]}}}))),
        vec![ids[2].clone()]
    );
    let prefix = &sorted[0][..3];
    let found = search(
        json!({"query": {"bool": {"filter": [{"wildcard": {"_id": format!("{prefix}*")}}],
                                       "must_not": [{"ids": {"values": [sorted[0]]}}]}}}),
    );
    assert!(!hit_ids(&found).contains(&sorted[0]), "{}", found.text());
    let page = search(json!({"from": 1, "size": 1}));
    assert_eq!(hit_ids(&page), vec![sorted[1].clone()]);
    assert_eq!(json_of(&page)["hits"]["total"]["value"], 3);
    let desc = search(json!({"sort": [{"_id": {"order": "desc"}}]}));
    assert_eq!(hit_ids(&desc)[0], sorted[2]);
    // a query on a field would read every detector: refused, not paid for
    let gets = total(&r, S3Op::Get);
    let named = search(json!({"query": {"term": {"name.keyword": "d1"}}}));
    assert_eq!(named.status, 400, "{}", named.text());
    assert!(
        named.text().contains("would read every detector"),
        "{}",
        named.text()
    );
    assert_eq!(total(&r, S3Op::Get), gets, "a refused query reads nothing");
    let sorted_by_name = search(json!({"sort": [{"name.keyword": "asc"}]}));
    assert_eq!(sorted_by_name.status, 400, "{}", sorted_by_name.text());
    assert_eq!(
        search(json!({"query": {"match": {"name": "d1"}}})).status,
        400
    );
}

// AC-6
#[test]
fn a_detector_costs_the_stored_scripts_shape_and_nothing_lists() {
    let r = rig();
    let (gets, puts) = (total(&r, S3Op::Get), total(&r, S3Op::Put));
    let id = create(&r, &detector());
    assert_eq!(
        (total(&r, S3Op::Get) - gets, total(&r, S3Op::Put) - puts),
        (1, 2),
        "a create"
    );
    let gets = total(&r, S3Op::Get);
    call(&r, "GET", &format!("{ROOT}/detectors/{id}"), None);
    assert_eq!(total(&r, S3Op::Get) - gets, 2, "a read");
    let (gets, puts) = (total(&r, S3Op::Get), total(&r, S3Op::Put));
    call(
        &r,
        "PUT",
        &format!("{ROOT}/detectors/{id}"),
        Some(&detector()),
    );
    assert_eq!(
        (total(&r, S3Op::Get) - gets, total(&r, S3Op::Put) - puts),
        (3, 2),
        "a replace reads the current detector first"
    );
    let gets = total(&r, S3Op::Get);
    call(
        &r,
        "POST",
        &format!("{ROOT}/detectors/_search"),
        Some(&json!({})),
    );
    assert_eq!(
        total(&r, S3Op::Get) - gets,
        3,
        "a search of one detector: the names, then the marker and content of each hit"
    );
    let gets = total(&r, S3Op::Get);
    call(&r, "GET", &format!("{ROOT}/detectors/_count"), None);
    assert_eq!(total(&r, S3Op::Get) - gets, 1, "a count reads the marker");
    let (gets, puts) = (total(&r, S3Op::Get), total(&r, S3Op::Put));
    call(&r, "DELETE", &format!("{ROOT}/detectors/{id}"), None);
    assert_eq!(
        (total(&r, S3Op::Get) - gets, total(&r, S3Op::Put) - puts),
        (1, 1),
        "a delete"
    );
    assert_eq!(total(&r, S3Op::List), 0);
}

// AC-5: the set holds at most 1,000 detectors
#[test]
fn a_thousand_and_first_detector_is_refused() {
    let r = rig();
    for i in 0..1000 {
        let mut d = detector();
        d["name"] = json!(format!("d{i}"));
        let made = call(&r, "POST", &format!("{ROOT}/detectors"), Some(&d));
        assert_eq!(made.status, 201, "{i}: {}", made.text());
    }
    let over = call(&r, "POST", &format!("{ROOT}/detectors"), Some(&detector()));
    assert_eq!(over.status, 429, "{}", over.text());
}
