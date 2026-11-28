//! docs/specs/ml-commons slice 1b: connectors, their validation and their sealed credentials.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, ByteRange, Emulator, S3Op};
use oss_mem::MemoryBudget;
use oss_ml::connector::Trusted;
use oss_node::ml_route::{MlService, routes};
use oss_rest::{Router, Server};
use serde_json::{Value, json};

const BUF: usize = 1 << 16;
const BASE: &str = "/_plugins/_ml/connectors";
const TRUSTED: &str = r"^http://127\.0\.0\.1:18080/.*$";

struct Rig {
    server: Server,
    store: Arc<Emulator>,
}

fn rig() -> Rig {
    rig_over(Arc::new(Emulator::new(1 << 28, 5)), TRUSTED)
}

/// A node over `store`; another node over the same store is another call.
fn rig_over(store: Arc<Emulator>, trusted: &str) -> Rig {
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let service = MlService::new(&blobs, "", BUF).with_trusted(Trusted::parse(trusted).unwrap());
    let router = routes(Router::new(), &Arc::new(service)).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig { server, store }
}

fn call(r: &Rig, method: &str, target: &str, body: Option<&Value>) -> support::Reply {
    let bytes = body.map(Value::to_string);
    support::request(
        r.server.local_addr(),
        method,
        target,
        bytes.as_deref().map(str::as_bytes),
    )
}

fn json_of(reply: &support::Reply) -> Value {
    serde_json::from_str(&reply.text()).unwrap_or_else(|e| panic!("{e}: {}", reply.text()))
}

fn total(r: &Rig, op: S3Op) -> u64 {
    r.store.meter().count(op)
}

fn def(name: &str) -> Value {
    json!({"name": name, "description": "d", "version": "1", "protocol": "http",
           "parameters": {"endpoint": "127.0.0.1:18080"},
           "credential": {"api_key": "sk-plaintext-secret"},
           "actions": [{"action_type": "predict", "method": "POST",
                        "url": "http://127.0.0.1:18080/json",
                        "headers": {"x": "y"},
                        "request_body": "{\"input\":\"${parameters.input}\"}"}]})
}

fn create(r: &Rig, body: &Value) -> support::Reply {
    call(r, "POST", &format!("{BASE}/_create"), Some(body))
}

fn id_of(reply: &support::Reply) -> String {
    json_of(reply)["connector_id"]
        .as_str()
        .expect("an id")
        .to_owned()
}

/// Every key the test store holds.
fn all_keys(r: &Rig) -> Vec<String> {
    let mut keys = Vec::new();
    // LIST: a test inspecting what the store holds, after the requests under test.
    r.store
        .list_prefix("", None, 1000, &mut |k, _| keys.push(k.to_owned()))
        .unwrap();
    keys
}

fn raw(r: &Rig, key: &str) -> Option<String> {
    let mut bytes = Vec::new();
    let mut buf = [0u8; 4096];
    let mut sink = |c: &[u8]| {
        bytes.extend_from_slice(c);
        Ok(())
    };
    r.store
        .get(key, ByteRange::Full, &mut buf, &mut sink)
        .ok()?;
    String::from_utf8(bytes).ok()
}

