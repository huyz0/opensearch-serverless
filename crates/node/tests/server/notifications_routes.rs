//! docs/specs/notifications slice 1: configs, channels and features.

use crate::support;

use std::net::TcpListener;
use std::sync::Arc;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_mem::MemoryBudget;
use oss_node::notifications_route::{NotificationsService, routes};
use oss_rest::{Router, Server};
use serde_json::{Value, json};

const BUF: usize = 1 << 16;

struct Rig {
    server: Server,
    store: Arc<Emulator>,
}

fn rig_over(store: Arc<Emulator>, base: &str) -> Rig {
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let service = Arc::new(NotificationsService::new(&blobs, base, BUF));
    let router = routes(Router::new(), &service).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    let server =
        Server::start(listener, support::server_config(), router, &budget).expect("starts");
    Rig { server, store }
}

fn rig() -> Rig {
    rig_over(Arc::new(Emulator::new(1 << 28, 5)), "")
}

fn call(r: &Rig, method: &str, target: &str, body: Option<Value>) -> (u16, Value) {
    let bytes = body.map(|b| b.to_string().into_bytes());
    let reply = support::request(r.server.local_addr(), method, target, bytes.as_deref());
    let parsed = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
    (reply.status, parsed)
}

const ROOT: &str = "/_plugins/_notifications";

fn slack(name: &str) -> Value {
    json!({"config": {"name": name, "description": "d", "config_type": "slack",
        "is_enabled": true, "slack": {"url": "https://hooks.slack.com/services/T/B/x"}}})
}

fn create(r: &Rig, name: &str) -> String {
    let (status, body) = call(r, "POST", &format!("{ROOT}/configs"), Some(slack(name)));
    assert_eq!(status, 200, "{body}");
    body["config_id"].as_str().expect("an id").to_owned()
}

fn requests(r: &Rig) -> [u64; 4] {
    let m = r.store.meter();
    [S3Op::Get, S3Op::Put, S3Op::Delete, S3Op::List].map(|op| m.count(op))
}

fn spent(r: &Rig, work: impl FnOnce()) -> [u64; 4] {
    let before = requests(r);
    work();
    let after = requests(r);
    [0, 1, 2, 3].map(|i| after[i].saturating_sub(before[i]))
}

// AC-1
#[test]
fn a_config_is_created_read_replaced_and_deleted() {
    let r = rig();
    let id = create(&r, "ops");
    let (status, read) = call(&r, "GET", &format!("{ROOT}/configs/{id}"), None);
    assert_eq!(status, 200, "{read}");
    assert_eq!(read["total_hits"], 1);
    assert_eq!(read["total_hit_relation"], "eq");
    assert_eq!(read["start_index"], 0);
    let item = &read["config_list"][0];
    assert_eq!(item["config_id"], id.as_str());
    assert_eq!(item["config"]["name"], "ops");
    assert_eq!(
        item["config"]["slack"]["url"],
        "https://hooks.slack.com/services/T/B/x"
    );
    assert!(item["created_time_ms"].as_u64().unwrap() > 0);

    let (status, put) = call(
        &r,
        "PUT",
        &format!("{ROOT}/configs/{id}"),
        Some(slack("ops2")),
    );
    assert_eq!(status, 200, "{put}");
    assert_eq!(put["config_id"], id.as_str());
    let (_, read) = call(&r, "GET", &format!("{ROOT}/configs/{id}"), None);
    assert_eq!(read["config_list"][0]["config"]["name"], "ops2");
    assert_eq!(
        read["config_list"][0]["created_time_ms"],
        item["created_time_ms"]
    );

    let (status, gone) = call(&r, "DELETE", &format!("{ROOT}/configs/{id}"), None);
    assert_eq!(status, 200, "{gone}");
    assert_eq!(gone["delete_response_list"][id.as_str()], "OK");
    let (status, _) = call(&r, "GET", &format!("{ROOT}/configs/{id}"), None);
    assert_eq!(status, 404);
    let (status, _) = call(&r, "DELETE", &format!("{ROOT}/configs/{id}"), None);
    assert_eq!(status, 404);
    let (status, _) = call(&r, "PUT", &format!("{ROOT}/configs/{id}"), Some(slack("x")));
    assert_eq!(status, 404, "a replace of a missing config");
}

