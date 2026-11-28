//! docs/specs/ism slice 1: the policy store and its routes.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_format::IndexRecord;
use oss_mem::MemoryBudget;
use oss_meta::{DeleteMode, DescriptorStore};
use oss_node::ism_route::{IsmService, routes};
use oss_rest::{Router, Server};

const BUF: usize = 1 << 16;
const POLICY: &str = r#"{"policy":{"description":"logs","default_state":"hot","states":[
  {"name":"hot","actions":[{"rollover":{"min_index_age":"1d"}}],
   "transitions":[{"state_name":"warm","conditions":{"min_index_age":"2d"}}]},
  {"name":"warm","actions":[{"read_only":{}}],
   "transitions":[{"state_name":"gone","conditions":{"min_index_age":"30d"}}]},
  {"name":"gone","actions":[{"delete":{}}],"transitions":[]}]}}"#;

struct Rig {
    server: Server,
    store: Arc<Emulator>,
}

fn rig() -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let descriptors =
        DescriptorStore::new(Arc::clone(&blobs), "", DeleteMode::Conditional).expect("descriptors");
    let service = Arc::new(IsmService::new(&blobs, "", BUF).with_descriptors(descriptors));
    let router = routes(Router::new(), &service).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig { server, store }
}

fn call(r: &Rig, method: &str, target: &str, body: Option<&str>) -> support::Reply {
    support::request(
        r.server.local_addr(),
        method,
        target,
        body.map(str::as_bytes),
    )
}

fn total(r: &Rig, op: S3Op) -> u64 {
    r.store.meter().count(op)
}

fn with(policy: &str, from: &str, to: &str) -> String {
    assert!(policy.contains(from), "{from}");
    policy.replacen(from, to, 1)
}

