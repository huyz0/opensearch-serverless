//! docs/specs/reports slice 1: report definitions.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_mem::MemoryBudget;
use oss_node::reports_route::{ReportsService, routes};
use oss_rest::{Router, Server};
use serde_json::{Value, json};

struct Rig {
    server: Server,
    store: Arc<Emulator>,
}

fn rig() -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let service = Arc::new(ReportsService::new(&blobs, "", 1 << 16));
    let router = routes(Router::new(), &service).expect("routes");
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

const DEF: &str = "/_plugins/_reports/definition";

fn csv(name: &str) -> Value {
    json!({
        "report_params": {
            "report_name": name,
            "report_source": "Saved search",
            "description": "",
            "core_params": {
                "base_url": "/app/discover#/view/abc",
                "report_format": "csv",
                "time_duration": "PT30M",
                "origin": "https://dash.example"
            }
        },
        "delivery": {"configIds": [], "title": "", "textDescription": "", "htmlDescription": ""},
        "trigger": {"trigger_type": "On demand"}
    })
}

fn with(mut v: Value, path: &[&str], to: Value) -> Value {
    let (last, parents) = path.split_last().expect("a path");
    let mut at = &mut v;
    for key in parents {
        at = &mut at[*key];
    }
    at[*last] = to;
    v
}

