//! docs/specs/ml-commons slice 1: the model group store and its routes.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_mem::MemoryBudget;
use oss_node::ml_route::{MlService, routes};
use oss_rest::{Router, Server};
use serde_json::Value;

const BUF: usize = 1 << 16;

struct Rig {
    server: Server,
    store: Arc<Emulator>,
}

fn rig() -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let service = Arc::new(MlService::new(&blobs, "", BUF));
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

fn json(reply: &support::Reply) -> Value {
    serde_json::from_str(&reply.text()).unwrap_or_else(|e| panic!("{e}: {}", reply.text()))
}

fn total(r: &Rig, op: S3Op) -> u64 {
    r.store.meter().count(op)
}

const BASE: &str = "/_plugins/_ml/model_groups";

fn register(r: &Rig, body: &str) -> support::Reply {
    call(r, "POST", &format!("{BASE}/_register"), Some(body))
}

fn id_of(reply: &support::Reply) -> String {
    json(reply)["model_group_id"]
        .as_str()
        .expect("an id")
        .to_owned()
}

// AC-1
#[test]
fn a_model_group_is_registered_read_updated_searched_and_deleted() {
    let r = rig();
    let made = register(&r, r#"{"name":"nlp","description":"text models"}"#);
    assert_eq!(made.status, 200, "{}", made.text());
    assert_eq!(json(&made)["status"], "CREATED");
    let id = id_of(&made);

    let read = call(&r, "GET", &format!("{BASE}/{id}"), None);
    assert_eq!(read.status, 200, "{}", read.text());
    let group = json(&read);
    assert_eq!(group["name"], "nlp");
    assert_eq!(group["description"], "text models");
    assert_eq!(group["latest_version"], 0);
    assert_eq!(group["access"], "public");

    let put = call(
        &r,
        "PUT",
        &format!("{BASE}/{id}"),
        Some(r#"{"name":"nlp2","description":"more"}"#),
    );
    assert_eq!(put.status, 200, "{}", put.text());
    assert_eq!(json(&put)["status"], "Updated");
    let read = json(&call(&r, "GET", &format!("{BASE}/{id}"), None));
    assert_eq!(
        (read["name"].as_str(), read["description"].as_str()),
        (Some("nlp2"), Some("more"))
    );

    register(&r, r#"{"name":"vision"}"#);
    let all = call(&r, "POST", &format!("{BASE}/_search"), Some("{}"));
    assert_eq!(all.status, 200, "{}", all.text());
    assert_eq!(json(&all)["hits"]["total"]["value"], 2);
    let exact = json(&call(
        &r,
        "POST",
        &format!("{BASE}/_search"),
        Some(r#"{"query":{"term":{"name.keyword":"vision"}}}"#),
    ));
    assert_eq!(exact["hits"]["total"]["value"], 1);
    assert_eq!(exact["hits"]["hits"][0]["_source"]["name"], "vision");
    let words = json(&call(
        &r,
        "GET",
        &format!("{BASE}/_search"),
        Some(r#"{"query":{"match":{"name":"NLP2 other"}}}"#),
    ));
    assert_eq!(words["hits"]["total"]["value"], 1);

    let gone = call(&r, "DELETE", &format!("{BASE}/{id}"), None);
    assert_eq!(gone.status, 200, "{}", gone.text());
    assert_eq!(json(&gone)["result"], "deleted");
    let after = call(&r, "GET", &format!("{BASE}/{id}"), None);
    assert_eq!(after.status, 404, "{}", after.text());
    assert!(
        after
            .text()
            .contains("Failed to find model group with the provided model group id"),
        "{}",
        after.text()
    );
    assert_eq!(
        call(&r, "DELETE", &format!("{BASE}/{id}"), None).status,
        404
    );
    assert_eq!(
        call(&r, "PUT", &format!("{BASE}/{id}"), Some(r#"{"name":"x"}"#)).status,
        404
    );
}

// AC-1
#[test]
fn a_name_in_use_is_refused_naming_its_holder_and_is_free_after_a_delete_or_rename() {
    let r = rig();
    let first = id_of(&register(&r, r#"{"name":"shared"}"#));
    let again = register(&r, r#"{"name":"shared"}"#);
    assert_eq!(again.status, 400, "{}", again.text());
    assert!(
        again.text().contains(&format!(
            "The name you provided is already being used by a model group with ID: {first}."
        )),
        "{}",
        again.text()
    );
    // a rename onto a held name is the same refusal and changes nothing
    let other = id_of(&register(&r, r#"{"name":"other"}"#));
    let clash = call(
        &r,
        "PUT",
        &format!("{BASE}/{other}"),
        Some(r#"{"name":"shared"}"#),
    );
    assert_eq!(clash.status, 400, "{}", clash.text());
    let read = json(&call(&r, "GET", &format!("{BASE}/{other}"), None));
    assert_eq!(read["name"], "other");
    // a rename frees the old name and a delete frees the name
    let moved = call(
        &r,
        "PUT",
        &format!("{BASE}/{other}"),
        Some(r#"{"name":"moved"}"#),
    );
    assert_eq!(moved.status, 200, "{}", moved.text());
    assert_eq!(register(&r, r#"{"name":"other"}"#).status, 200);
    assert_eq!(
        call(&r, "DELETE", &format!("{BASE}/{first}"), None).status,
        200
    );
    assert_eq!(register(&r, r#"{"name":"shared"}"#).status, 200);
    // a put that keeps its own name is not a clash with itself
    let same = call(
        &r,
        "PUT",
        &format!("{BASE}/{other}"),
        Some(r#"{"name":"moved","description":"d"}"#),
    );
    assert_eq!(same.status, 200, "{}", same.text());
}

// AC-1, AC-18
#[test]
fn a_malformed_register_is_400_and_never_a_panic() {
    let r = rig();
    for body in [
        "",
        "{}",
        r#"{"name":""}"#,
        r#"{"name":"   "}"#,
        r#"{"name":7}"#,
        r#"{"name":"a","description":5}"#,
        "[1]",
        "{not json",
    ] {
        let reply = register(&r, body);
        assert_eq!(reply.status, 400, "{body}: {}", reply.text());
    }
    let long = format!(r#"{{"name":"{}"}}"#, "x".repeat(300));
    assert_eq!(register(&r, &long).status, 400);
    // a refused register leaves no claim behind: the name is free
    assert_eq!(register(&r, r#"{"name":"a"}"#).status, 200);
}

// AC-4
#[test]
fn a_model_group_costs_one_get_and_three_puts_and_nothing_lists() {
    let r = rig();
    let (gets, puts) = (total(&r, S3Op::Get), total(&r, S3Op::Put));
    let id = id_of(&register(&r, r#"{"name":"nlp"}"#));
    assert_eq!(
        (total(&r, S3Op::Get) - gets, total(&r, S3Op::Put) - puts),
        (1, 3),
        "a register"
    );
    let gets = total(&r, S3Op::Get);
    call(&r, "GET", &format!("{BASE}/{id}"), None);
    assert_eq!(total(&r, S3Op::Get) - gets, 2, "a read");
    // an exact name search reads the claim and the document, and the marker
    let gets = total(&r, S3Op::Get);
    call(
        &r,
        "POST",
        &format!("{BASE}/_search"),
        Some(r#"{"query":{"term":{"name.keyword":"nlp"}}}"#),
    );
    assert_eq!(total(&r, S3Op::Get) - gets, 4, "an exact name search");
    assert_eq!(total(&r, S3Op::List), 0);
}

// AC-4, AC-20
#[test]
fn a_search_reads_a_page_of_documents_not_the_set() {
    let r = rig();
    for n in 0..200 {
        let made = register(&r, &format!(r#"{{"name":"g{n}"}}"#));
        assert_eq!(made.status, 200, "{n}: {}", made.text());
    }
    let gets = total(&r, S3Op::Get);
    let page = json(&call(
        &r,
        "POST",
        &format!("{BASE}/_search"),
        Some(r#"{"query":{"match_all":{}},"size":5,"from":10}"#),
    ));
    assert_eq!(page["hits"]["total"]["value"], 200);
    assert_eq!(page["hits"]["hits"].as_array().map(Vec::len), Some(5));
    // the marker once, then one document a hit
    assert_eq!(total(&r, S3Op::Get) - gets, 1 + 5 * 2, "a page");
    let big = call(
        &r,
        "POST",
        &format!("{BASE}/_search"),
        Some(r#"{"size":101}"#),
    );
    assert_eq!(big.status, 400, "{}", big.text());
    let unsupported = call(
        &r,
        "POST",
        &format!("{BASE}/_search"),
        Some(r#"{"query":{"bool":{}}}"#),
    );
    assert_eq!(unsupported.status, 400, "{}", unsupported.text());
}

// AC-20
#[test]
fn a_set_holds_at_most_a_thousand_groups_and_a_refused_register_keeps_no_claim() {
    let r = rig();
    for n in 0..1000 {
        let made = register(&r, &format!(r#"{{"name":"g{n}"}}"#));
        assert_eq!(made.status, 200, "{n}: {}", made.text());
    }
    let over = register(&r, r#"{"name":"one-too-many"}"#);
    assert_eq!(over.status, 429, "{}", over.text());
    // the refusal released its claim: with room made, the name registers
    let first = json(&call(
        &r,
        "POST",
        &format!("{BASE}/_search"),
        Some(r#"{"size":1}"#),
    ));
    let id = first["hits"]["hits"][0]["_id"].as_str().unwrap().to_owned();
    assert_eq!(
        call(&r, "DELETE", &format!("{BASE}/{id}"), None).status,
        200
    );
    assert_eq!(register(&r, r#"{"name":"one-too-many"}"#).status, 200);
}