#[test]
fn an_explicit_id_is_taken_once_and_a_bad_config_is_a_400_naming_it() {
    let r = rig();
    let mut body = slack("a");
    body["config_id"] = json!("mine");
    let (status, _) = call(&r, "POST", &format!("{ROOT}/configs"), Some(body.clone()));
    assert_eq!(status, 200);
    let (status, _) = call(&r, "POST", &format!("{ROOT}/configs"), Some(body));
    assert_eq!(status, 409);
    let bad = |config: Value, needle: &str| {
        let (status, body) = call(
            &r,
            "POST",
            &format!("{ROOT}/configs"),
            Some(json!({"config": config})),
        );
        assert_eq!(status, 400, "{config}: {body}");
        let text = body.to_string();
        assert!(text.contains(needle), "{needle} not in {text}");
    };
    let ok = slack("n")["config"].clone();
    let with = |k: &str, v: Value| {
        let mut c = ok.clone();
        c[k] = v;
        c
    };
    let (status, body) = call(
        &r,
        "POST",
        &format!("{ROOT}/configs"),
        Some(json!({"config": with("config_type", json!("pager"))})),
    );
    assert_eq!(status, 406, "an unknown type is not acceptable: {body}");
    bad(with("name", json!("")), "name");
    // The plugin takes these: a long name, a key it does not know.
    for extra in [
        with("name", json!("x".repeat(257))),
        with("surprise", json!(1)),
        with(
            "slack",
            json!({"url": "https://hooks.slack.com/services/x", "extra": 1}),
        ),
    ] {
        let (status, body) = call(
            &r,
            "POST",
            &format!("{ROOT}/configs"),
            Some(json!({"config": extra})),
        );
        assert_eq!(status, 200, "{extra}: {body}");
    }
    bad(with("slack", json!({"url": "ftp://x"})), "url");
    bad(with("slack", json!({"url": "https://u:p@host/x"})), "url");
    let mut no_block = ok.clone();
    no_block.as_object_mut().unwrap().remove("slack");
    bad(no_block, "slack");
    let hook = |h: Value| json!({"name": "w", "config_type": "webhook", "webhook": {"url": "https://h/x", "header_params": h}});
    bad(hook(json!({"Host": "evil"})), "Host");
    bad(hook(json!({"X-A": "a\r\nB: c"})), "X-A");
    bad(hook(json!({"bad name": "v"})), "bad name");
    bad(
        json!({"name": "s", "config_type": "ses_account", "ses_account": {"region": "us-east-1.evil.com", "from_address": "a@b.c"}}),
        "region",
    );
    let (status, _) = call(
        &r,
        "POST",
        &format!("{ROOT}/configs"),
        Some(json!({"config": {
        "name": "w", "config_type": "webhook",
        "webhook": {"url": "https://h/x", "header_params": {"X-Token": "t"}, "method": "PUT"}}})),
    );
    assert_eq!(status, 200);
}

