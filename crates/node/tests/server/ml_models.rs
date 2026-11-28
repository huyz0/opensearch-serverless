//! docs/specs/ml-commons slice 2: remote models, their tasks and deployment state.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_mem::MemoryBudget;
use oss_ml::connector::Trusted;
use oss_node::ml_route::{MlService, routes};
use oss_rest::{Router, Server};
use serde_json::{Value, json};

const BUF: usize = 1 << 16;
const ML: &str = "/_plugins/_ml";
const NODE: &str = "node-a";

struct Rig {
    server: Server,
    store: Arc<Emulator>,
}

fn rig() -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let trusted = Trusted::parse(r"^http://127\.0\.0\.1:18080/.*$").unwrap();
    let service = MlService::new(&blobs, "", BUF)
        .with_trusted(trusted)
        .with_node_id(NODE);
    let router = routes(Router::new(), &Arc::new(service)).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig { server, store }
}

fn call(r: &Rig, method: &str, path: &str, body: Option<&Value>) -> support::Reply {
    let bytes = body.map(Value::to_string);
    support::request(
        r.server.local_addr(),
        method,
        &format!("{ML}/{path}"),
        bytes.as_deref().map(str::as_bytes),
    )
}

fn ok(reply: &support::Reply) -> Value {
    assert_eq!(reply.status, 200, "{}", reply.text());
    serde_json::from_str(&reply.text()).unwrap()
}

fn text(v: &Value, key: &str) -> String {
    v[key]
        .as_str()
        .unwrap_or_else(|| panic!("{key} in {v}"))
        .to_owned()
}

fn total(r: &Rig, op: S3Op) -> u64 {
    r.store.meter().count(op)
}

fn connector() -> Value {
    json!({"name": "c", "description": "d", "version": "1", "protocol": "http",
           "parameters": {"endpoint": "127.0.0.1:18080"}, "credential": {"k": "v"},
           "actions": [{"action_type": "predict", "method": "POST",
                        "url": "http://127.0.0.1:18080/json",
                        "request_body": "{\"input\":\"${parameters.input}\"}"}]})
}

/// A group and a connector, as the recorded flow starts.
fn setup(r: &Rig) -> (String, String) {
    let g = ok(&call(
        r,
        "POST",
        "model_groups/_register",
        Some(&json!({"name": "g"})),
    ));
    let c = ok(&call(r, "POST", "connectors/_create", Some(&connector())));
    (text(&g, "model_group_id"), text(&c, "connector_id"))
}

fn register(r: &Rig, name: &str, group: &str, connector: &str) -> support::Reply {
    call(
        r,
        "POST",
        "models/_register",
        Some(
            &json!({"name": name, "function_name": "remote", "description": "d",
                     "connector_id": connector, "model_group_id": group}),
        ),
    )
}

fn refused(reply: &support::Reply, status: u16, reason: &str) {
    assert_eq!(reply.status, status, "{}", reply.text());
    let e: Value = serde_json::from_str(&reply.text()).unwrap();
    assert_eq!(e["error"]["reason"], reason, "{e}");
}