// AC-1
#[test]
fn a_connector_is_created_read_updated_searched_and_deleted_in_stocks_shapes() {
    let r = rig();
    let made = create(&r, &def("compat-c1"));
    assert_eq!(made.status, 200, "{}", made.text());
    let id = id_of(&made);

    let read = json_of(&call(&r, "GET", &format!("{BASE}/{id}"), None));
    assert_eq!(read["name"], "compat-c1");
    assert_eq!(read["actions"][0]["action_type"], "PREDICT");
    assert_eq!(read["parameters"]["endpoint"], "127.0.0.1:18080");
    assert!(read.get("credential").is_none(), "{read}");
    assert!(read["created_time"].is_u64() && read["last_updated_time"].is_u64());

    let put = call(
        &r,
        "PUT",
        &format!("{BASE}/{id}"),
        Some(&json!({"description": "changed"})),
    );
    assert_eq!(put.status, 200, "{}", put.text());
    let put = json_of(&put);
    assert_eq!(
        (put["result"].as_str(), put["_id"].as_str()),
        (Some("updated"), Some(id.as_str()))
    );
    assert_eq!(put["_index"], ".plugins-ml-connector");
    let read = json_of(&call(&r, "GET", &format!("{BASE}/{id}"), None));
    assert_eq!(
        (read["description"].as_str(), read["name"].as_str()),
        (Some("changed"), Some("compat-c1"))
    );

    create(&r, &def("alpha"));
    let search = call(
        &r,
        "POST",
        &format!("{BASE}/_search"),
        Some(&json!({"query": {"match_all": {}}, "sort": [{"name.keyword": "asc"}]})),
    );
    let hits = json_of(&search)["hits"]["hits"].clone();
    let names: Vec<_> = hits
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["_source"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["alpha", "compat-c1"]);
    assert!(hits[0]["_source"].get("credential").is_none(), "{hits}");
    assert_eq!(hits[0]["_index"], ".plugins-ml-connector");

    let del = call(&r, "DELETE", &format!("{BASE}/{id}"), None);
    assert_eq!(del.status, 200, "{}", del.text());
    assert_eq!(json_of(&del)["result"], "deleted");
    let gone = call(&r, "GET", &format!("{BASE}/{id}"), None);
    assert_eq!(gone.status, 404);
    assert_eq!(
        json_of(&gone)["error"]["reason"],
        format!("Failed to find connector with the provided connector id: {id}")
    );
    assert_eq!(
        call(&r, "DELETE", &format!("{BASE}/{id}"), None).status,
        404
    );
    assert_eq!(
        call(
            &r,
            "PUT",
            &format!("{BASE}/{id}"),
            Some(&json!({"name": "x"}))
        )
        .status,
        404
    );
}

// AC-2
#[test]
fn a_credential_is_stored_sealed_and_never_returned() {
    let r = rig();
    let id = id_of(&create(&r, &def("c")));
    let keys = all_keys(&r);
    for key in &keys {
        let body = raw(&r, key).unwrap_or_default();
        assert!(
            !body.contains("sk-plaintext-secret"),
            "{key} holds the plaintext"
        );
    }
    let master = raw(&r, "ml/master_key").expect("a master key blob");
    let doc = keys
        .iter()
        .filter_map(|k| raw(&r, k))
        .find(|b| b.contains("compat") || b.contains("\"api_key\""))
        .expect("the connector document");
    let sealed = serde_json::from_str::<Value>(&doc).unwrap()["credential"]["api_key"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        oss_ml::credentials::open(&master, &sealed).unwrap(),
        "sk-plaintext-secret"
    );

    // A new credential replaces the sealed one, and is sealed too.
    call(
        &r,
        "PUT",
        &format!("{BASE}/{id}"),
        Some(&json!({"credential": {"api_key": "sk-second"}})),
    );
    let again = all_keys(&r);
    for key in &again {
        assert!(
            !raw(&r, key).unwrap_or_default().contains("sk-second"),
            "{key}"
        );
    }
    for reply in [
        call(&r, "GET", &format!("{BASE}/{id}"), None),
        call(&r, "POST", &format!("{BASE}/_search"), Some(&json!({}))),
    ] {
        assert!(!reply.text().contains("credential"), "{}", reply.text());
    }
}

#[test]
fn two_nodes_over_one_store_share_one_master_key() {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let a = rig_over(Arc::clone(&store), TRUSTED);
    let b = rig_over(Arc::clone(&store), TRUSTED);
    create(&a, &def("one"));
    let key = raw(&a, "ml/master_key").unwrap();
    create(&b, &def("two"));
    assert_eq!(raw(&b, "ml/master_key").unwrap(), key);
}

// AC-3
#[test]
fn a_definition_stock_refuses_is_refused_with_stocks_text() {
    let r = rig();
    let refused = |body: Value, reason: &str| {
        let reply = create(&r, &body);
        assert_eq!(reply.status, 400, "{}", reply.text());
        let e = json_of(&reply);
        assert_eq!(e["error"]["type"], "illegal_argument_exception");
        assert_eq!(e["error"]["reason"], reason, "{e}");
    };
    let mut untrusted = def("c2");
    untrusted["actions"][0]["url"] = json!("http://203.0.113.9/x");
    refused(
        untrusted,
        "Connector URL is not matching the trusted connector endpoint regex",
    );
    let mut protocol = def("c4");
    protocol["protocol"] = json!("carrier");
    refused(
        protocol,
        "Unsupported connector protocol. Please use one of \
         [aws_sigv4, http, google_cloud, mcp_sse, mcp_streamable_http]",
    );
    let mut nameless = def("x");
    nameless.as_object_mut().unwrap().remove("name");
    refused(nameless, "Connector name is null");
    let mut access = def("c5");
    access["backend_roles"] = json!(["admin"]);
    assert_eq!(create(&r, &access).status, 400);
    let mut bad_secret = def("c6");
    bad_secret["credential"] = json!({"k": 1});
    assert_eq!(create(&r, &bad_secret).status, 400);
    assert_eq!(
        call(&r, "POST", &format!("{BASE}/_create"), None).status,
        400
    );

    let mut none = def("c3");
    none["actions"] = json!([]);
    assert_eq!(create(&r, &none).status, 200);
    // An update that would leave an untrusted URL is refused and changes nothing.
    let id = id_of(&create(&r, &def("c7")));
    let put = call(
        &r,
        "PUT",
        &format!("{BASE}/{id}"),
        Some(&json!({"actions": [{"action_type": "predict", "url": "http://203.0.113.9/x"}]})),
    );
    assert_eq!(put.status, 400, "{}", put.text());
    let read = json_of(&call(&r, "GET", &format!("{BASE}/{id}"), None));
    assert_eq!(read["actions"][0]["url"], "http://127.0.0.1:18080/json");
}