// AC-2
#[test]
fn features_and_channels_have_the_plugins_shapes() {
    let r = rig();
    let (status, f) = call(&r, "GET", &format!("{ROOT}/features"), None);
    assert_eq!(status, 200);
    let types: Vec<&str> = f["allowed_config_type_list"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(types.len(), 9);
    assert!(types.contains(&"slack") && types.contains(&"smtp_account"));
    assert!(f["plugin_features"].is_object());

    create(&r, "chan");
    call(
        &r,
        "POST",
        &format!("{ROOT}/configs"),
        Some(json!({"config": {
        "name": "grp", "config_type": "email_group", "email_group": {"recipient_list": [{"recipient": "a@b.c"}]}}})),
    );
    let (status, c) = call(&r, "GET", &format!("{ROOT}/channels"), None);
    assert_eq!(status, 200, "{c}");
    assert_eq!(c["total_hits"], 1, "a group is not a channel: {c}");
    let ch = &c["channel_list"][0];
    assert_eq!(ch["name"], "chan");
    assert_eq!(ch["config_type"], "slack");
    assert_eq!(ch["is_enabled"], true);
    assert!(ch.get("slack").is_none() && !c.to_string().contains("hooks.slack.com"));
}

// AC-3
#[test]
fn request_counts_are_constant_in_the_number_of_configs() {
    let counts = |n: usize| {
        let r = rig();
        let ids: Vec<String> = (0..n).map(|i| create(&r, &format!("c{i:04}"))).collect();
        let id = ids[0].clone();
        let mut out = Vec::new();
        out.push(spent(&r, || {
            create(&r, "fresh");
        }));
        out.push(spent(&r, || {
            call(&r, "GET", &format!("{ROOT}/configs/{id}"), None);
        }));
        out.push(spent(&r, || {
            call(&r, "GET", &format!("{ROOT}/channels"), None);
        }));
        out.push(spent(&r, || {
            call(
                &r,
                "PUT",
                &format!("{ROOT}/configs/{id}"),
                Some(slack("again")),
            );
        }));
        out.push(spent(&r, || {
            call(&r, "DELETE", &format!("{ROOT}/configs/{id}"), None);
        }));
        out
    };
    let small = counts(5);
    let large = counts(60);
    assert_eq!(small, large, "requests must not scale with configs");
    for row in &small {
        assert_eq!(row[3], 0, "nothing lists");
    }
    assert_eq!(small[1], [2, 0, 0, 0], "a read is 2 GETs");
    assert_eq!(small[2], [3, 0, 0, 0], "channels is 3 GETs");
    assert_eq!((small[0][0], small[0][1]), (3, 4), "a create");
}

// AC-4
#[test]
fn a_missing_catalogue_entry_is_healed_by_a_list() {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let r = rig_over(Arc::clone(&store), "");
    let keep = create(&r, "keep");
    // a config written by a node that died before the catalogue swap
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let set = oss_meta::DocSet::new(blobs, "", oss_format::DocKind::NotificationConfigs).unwrap();
    let doc = json!({"config_id": "orphan", "seq": 0, "created_time_ms": 1, "last_updated_time_ms": 1,
        "config": slack("orphan")["config"]});
    set.put(
        "orphan",
        doc.to_string().as_bytes(),
        true,
        &mut vec![0; 1 << 16],
    )
    .unwrap();
    let (_, c) = call(&r, "GET", &format!("{ROOT}/channels"), None);
    assert_eq!(c["total_hits"], 2, "{c}");
    let after = spent(&r, || {
        let (_, c) = call(&r, "GET", &format!("{ROOT}/channels"), None);
        assert_eq!(c["total_hits"], 2);
    });
    assert_eq!(
        after,
        [3, 0, 0, 0],
        "a healed catalogue costs the plain list"
    );
    call(&r, "DELETE", &format!("{ROOT}/configs/{keep}"), None);
    let (_, c) = call(&r, "GET", &format!("{ROOT}/channels"), None);
    assert_eq!(c["total_hits"], 1);
}

// AC-5
#[test]
fn a_list_filters_sorts_and_pages() {
    let r = rig();
    for n in ["delta", "alpha", "charlie", "bravo"] {
        create(&r, n);
    }
    call(
        &r,
        "POST",
        &format!("{ROOT}/configs"),
        Some(json!({"config": {
        "name": "hook", "config_type": "chime", "is_enabled": false, "chime": {"url": "https://hooks.chime.aws/incomingwebhooks/x?token=t"}}})),
    );
    let names = |q: &str| -> (u64, Vec<String>) {
        let (s, b) = call(&r, "GET", &format!("{ROOT}/configs?{q}"), None);
        assert_eq!(s, 200, "{b}");
        (
            b["total_hits"].as_u64().unwrap(),
            b["config_list"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["config"]["name"].as_str().unwrap().to_owned())
                .collect(),
        )
    };
    assert_eq!(
        names("sort_field=name&sort_order=asc").1,
        ["alpha", "bravo", "charlie", "delta", "hook"]
    );
    assert_eq!(
        names("sort_field=name&sort_order=desc&max_items=2&from_index=1"),
        (5, vec!["delta".into(), "charlie".into()])
    );
    assert_eq!(names("config_type=chime").1, ["hook"]);
    assert_eq!(names("is_enabled=false").1, ["hook"]);
    assert_eq!(names("name=bravo").1, ["bravo"]);
    assert_eq!(names("text_query=ALPH").1, ["alpha"]);
    assert_eq!(names("max_items=100000&sort_field=name").1.len(), 5);
    let status = |q: &str| call(&r, "GET", &format!("{ROOT}/configs?{q}"), None).0;
    assert_eq!(status("sort_field=bogus"), 406);
    assert_eq!(status("sort_order=sideways"), 400);
    assert_eq!(status("max_items=0"), 400);
    assert_eq!(status("from_index=-1"), 400);
}

// AC-14
#[test]
fn two_bases_are_two_tenants() {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let a = rig_over(Arc::clone(&store), "tenant-a/");
    let b = rig_over(Arc::clone(&store), "tenant-b/");
    let id = create(&a, "mine");
    assert_eq!(
        call(&b, "GET", &format!("{ROOT}/configs/{id}"), None).0,
        404
    );
    let (_, list) = call(&b, "GET", &format!("{ROOT}/channels"), None);
    assert_eq!(list["total_hits"], 0);
}

// AC-10 (slice 1 part): the catalogue carries no secret
#[test]
fn the_catalogue_blob_holds_no_url_and_no_header() {
    let r = rig();
    call(
        &r,
        "POST",
        &format!("{ROOT}/configs"),
        Some(json!({"config": {
        "name": "w", "config_type": "webhook",
        "webhook": {"url": "https://secret.example/hook", "header_params": {"Authorization": "SECRETVALUE"}}}})),
    );
    let blobs = Arc::clone(&r.store) as Arc<dyn BlobStore>;
    let set = oss_meta::DocSet::new(blobs, "", oss_format::DocKind::NotificationCatalogue).unwrap();
    let mut body = vec![0u8; 2 << 20];
    let n = set
        .get("catalogue", &mut vec![0; 1 << 16], &mut body)
        .unwrap()
        .expect("catalogue");
    let text = String::from_utf8(body[..n].to_vec()).unwrap();
    assert!(text.contains("\"w\""));
    assert!(
        !text.contains("secret.example") && !text.contains("SECRETVALUE"),
        "{text}"
    );
}