// AC-5
#[test]
fn a_remote_model_is_registered_deployed_updated_searched_undeployed_and_deleted() {
    let r = rig();
    let (g, c) = setup(&r);
    let made = ok(&register(&r, "compat-m1", &g, &c));
    assert_eq!(made["status"], "CREATED");
    let m = text(&made, "model_id");

    let task = ok(&call(
        &r,
        "GET",
        &format!("tasks/{}", text(&made, "task_id")),
        None,
    ));
    assert_eq!(task["task_type"], "REGISTER_MODEL");
    assert_eq!(
        (task["state"].as_str(), task["is_async"].as_bool()),
        (Some("COMPLETED"), Some(false))
    );
    assert_eq!(
        (task["model_id"].as_str(), task["worker_node"][0].as_str()),
        (Some(m.as_str()), Some(NODE))
    );

    let model = ok(&call(&r, "GET", &format!("models/{m}"), None));
    assert_eq!(model["algorithm"], "REMOTE");
    assert_eq!(model["model_state"], "REGISTERED");
    assert_eq!(model["model_version"], "1");
    assert_eq!(model["connector_id"], c.as_str());
    assert_eq!(model["model_group_id"], g.as_str());
    assert!(
        model.get("model_id").is_none() && model.get("last_deployed_time").is_none(),
        "{model}"
    );
    let group = ok(&call(&r, "GET", &format!("model_groups/{g}"), None));
    assert_eq!(group["latest_version"], 1);
    let second = ok(&register(&r, "compat-m1b", &g, &c));
    let m2 = text(&second, "model_id");
    assert_eq!(
        ok(&call(&r, "GET", &format!("models/{m2}"), None))["model_version"],
        "2"
    );

    let deploy = ok(&call(&r, "POST", &format!("models/{m}/_deploy"), None));
    assert_eq!(
        (deploy["task_type"].as_str(), deploy["status"].as_str()),
        (Some("DEPLOY_MODEL"), Some("COMPLETED"))
    );
    let dtask = ok(&call(
        &r,
        "GET",
        &format!("tasks/{}", text(&deploy, "task_id")),
        None,
    ));
    assert_eq!(
        (dtask["task_type"].as_str(), dtask["is_async"].as_bool()),
        (Some("DEPLOY_MODEL"), Some(true))
    );
    let model = ok(&call(&r, "GET", &format!("models/{m}"), None));
    assert_eq!(model["model_state"], "DEPLOYED");
    assert_eq!(model["deploy_to_all_nodes"], true);
    assert_eq!(model["planning_worker_nodes"][0], NODE);

    let put = ok(&call(
        &r,
        "PUT",
        &format!("models/{m}"),
        Some(&json!({"description": "changed"})),
    ));
    assert_eq!(
        (put["result"].as_str(), put["_index"].as_str()),
        (Some("updated"), Some(".plugins-ml-model"))
    );
    let hits = ok(&call(
        &r,
        "POST",
        "models/_search",
        Some(&json!({"query": {"match_all": {}}, "sort": [{"name.keyword": "asc"}]})),
    ));
    let first = &hits["hits"]["hits"][0];
    assert_eq!(first["_source"]["name"], "compat-m1");
    assert_eq!(first["_source"]["model_id"], m.as_str());
    assert_eq!(first["_source"]["description"], "changed");
    assert_eq!(first["sort"][0], "compat-m1");

    refused(
        &call(&r, "DELETE", &format!("models/{m}"), None),
        400,
        "Model cannot be deleted in deploying or deployed state. Try undeploy model first then delete",
    );
    let un = ok(&call(&r, "POST", &format!("models/{m}/_undeploy"), None));
    assert_eq!(un[NODE]["stats"][m.as_str()], "undeployed");
    let again = ok(&call(&r, "POST", &format!("models/{m}/_undeploy"), None));
    assert_eq!(again[NODE]["stats"][m.as_str()], "not_found");
    let model = ok(&call(&r, "GET", &format!("models/{m}"), None));
    assert!(model.get("deploy_to_all_nodes").is_none(), "{model}");
    let del = ok(&call(&r, "DELETE", &format!("models/{m}"), None));
    assert_eq!(del["result"], "deleted");
    refused(
        &call(&r, "DELETE", &format!("models/{m}"), None),
        404,
        "Failed to find model",
    );
    refused(
        &call(&r, "GET", "models/nope", None),
        404,
        "Failed to find model with the provided model id: nope",
    );
    assert_eq!(call(&r, "POST", "models/nope/_deploy", None).status, 404);
}

#[test]
fn a_connector_a_model_uses_is_not_deleted_until_the_model_is() {
    let r = rig();
    let (g, c) = setup(&r);
    let m = text(&ok(&register(&r, "m", &g, &c)), "model_id");
    refused(
        &call(&r, "DELETE", &format!("connectors/{c}"), None),
        409,
        &format!(
            "1 models are still using this connector, please delete or update the models first: [{m}]"
        ),
    );
    ok(&call(&r, "DELETE", &format!("models/{m}"), None));
    assert_eq!(
        ok(&call(&r, "DELETE", &format!("connectors/{c}"), None))["result"],
        "deleted"
    );
}

// AC-3
#[test]
fn a_register_stock_refuses_is_refused_with_stocks_text() {
    let r = rig();
    let (g, c) = setup(&r);
    let post = |body: Value| call(&r, "POST", "models/_register", Some(&body));
    refused(
        &post(json!({"name": "m", "function_name": "remote"})),
        400,
        "You must provide connector content when creating a remote model without connector id!",
    );
    refused(
        &post(json!({"name": "m", "function_name": "nope"})),
        400,
        "Wrong function name",
    );
    refused(
        &post(json!({"function_name": "remote"})),
        400,
        "model name is null",
    );
    refused(
        &post(
            json!({"name": "m", "function_name": "remote", "connector_id": c, "model_group_id": "nope"}),
        ),
        500,
        "Model Group Response is empty for nope",
    );
    refused(
        &post(json!({"name": "m", "function_name": "remote", "connector_id": "gone"})),
        404,
        "Failed to find connector with the provided connector id: gone",
    );
    assert_eq!(
        post(json!({"name": "m", "function_name": "TEXT_EMBEDDING", "model_format": "ONNX"}))
            .status,
        501
    );
    assert_eq!(
        post(json!({"name": "m", "function_name": "remote", "connector_id": c, "backend_roles": ["a"]})).status,
        400
    );
    // Nothing above registered a model or moved the group's version.
    assert_eq!(
        ok(&call(&r, "POST", "models/_search", Some(&json!({}))))["hits"]["total"]["value"],
        0
    );
    assert_eq!(
        ok(&call(&r, "GET", &format!("model_groups/{g}"), None))["latest_version"],
        0
    );
}