#[test]
fn with_no_trusted_endpoint_set_no_connector_with_an_action_is_accepted() {
    let r = rig_over(Arc::new(Emulator::new(1 << 28, 5)), "");
    let refused = create(&r, &def("c"));
    assert_eq!(refused.status, 400, "{}", refused.text());
    let mut none = def("c");
    none["actions"] = json!([]);
    assert_eq!(create(&r, &none).status, 200);
}

#[test]
fn the_same_name_may_be_used_twice_as_stock_allows() {
    let r = rig();
    let (a, b) = (
        id_of(&create(&r, &def("same"))),
        id_of(&create(&r, &def("same"))),
    );
    assert_ne!(a, b);
    let hits = json_of(&call(
        &r,
        "POST",
        &format!("{BASE}/_search"),
        Some(&json!({"query": {"term": {"name.keyword": "same"}}})),
    ));
    assert_eq!(hits["hits"]["total"]["value"], 2);
}

// AC-4
#[test]
fn a_connector_put_costs_one_get_and_two_puts_a_read_two_gets_and_nothing_lists() {
    let r = rig();
    create(&r, &def("warm-up"));
    let (g, p, l) = (
        total(&r, S3Op::Get),
        total(&r, S3Op::Put),
        total(&r, S3Op::List),
    );
    let id = id_of(&create(&r, &def("second")));
    assert_eq!(total(&r, S3Op::Get) - g, 1, "a put reads the marker once");
    assert_eq!(
        total(&r, S3Op::Put) - p,
        2,
        "a put writes the document and the marker"
    );
    let g = total(&r, S3Op::Get);
    call(&r, "GET", &format!("{BASE}/{id}"), None);
    assert_eq!(total(&r, S3Op::Get) - g, 2);
    let g = total(&r, S3Op::Get);
    call(
        &r,
        "POST",
        &format!("{BASE}/_search"),
        Some(&json!({"size": 10})),
    );
    assert_eq!(
        total(&r, S3Op::Get) - g,
        1 + 2 * 2,
        "the marker, then 2 GETs a hit"
    );
    assert_eq!(total(&r, S3Op::List), l);
    assert_eq!(total(&r, S3Op::List), 0);
}

#[test]
fn the_master_key_is_read_once_a_node_and_a_read_never_needs_it() {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let a = rig_over(Arc::clone(&store), TRUSTED);
    let id = id_of(&create(&a, &def("c")));
    let b = rig_over(Arc::clone(&store), TRUSTED);
    let (g, p) = (total(&b, S3Op::Get), total(&b, S3Op::Put));
    call(&b, "GET", &format!("{BASE}/{id}"), None);
    call(
        &b,
        "PUT",
        &format!("{BASE}/{id}"),
        Some(&json!({"description": "x"})),
    );
    assert_eq!(
        total(&b, S3Op::Get) - g,
        2 + 3,
        "a get and an update: no key read"
    );
    assert_eq!(total(&b, S3Op::Put) - p, 2);
    let g = total(&b, S3Op::Get);
    call(
        &b,
        "PUT",
        &format!("{BASE}/{id}"),
        Some(&json!({"credential": {"k": "v"}})),
    );
    assert_eq!(
        total(&b, S3Op::Get) - g,
        3 + 1,
        "the update, plus the key once"
    );
    let g = total(&b, S3Op::Get);
    call(
        &b,
        "PUT",
        &format!("{BASE}/{id}"),
        Some(&json!({"credential": {"k": "w"}})),
    );
    assert_eq!(total(&b, S3Op::Get) - g, 3, "the key is remembered");
}