// AC-1
#[test]
fn a_policy_is_created_read_replaced_listed_and_deleted() {
    let r = rig();
    let made = call(&r, "PUT", "/_plugins/_ism/policies/logs", Some(POLICY));
    assert_eq!(made.status, 201, "{}", made.text());
    let body: serde_json::Value = serde_json::from_str(&made.text()).unwrap();
    assert_eq!(body["_id"], "logs");
    assert_eq!(body["_version"], 1);
    assert_eq!(body["_seq_no"], 0);
    assert_eq!(body["_primary_term"], 1);
    assert_eq!(body["policy"]["policy_id"], "logs");
    assert_eq!(body["policy"]["default_state"], "hot");

    let read = call(&r, "GET", "/_plugins/_ism/policies/logs", None);
    assert_eq!(read.status, 200, "{}", read.text());
    let read: serde_json::Value = serde_json::from_str(&read.text()).unwrap();
    assert_eq!(read["policy"]["states"][0]["name"], "hot");

    // a replace without the sequence numbers is a conflict, as the plugin's
    let bare = call(&r, "PUT", "/_plugins/_ism/policies/logs", Some(POLICY));
    assert_eq!(bare.status, 409, "{}", bare.text());
    let stale = call(
        &r,
        "PUT",
        "/_plugins/_ism/policies/logs?if_seq_no=7&if_primary_term=1",
        Some(POLICY),
    );
    assert_eq!(stale.status, 409, "{}", stale.text());
    let ok = call(
        &r,
        "PUT",
        "/_plugins/_ism/policies/logs?if_seq_no=0&if_primary_term=1",
        Some(POLICY),
    );
    assert_eq!(ok.status, 200, "{}", ok.text());
    let ok: serde_json::Value = serde_json::from_str(&ok.text()).unwrap();
    assert_eq!(
        (ok["_seq_no"].as_i64(), ok["_version"].as_i64()),
        (Some(1), Some(2))
    );

    // the legacy prefix is the same tree
    let legacy = call(&r, "GET", "/_opendistro/_ism/policies/logs", None);
    assert_eq!(legacy.status, 200, "{}", legacy.text());

    call(&r, "PUT", "/_plugins/_ism/policies/another", Some(POLICY));
    let list = call(&r, "GET", "/_plugins/_ism/policies", None);
    assert_eq!(list.status, 200, "{}", list.text());
    let list: serde_json::Value = serde_json::from_str(&list.text()).unwrap();
    assert_eq!(list["total_policies"], 2);
    assert_eq!(list["policies"][0]["_id"], "another");

    let gone = call(&r, "DELETE", "/_plugins/_ism/policies/logs", None);
    assert_eq!(gone.status, 200, "{}", gone.text());
    assert!(gone.text().contains(r#""result":"deleted""#));
    let after = call(&r, "GET", "/_plugins/_ism/policies/logs", None);
    assert_eq!(after.status, 404, "{}", after.text());
    assert!(
        after.text().contains("Policy not found"),
        "{}",
        after.text()
    );
    let again = call(&r, "DELETE", "/_plugins/_ism/policies/logs", None);
    assert_eq!(again.status, 404, "{}", again.text());
}

// AC-1
#[test]
fn an_invalid_policy_is_refused_naming_what_is_wrong() {
    let r = rig();
    let cases = [
        (
            with(
                POLICY,
                "\"state_name\":\"warm\"",
                "\"state_name\":\"nowhere\"",
            ),
            "nowhere",
        ),
        (
            with(
                POLICY,
                "\"default_state\":\"hot\"",
                "\"default_state\":\"cold\"",
            ),
            "cold",
        ),
        (
            with(POLICY, "{\"read_only\":{}}", "{\"close\":{}}"),
            "close",
        ),
        (
            with(
                POLICY,
                "{\"read_only\":{}}",
                "{\"force_merge\":{\"max_num_segments\":1}}",
            ),
            "force_merge",
        ),
        (
            with(POLICY, "\"min_index_age\":\"1d\"", "\"min_doc_count\":5"),
            "min_doc_count",
        ),
        (
            with(POLICY, "\"min_index_age\":\"2d\"", "\"min_doc_count\":5"),
            "min_doc_count",
        ),
        (
            with(POLICY, "\"min_index_age\":\"1d\"", "\"min_size\":\"5gb\""),
            "min_size",
        ),
        (
            with(POLICY, "{\"name\":\"warm\"", "{\"name\":\"hot\""),
            "hot",
        ),
        (
            "{\"policy\":{\"default_state\":\"a\",\"states\":[]}}".to_owned(),
            "states",
        ),
        ("not json".to_owned(), "body"),
    ];
    for (body, word) in cases {
        let reply = call(&r, "PUT", "/_plugins/_ism/policies/bad", Some(&body));
        assert_eq!(reply.status, 400, "{body}: {}", reply.text());
        assert!(
            reply.text().contains("illegal_argument_exception") && reply.text().contains(word),
            "{word}: {}",
            reply.text()
        );
    }
    assert_eq!(
        call(&r, "GET", "/_plugins/_ism/policies/bad", None).status,
        404
    );
}

// AC-2
#[test]
fn a_policy_costs_the_stored_scripts_shape_and_nothing_lists() {
    let r = rig();
    let (gets, puts) = (total(&r, S3Op::Get), total(&r, S3Op::Put));
    call(&r, "PUT", "/_plugins/_ism/policies/logs", Some(POLICY));
    assert_eq!(
        (total(&r, S3Op::Get) - gets, total(&r, S3Op::Put) - puts),
        (1, 2),
        "a create"
    );
    let gets = total(&r, S3Op::Get);
    call(&r, "GET", "/_plugins/_ism/policies/logs", None);
    assert_eq!(total(&r, S3Op::Get) - gets, 2, "a read");
    let (gets, puts) = (total(&r, S3Op::Get), total(&r, S3Op::Put));
    call(&r, "DELETE", "/_plugins/_ism/policies/logs", None);
    assert_eq!(
        (total(&r, S3Op::Get) - gets, total(&r, S3Op::Put) - puts),
        (1, 1),
        "a delete"
    );
    assert_eq!(total(&r, S3Op::List), 0);
}

// AC-1: the set holds at most 1,000 policies
#[test]
fn a_thousand_and_first_policy_is_refused() {
    let r = rig();
    for i in 0..1000 {
        let put = call(
            &r,
            "PUT",
            &format!("/_plugins/_ism/policies/p{i}"),
            Some(POLICY),
        );
        assert_eq!(put.status, 201, "{i}: {}", put.text());
    }
    let over = call(&r, "PUT", "/_plugins/_ism/policies/over", Some(POLICY));
    assert_eq!(over.status, 429, "{}", over.text());
}

/// An index record as the coordinator writes one, created at `created`.
fn seed(r: &Rig, name: &str, created: i64) {
    let blobs = Arc::clone(&r.store) as Arc<dyn BlobStore>;
    let descriptors =
        DescriptorStore::new(blobs, "", DeleteMode::Conditional).expect("descriptors");
    let uuid = format!("uuid-{name}");
    let mut record = IndexRecord::new(name, &uuid, 1);
    record.created_at_millis = created;
    descriptors
        .create(&record, 1, &mut vec![0u8; BUF])
        .expect("index");
}

fn json(reply: &support::Reply) -> serde_json::Value {
    serde_json::from_str(&reply.text()).unwrap_or_else(|e| panic!("{e}: {}", reply.text()))
}

const ADD: &str = r#"{"policy_id":"logs"}"#;

// AC-3
#[test]
fn a_policy_is_added_to_an_index_explained_and_removed() {
    let r = rig();
    seed(&r, "logs-1", 1_700_000_000_000);
    call(&r, "PUT", "/_plugins/_ism/policies/logs", Some(POLICY));

    let none = call(&r, "GET", "/_plugins/_ism/explain/logs-1", None);
    assert_eq!(none.status, 200, "{}", none.text());
    let none = json(&none);
    assert!(none["logs-1"]["index.plugins.index_state_management.policy_id"].is_null());
    assert_eq!(none["total_managed_indices"], 0);

    let added = call(&r, "POST", "/_plugins/_ism/add/logs-1", Some(ADD));
    assert_eq!(added.status, 200, "{}", added.text());
    assert_eq!(
        json(&added),
        serde_json::json!({"updated_indices":1,"failures":false,"failed_indices":[]})
    );

    let explained = json(&call(&r, "GET", "/_plugins/_ism/explain/logs-1", None));
    let entry = &explained["logs-1"];
    assert_eq!(
        entry["index.plugins.index_state_management.policy_id"],
        "logs"
    );
    assert_eq!(
        entry["index.opendistro.index_state_management.policy_id"],
        "logs"
    );
    assert_eq!(entry["index"], "logs-1");
    assert_eq!(entry["index_uuid"], "uuid-logs-1");
    assert_eq!(entry["policy_id"], "logs");
    assert_eq!(entry["policy_seq_no"], 0);
    assert_eq!(entry["index_creation_date"], 1_700_000_000_000_i64);
    assert_eq!(entry["state"]["name"], "hot");
    assert_eq!(entry["enabled"], true);
    assert_eq!(explained["total_managed_indices"], 1);

    // the legacy prefix and POST are the same route
    let legacy = call(&r, "POST", "/_opendistro/_ism/explain/logs-1", None);
    assert_eq!(legacy.status, 200, "{}", legacy.text());

    let again = json(&call(&r, "POST", "/_plugins/_ism/add/logs-1", Some(ADD)));
    assert_eq!(again["updated_indices"], 0);
    assert_eq!(again["failures"], true);
    assert_eq!(again["failed_indices"][0]["index_name"], "logs-1");
    assert!(
        again["failed_indices"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("already has a policy")
    );

    let removed = json(&call(&r, "POST", "/_plugins/_ism/remove/logs-1", None));
    assert_eq!(removed["updated_indices"], 1);
    let after = json(&call(&r, "GET", "/_plugins/_ism/explain/logs-1", None));
    assert!(after["logs-1"]["index.plugins.index_state_management.policy_id"].is_null());
    let twice = json(&call(&r, "POST", "/_plugins/_ism/remove/logs-1", None));
    assert_eq!(
        (
            twice["updated_indices"].as_i64(),
            twice["failures"].as_bool()
        ),
        (Some(0), Some(true))
    );
}

// AC-3
#[test]
fn an_add_that_cannot_attach_is_a_failure_entry_naming_why() {
    let r = rig();
    seed(&r, "logs-1", 1);
    call(&r, "PUT", "/_plugins/_ism/policies/logs", Some(POLICY));
    let cases = [
        ("logs-1", r#"{"policy_id":"nope"}"#, "Policy not found"),
        ("ghost", ADD, "no such index"),
    ];
    for (index, body, word) in cases {
        let reply = call(
            &r,
            "POST",
            &format!("/_plugins/_ism/add/{index}"),
            Some(body),
        );
        assert_eq!(reply.status, 200, "{}", reply.text());
        let reply = json(&reply);
        assert_eq!(reply["updated_indices"], 0, "{reply}");
        assert!(
            reply["failed_indices"][0]["reason"]
                .as_str()
                .unwrap()
                .contains(word),
            "{reply}"
        );
    }
    for (target, body) in [("logs-*", ADD), ("logs-1", "{}"), ("logs-1", "not json")] {
        let reply = call(
            &r,
            "POST",
            &format!("/_plugins/_ism/add/{target}"),
            Some(body),
        );
        assert_eq!(reply.status, 400, "{target} {body}: {}", reply.text());
    }
}

// AC-4b (slice 2: the record's cost; the run register and registry come with the runner)
#[test]
fn adding_reading_and_removing_a_policy_cost_their_requests_and_nothing_lists() {
    let r = rig();
    seed(&r, "logs-1", 1);
    call(&r, "PUT", "/_plugins/_ism/policies/logs", Some(POLICY));
    let (gets, puts) = (total(&r, S3Op::Get), total(&r, S3Op::Put));
    call(&r, "POST", "/_plugins/_ism/add/logs-1", Some(ADD));
    assert_eq!(
        (total(&r, S3Op::Get) - gets, total(&r, S3Op::Put) - puts),
        (4, 2),
        "an add: the policy's 2 GETs, the index's 1, the record's 1 GET and 2 PUTs"
    );
    let gets = total(&r, S3Op::Get);
    call(&r, "GET", "/_plugins/_ism/explain/logs-1", None);
    assert_eq!(
        total(&r, S3Op::Get) - gets,
        2,
        "an explain of a managed index"
    );
    let (gets, puts) = (total(&r, S3Op::Get), total(&r, S3Op::Put));
    call(&r, "POST", "/_plugins/_ism/remove/logs-1", None);
    assert_eq!(
        (total(&r, S3Op::Get) - gets, total(&r, S3Op::Put) - puts),
        (1, 1),
        "a remove"
    );
    let gets = total(&r, S3Op::Get);
    call(&r, "GET", "/_plugins/_ism/explain/logs-1", None);
    assert_eq!(
        total(&r, S3Op::Get) - gets,
        1,
        "an explain of an unmanaged index"
    );
    assert_eq!(total(&r, S3Op::List), 0);
}

// AC-11
#[test]
fn a_tenant_holds_at_most_a_thousand_managed_indices() {
    let r = rig();
    call(&r, "PUT", "/_plugins/_ism/policies/logs", Some(POLICY));
    for i in 0..=1000 {
        seed(&r, &format!("i{i}"), 1);
    }
    for i in 0..1000 {
        let add = call(&r, "POST", &format!("/_plugins/_ism/add/i{i}"), Some(ADD));
        assert_eq!(add.status, 200, "{i}: {}", add.text());
    }
    let over = call(&r, "POST", "/_plugins/_ism/add/i1000", Some(ADD));
    assert_eq!(over.status, 429, "{}", over.text());
}