// AC-1
#[test]
fn a_definition_is_created_read_replaced_listed_and_deleted() {
    let r = rig();
    let made = call(&r, "POST", DEF, Some(&csv("weekly")));
    assert_eq!(made.status, 200, "{}", made.text());
    let id = json_of(&made)["reportDefinitionId"]
        .as_str()
        .expect("id")
        .to_owned();

    let read = json_of(&call(&r, "GET", &format!("{DEF}/{id}"), None));
    let d = &read["reportDefinitionDetails"];
    assert_eq!(d["id"], id);
    assert_eq!(
        d["reportDefinition"]["report_params"]["report_name"],
        "weekly"
    );
    assert_eq!(d["createdTimeMs"], d["lastUpdatedTimeMs"]);

    std::thread::sleep(std::time::Duration::from_millis(3));
    let put = call(&r, "PUT", &format!("{DEF}/{id}"), Some(&csv("renamed")));
    assert_eq!(put.status, 200, "{}", put.text());
    let read = json_of(&call(&r, "GET", &format!("{DEF}/{id}"), None));
    let d = &read["reportDefinitionDetails"];
    assert_eq!(
        d["reportDefinition"]["report_params"]["report_name"],
        "renamed"
    );
    assert_ne!(d["createdTimeMs"], d["lastUpdatedTimeMs"]);

    std::thread::sleep(std::time::Duration::from_millis(3));
    call(&r, "POST", DEF, Some(&csv("second")));
    let list = json_of(&call(&r, "GET", "/_plugins/_reports/definitions", None));
    assert_eq!(list["totalHits"], 2);
    let names: Vec<&str> = list["reportDefinitionDetailsList"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| {
            x["reportDefinition"]["report_params"]["report_name"]
                .as_str()
                .unwrap()
        })
        .collect();
    assert_eq!(names, ["second", "renamed"], "newest first");
    let page = json_of(&call(
        &r,
        "GET",
        "/_plugins/_reports/definitions?fromIndex=1&maxItems=1",
        None,
    ));
    assert_eq!(
        page["reportDefinitionDetailsList"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    assert_eq!(call(&r, "DELETE", &format!("{DEF}/{id}"), None).status, 200);
    assert_eq!(call(&r, "GET", &format!("{DEF}/{id}"), None).status, 404);
    assert_eq!(call(&r, "DELETE", &format!("{DEF}/{id}"), None).status, 404);
    assert_eq!(
        call(&r, "PUT", &format!("{DEF}/{id}"), Some(&csv("x"))).status,
        404
    );
}

// AC-1
#[test]
fn a_definition_that_does_not_match_the_stock_shape_is_a_400() {
    let r = rig();
    let cron = |expr: &str| {
        with(
            csv("c"),
            &["trigger"],
            json!({"trigger_type": "Schedule", "trigger_params": {
                "schedule_type": "Cron based",
                "schedule": {"cron": {"expression": expr, "timezone": "UTC"}}}}),
        )
    };
    let every = |period: i64, unit: &str| {
        with(
            csv("i"),
            &["trigger"],
            json!({"trigger_type": "Schedule", "trigger_params": {
                "schedule_type": "Recurring",
                "schedule": {"interval": {"period": period, "unit": unit, "start_time": 0}}}}),
        )
    };
    let cases = [
        with(csv("a"), &["report_params", "report_source"], json!("Wiki")),
        with(
            csv("a"),
            &["report_params", "core_params", "report_format"],
            json!("doc"),
        ),
        with(
            csv("a"),
            &["report_params", "core_params", "base_url"],
            json!("https://evil.example/x"),
        ),
        with(csv("a"), &["report_params", "report_name"], json!("")),
        with(csv("a"), &["trigger", "trigger_type"], json!("Sometimes")),
        json!([1]),
        json!({}),
        cron("61 * * * *"),
        cron("not a cron"),
        every(4, "MINUTES"),
        every(1, "WEEKS"),
        every(0, "HOURS"),
    ];
    for body in cases {
        let reply = call(&r, "POST", DEF, Some(&body));
        assert_eq!(reply.status, 400, "{body}: {}", reply.text());
    }
    for ok in [cron("0 9 * * 1-5"), every(5, "MINUTES"), every(1, "DAYS")] {
        let reply = call(&r, "POST", DEF, Some(&ok));
        assert_eq!(reply.status, 200, "{ok}: {}", reply.text());
    }
    let none = support::request(r.server.local_addr(), "POST", DEF, None);
    assert_eq!(none.status, 400);
}

// AC-6
#[test]
fn a_pdf_with_no_renderer_and_a_delivery_with_no_notifications_are_501() {
    let r = rig();
    for format in ["pdf", "png"] {
        let body = with(
            with(
                csv("p"),
                &["report_params", "core_params", "report_format"],
                json!(format),
            ),
            &["report_params", "report_source"],
            json!("Dashboard"),
        );
        let reply = call(&r, "POST", DEF, Some(&body));
        assert_eq!(reply.status, 501, "{}", reply.text());
        assert_eq!(json_of(&reply)["error"]["type"], "renderer_not_configured");
    }
    let body = with(csv("d"), &["delivery", "configIds"], json!(["chan"]));
    let reply = call(&r, "POST", DEF, Some(&body));
    assert_eq!(reply.status, 501, "{}", reply.text());
    assert_eq!(json_of(&reply)["error"]["type"], "delivery_unsupported");
    // a malformed cron on a PDF is still the 400
    let both = with(
        with(
            csv("p"),
            &["report_params", "core_params", "report_format"],
            json!("pdf"),
        ),
        &["trigger"],
        json!({"trigger_type": "Schedule", "trigger_params": {"schedule_type": "Cron based",
            "schedule": {"cron": {"expression": "x", "timezone": "UTC"}}}}),
    );
    assert_eq!(call(&r, "POST", DEF, Some(&both)).status, 400);
    assert_eq!(
        r.store.meter().count(S3Op::Put),
        0,
        "a refusal writes nothing"
    );
}

// AC-8
#[test]
fn a_deployment_holds_at_most_500_definitions() {
    let r = rig();
    for i in 0..500 {
        let reply = call(&r, "POST", DEF, Some(&csv(&format!("d{i}"))));
        assert_eq!(reply.status, 200, "{i}: {}", reply.text());
    }
    let over = call(&r, "POST", DEF, Some(&csv("one too many")));
    assert_eq!(over.status, 429, "{}", over.text());
    assert_eq!(json_of(&over)["error"]["type"], "report_limit");
    let list = json_of(&call(
        &r,
        "GET",
        "/_plugins/_reports/definitions?maxItems=1000",
        None,
    ));
    assert_eq!(list["totalHits"], 500);
    assert_eq!(
        list["reportDefinitionDetailsList"]
            .as_array()
            .unwrap()
            .len(),
        100
    );
}

// AC-10
#[test]
fn a_create_costs_one_get_and_two_puts_a_read_two_gets_and_nothing_lists() {
    let r = rig();
    let m = &r.store;
    let (gets, puts) = (m.meter().count(S3Op::Get), m.meter().count(S3Op::Put));
    let made = call(&r, "POST", DEF, Some(&csv("m")));
    assert_eq!(
        (
            m.meter().count(S3Op::Get) - gets,
            m.meter().count(S3Op::Put) - puts
        ),
        (1, 2),
        "a create"
    );
    let id = json_of(&made)["reportDefinitionId"]
        .as_str()
        .unwrap()
        .to_owned();
    let gets = m.meter().count(S3Op::Get);
    call(&r, "GET", &format!("{DEF}/{id}"), None);
    assert_eq!(m.meter().count(S3Op::Get) - gets, 2, "a read");
    assert_eq!(m.meter().count(S3Op::List), 0);
}
