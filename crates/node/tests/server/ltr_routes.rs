//! docs/specs/ltr slice 2: the feature store and its routes.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_mem::MemoryBudget;
use oss_node::ltr_route::{LtrService, routes};
use oss_rest::{Router, Server};

const BUF: usize = 1 << 16;
const ACK: &str = r#"{"acknowledged":true}"#;
const SET: &str = r#"{"featureset":{"features":[
  {"name":"title","params":["q"],"template_language":"mustache","template":{"match":{"title":"{{q}}"}}},
  {"name":"views","params":[],"template":{"match_all":{}}}]}}"#;
const LINEAR: &str = r#"{"model":{"name":"lin","model":{"type":"model/linear","definition":"{\"title\":2.0,\"views\":0.5}"}}}"#;

struct Rig {
    server: Server,
    store: Arc<Emulator>,
}

fn rig() -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let service = Arc::new(LtrService::new(&blobs, "", BUF));
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

// AC-5
#[test]
fn a_store_a_feature_set_and_a_model_are_created_read_replaced_listed_and_deleted() {
    let r = rig();
    // no store yet: 404
    let none = call(&r, "PUT", "/_ltr/_featureset/fs", Some(SET));
    assert_eq!(none.status, 404, "{}", none.text());
    let made = call(&r, "PUT", "/_ltr", None);
    assert_eq!((made.status, made.text().as_str()), (200, ACK));
    let again = call(&r, "PUT", "/_ltr", None);
    assert_eq!(again.status, 400, "{}", again.text());
    assert!(again.text().contains("resource_already_exists_exception"));

    let put = call(&r, "PUT", "/_ltr/_featureset/fs", Some(SET));
    assert_eq!(put.status, 201, "{}", put.text());
    assert!(
        put.text().contains(r#""result":"created""#),
        "{}",
        put.text()
    );
    assert!(
        put.text().contains(r#""_id":"featureset-fs""#),
        "{}",
        put.text()
    );
    let replaced = call(&r, "POST", "/_plugins/_ltr/_featureset/fs", Some(SET));
    assert_eq!(replaced.status, 200, "{}", replaced.text());
    assert!(
        replaced.text().contains(r#""result":"updated""#),
        "{}",
        replaced.text()
    );

    let got = call(&r, "GET", "/_ltr/_featureset/fs", None);
    assert_eq!(got.status, 200, "{}", got.text());
    assert!(
        got.text().contains(r#""found":true"#) && got.text().contains("title"),
        "{}",
        got.text()
    );
    let missing = call(&r, "GET", "/_ltr/_featureset/nope", None);
    assert_eq!(missing.status, 404, "{}", missing.text());
    assert!(
        missing.text().contains(r#""found":false"#),
        "{}",
        missing.text()
    );

    let model = call(
        &r,
        "POST",
        "/_ltr/_featureset/fs/_createmodel",
        Some(LINEAR),
    );
    assert_eq!(model.status, 201, "{}", model.text());
    let read = call(&r, "GET", "/_ltr/_model/lin", None);
    assert_eq!(read.status, 200, "{}", read.text());
    assert!(
        read.text().contains("model/linear") && read.text().contains("feature_set"),
        "{}",
        read.text()
    );

    let sets = call(&r, "GET", "/_ltr/_featureset?prefix=f", None);
    assert!(sets.text().contains(r#""value":1"#), "{}", sets.text());
    let models = call(&r, "GET", "/_ltr/_model", None);
    assert!(models.text().contains(r#""value":1"#), "{}", models.text());
    let stores = call(&r, "GET", "/_ltr", None);
    assert!(
        stores.text().contains(r#""default":true"#),
        "{}",
        stores.text()
    );

    let gone = call(&r, "DELETE", "/_ltr/_model/lin", None);
    assert_eq!(gone.status, 200, "{}", gone.text());
    let again = call(&r, "DELETE", "/_ltr/_model/lin", None);
    assert_eq!(again.status, 404, "{}", again.text());
    assert_eq!(call(&r, "GET", "/_ltr/_model/lin", None).status, 404);
    let deleted = call(&r, "DELETE", "/_ltr", None);
    assert_eq!(deleted.status, 200, "{}", deleted.text());
    assert_eq!(call(&r, "GET", "/_ltr/_featureset/fs", None).status, 404);
}

// AC-5: a named store is its own
#[test]
fn a_named_store_holds_its_own_objects() {
    let r = rig();
    assert_eq!(call(&r, "PUT", "/_ltr/shop", None).status, 200);
    assert_eq!(
        call(&r, "PUT", "/_ltr/shop/_featureset/fs", Some(SET)).status,
        201
    );
    assert_eq!(
        call(&r, "GET", "/_ltr/shop/_featureset/fs", None).status,
        200
    );
    // the default store was never made
    assert_eq!(call(&r, "GET", "/_ltr/_featureset/fs", None).status, 404);
    let stores = call(&r, "GET", "/_ltr", None);
    assert!(
        stores.text().contains(".ltrstore_shop"),
        "{}",
        stores.text()
    );
    assert_eq!(call(&r, "PUT", "/_ltr/bad%2Fname", None).status, 400);
    assert_eq!(call(&r, "DELETE", "/_ltr/shop", None).status, 200);
    assert_eq!(
        call(&r, "GET", "/_ltr/shop/_featureset/fs", None).status,
        404
    );
}

// AC-5: an invalid object is a 400 with a reason
#[test]
fn an_invalid_feature_set_or_model_is_a_400() {
    let r = rig();
    assert_eq!(call(&r, "PUT", "/_ltr", None).status, 200);
    for (body, why) in [
        (r#"{}"#, "[featureset]"),
        (r#"{"featureset":{"features":[{"name":"a"}]}}"#, "template"),
        (
            r#"{"featureset":{"features":[{"name":"_a","template":"x"}]}}"#,
            "'_'",
        ),
        (
            r#"{"featureset":{"features":[{"name":"a","template":"x"},{"name":"a","template":"y"}]}}"#,
            "twice",
        ),
        (
            r#"{"featureset":{"name":"other","features":[]}}"#,
            "does not match",
        ),
    ] {
        let put = call(&r, "PUT", "/_ltr/_featureset/fs", Some(body));
        assert_eq!(put.status, 400, "{body}: {}", put.text());
        assert!(put.text().contains(why), "{body}: {}", put.text());
    }
    assert_eq!(
        call(&r, "PUT", "/_ltr/_featureset/_bad", Some(SET)).status,
        400
    );
    assert_eq!(
        call(&r, "PUT", "/_ltr/_featureset/fs", Some("not json")).status,
        400
    );
    assert_eq!(
        call(&r, "PUT", "/_ltr/_featureset/fs", Some(SET)).status,
        201
    );

    let no_set = call(
        &r,
        "POST",
        "/_ltr/_featureset/none/_createmodel",
        Some(LINEAR),
    );
    assert_eq!(no_set.status, 404, "{}", no_set.text());
    for (body, why) in [
        (LINEAR.replace("title", "nope"), "[nope]"),
        (LINEAR.replace("model/linear", "model/other"), "unknown model type"),
        (LINEAR.replace("\"name\":\"lin\",", ""), "needs a [name]"),
        (
            r###"{"model":{"name":"m","model":{"type":"model/ranklib","definition":"## AdaRank\n<ensemble/>"}}}"###.to_owned(),
            "AdaRank",
        ),
    ] {
        let put = call(&r, "POST", "/_ltr/_featureset/fs/_createmodel", Some(body.as_str()));
        assert_eq!(put.status, 400, "{body}: {}", put.text());
        assert!(put.text().contains(why), "{body}: {}", put.text());
    }
}

// AC-7
#[test]
fn addfeatures_extends_a_set_and_createmodel_scores_its_features() {
    let r = rig();
    assert_eq!(call(&r, "PUT", "/_ltr", None).status, 200);
    let added = call(
        &r,
        "POST",
        "/_ltr/_featureset/grow/_addfeatures",
        Some(r#"{"features":[{"name":"a","template":{"match_all":{}}}]}"#),
    );
    assert_eq!(added.status, 201, "{}", added.text());
    let more = call(
        &r,
        "POST",
        "/_ltr/_featureset/grow/_addfeatures",
        Some(r#"{"features":[{"name":"b","template":{"match_all":{}}}]}"#),
    );
    assert_eq!(more.status, 200, "{}", more.text());
    let dup = call(
        &r,
        "POST",
        "/_ltr/_featureset/grow/_addfeatures",
        Some(r#"{"features":[{"name":"a","template":{"match_all":{}}}]}"#),
    );
    assert_eq!(dup.status, 400, "{}", dup.text());
    let got = call(&r, "GET", "/_ltr/_featureset/grow", None);
    assert!(
        got.text().contains("\"a\"") && got.text().contains("\"b\""),
        "{}",
        got.text()
    );
    let model =
        r#"{"model":{"name":"m","model":{"type":"model/linear","definition":{"a":1.0,"b":2.0}}}}"#;
    assert_eq!(
        call(
            &r,
            "POST",
            "/_ltr/_featureset/grow/_createmodel",
            Some(model)
        )
        .status,
        201
    );
    // a model put with its set embedded
    let embedded = r#"{"model":{"name":"m2","model":{"type":"model/linear","definition":"{\"x\":1}"},
        "feature_set":{"name":"emb","features":[{"name":"x","template":"q"}]}}}"#;
    assert_eq!(
        call(&r, "PUT", "/_ltr/_model/m2", Some(embedded)).status,
        201
    );
    assert_eq!(
        call(&r, "PUT", "/_ltr/_model/other", Some(embedded)).status,
        400
    );
}

// AC-6
#[test]
fn a_put_costs_one_get_and_two_puts_a_read_two_gets_and_nothing_lists() {
    let r = rig();
    assert_eq!(call(&r, "PUT", "/_ltr", None).status, 200);
    let (gets, puts) = (total(&r, S3Op::Get), total(&r, S3Op::Put));
    assert_eq!(
        call(&r, "PUT", "/_ltr/_featureset/fs", Some(SET)).status,
        201
    );
    assert_eq!(
        (total(&r, S3Op::Get) - gets, total(&r, S3Op::Put) - puts),
        (1, 2),
        "a put"
    );
    let gets = total(&r, S3Op::Get);
    assert_eq!(call(&r, "GET", "/_ltr/_featureset/fs", None).status, 200);
    assert_eq!(total(&r, S3Op::Get) - gets, 2, "a read");
    assert_eq!(total(&r, S3Op::List), 0);
}