#[test]
fn an_inline_connector_is_stored_with_the_model_and_a_bad_one_registers_nothing() {
    let r = rig();
    let body = json!({"name": "m2", "function_name": "remote", "connector": connector()});
    let m = text(
        &ok(&call(&r, "POST", "models/_register", Some(&body))),
        "model_id",
    );
    let model = ok(&call(&r, "GET", &format!("models/{m}"), None));
    let cid = text(&model, "connector_id");
    assert_eq!(
        ok(&call(&r, "GET", &format!("connectors/{cid}"), None))["name"],
        "c"
    );
    assert!(model.get("model_group_id").is_none(), "{model}");
    assert_eq!(model["model_version"], "1");

    let mut bad = connector();
    bad["actions"][0]["url"] = json!("http://203.0.113.9/x");
    let reply = call(
        &r,
        "POST",
        "models/_register",
        Some(&json!({"name": "m3", "function_name": "remote", "connector": bad})),
    );
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert_eq!(
        ok(&call(&r, "POST", "models/_search", Some(&json!({}))))["hits"]["total"]["value"],
        1
    );
}

#[test]
fn tasks_are_listed_read_deleted_and_only_the_newest_hundred_and_twenty_eight_are_kept() {
    let r = rig();
    let (g, c) = setup(&r);
    assert_eq!(
        ok(&call(
            &r,
            "POST",
            "tasks/_search",
            Some(&json!({"size": 0}))
        ))["hits"]["total"]["value"],
        0
    );
    let mut ids = Vec::new();
    for i in 0..130 {
        let made = ok(&register(&r, &format!("m{i}"), &g, &c));
        ids.push(text(&made, "task_id"));
    }
    assert_eq!(
        call(&r, "GET", &format!("tasks/{}", ids[0]), None).status,
        404
    );
    assert_eq!(
        call(&r, "GET", &format!("tasks/{}", ids[1]), None).status,
        404
    );
    assert_eq!(
        call(&r, "GET", &format!("tasks/{}", ids[2]), None).status,
        200
    );
    let page = ok(&call(&r, "GET", "tasks/_search", None));
    assert_eq!(page["hits"]["total"]["value"], 128);
    assert_eq!(page["hits"]["hits"].as_array().unwrap().len(), 10);
    assert_eq!(page["hits"]["hits"][0]["_id"], ids[129].as_str());
    assert_eq!(
        call(&r, "POST", "tasks/_search", Some(&json!({"size": 500}))).status,
        400
    );
    let last = ids.last().unwrap();
    assert_eq!(
        ok(&call(&r, "DELETE", &format!("tasks/{last}"), None))["result"],
        "deleted"
    );
    assert_eq!(
        call(&r, "DELETE", &format!("tasks/{last}"), None).status,
        404
    );
    refused(
        &call(&r, "GET", "tasks/nope", None),
        404,
        "Failed to find task with the provided task id: nope",
    );
}

// AC-4
#[test]
fn a_model_register_and_a_read_cost_what_the_budget_holds_and_nothing_lists() {
    let r = rig();
    let (g, c) = setup(&r);
    ok(&register(&r, "warm", &g, &c));
    let (gets, puts) = (total(&r, S3Op::Get), total(&r, S3Op::Put));
    let m = text(&ok(&register(&r, "second", &g, &c)), "model_id");
    // The connector (2), the group (2), the group's put (1), the model's put (1).
    assert_eq!(total(&r, S3Op::Get) - gets, 6, "register reads");
    assert_eq!(total(&r, S3Op::Put) - puts, 4, "register writes");
    let gets = total(&r, S3Op::Get);
    ok(&call(&r, "GET", &format!("models/{m}"), None));
    assert_eq!(total(&r, S3Op::Get) - gets, 2);
    let (gets, puts) = (total(&r, S3Op::Get), total(&r, S3Op::Put));
    ok(&call(&r, "GET", "tasks/_search", None));
    assert_eq!(
        (total(&r, S3Op::Get) - gets, total(&r, S3Op::Put) - puts),
        (0, 0),
        "tasks are memory"
    );
    assert_eq!(total(&r, S3Op::List), 0);
}
