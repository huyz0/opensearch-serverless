//! docs/specs/ml-commons slice 2b: a remote model's predict through its connector, and the
//! outbound client's refusals and bounds.

use crate::support;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use oss_blob::{BlobStore, Emulator, S3Op};
use oss_mem::MemoryBudget;
use oss_ml::connector::Trusted;
use oss_node::ml_route::{Limits, MlService, Outbound, Policy, routes};
use oss_node::notifications_route::{EgressPolicy, system_lookup};
use oss_rest::{Router, Server};
use serde_json::{Value, json};

const BUF: usize = 1 << 16;

/// What the fixture server saw of one request.
#[derive(Clone, Debug)]
struct Seen {
    request_line: String,
    headers: Vec<(String, String)>,
    body: String,
}

type Answer = Box<dyn Fn(&Seen) -> (u16, String) + Send + Sync>;

struct Fixture {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Fixture {
    fn start(answer: Answer) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let answer = Arc::new(answer);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (log, answer) = (Arc::clone(&log), Arc::clone(&answer));
                std::thread::spawn(move || serve(stream, &log, &*answer));
            }
        });
        Self { port, seen }
    }

    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

fn serve(
    mut stream: TcpStream,
    log: &Mutex<Vec<Seen>>,
    answer: &(dyn Fn(&Seen) -> (u16, String) + Send + Sync),
) {
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            return;
        }
        raw.extend_from_slice(&chunk[..n]);
        if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break i.saturating_add(4);
        }
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default().to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_owned()))
        .collect();
    let len: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    while raw.len() < head_end.saturating_add(len) {
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..n]);
    }
    let body = String::from_utf8_lossy(&raw[head_end..]).into_owned();
    let seen = Seen {
        request_line,
        headers,
        body,
    };
    log.lock().unwrap().push(seen.clone());
    let (status, text) = answer(&seen);
    let location = if (300..400).contains(&status) {
        "Location: http://127.0.0.1:1/x\r\n"
    } else {
        ""
    };
    let reply = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n{location}Content-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
    let _ = stream.write_all(reply.as_bytes());
}

struct Rig {
    server: Server,
    store: Arc<Emulator>,
}

fn rig(port: u16, limits: Limits, ttl: Duration, private: bool) -> Rig {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let policy = Policy {
        egress: EgressPolicy {
            allow_private: private,
            ports: vec![port],
            deny_hosts: Vec::new(),
        },
        allow_http: private,
    };
    let service = MlService::new(&blobs, "", BUF)
        .with_trusted(Trusted::parse(r"^http://127\.0\.0\.1:\d+/.*$").unwrap())
        .with_outbound(Outbound::new(system_lookup(), policy, limits))
        .with_cache_ttl(ttl);
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
        &format!("/_plugins/_ml/{path}"),
        bytes.as_deref().map(str::as_bytes),
    )
}

fn ok(reply: &support::Reply) -> Value {
    assert_eq!(reply.status, 200, "{}", reply.text());
    serde_json::from_str(&reply.text()).unwrap()
}

fn total(r: &Rig, op: S3Op) -> u64 {
    r.store.meter().count(op)
}

/// A remote model over a connector that calls the fixture; returns the model id.
fn model(r: &Rig, port: u16) -> String {
    let connector = json!({"name": "c", "description": "d", "version": "1", "protocol": "http",
        "parameters": {"endpoint": format!("127.0.0.1:{port}")},
        "credential": {"api_key": "sk-plaintext-secret"},
        "actions": [{"action_type": "predict", "method": "POST",
            "url": "http://${parameters.endpoint}/json",
            "headers": {"x": "y", "authorization": "Bearer ${credential.api_key}"},
            "request_body": "{\"input\":\"${parameters.input}\"}"}]});
    let c = ok(&call(r, "POST", "connectors/_create", Some(&connector)))["connector_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let made = ok(&call(
        r,
        "POST",
        "models/_register",
        Some(&json!({"name": "m", "function_name": "remote", "connector_id": c})),
    ));
    made["model_id"].as_str().unwrap().to_owned()
}

fn echo() -> Answer {
    Box::new(|_| (200, r#"{"out":[1,2,3],"echo":"ok"}"#.to_owned()))
}

fn predict(r: &Rig, m: &str, params: Value) -> support::Reply {
    call(
        r,
        "POST",
        &format!("models/{m}/_predict"),
        Some(&json!({"parameters": params})),
    )
}

fn refused(reply: &support::Reply, status: u16, reason: &str) {
    assert_eq!(reply.status, status, "{}", reply.text());
    let e: Value = serde_json::from_str(&reply.text()).unwrap();
    assert_eq!(e["error"]["reason"], reason, "{e}");
}

// AC-5, AC-6
#[test]
fn a_predict_makes_the_connectors_request_and_shows_the_answer_in_stocks_shape() {
    let fx = Fixture::start(echo());
    let r = rig(fx.port, Limits::default(), Duration::from_secs(60), true);
    let m = model(&r, fx.port);
    // Never deployed: stock predicts all the same.
    let out = ok(&predict(&r, &m, json!({"input": "hi"})));
    assert_eq!(
        out,
        json!({"inference_results": [{"output": [{"name": "response",
            "dataAsMap": {"out": [1, 2, 3], "echo": "ok"}}], "status_code": 200}]})
    );
    let seen = fx.requests();
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].request_line.starts_with("POST /json "),
        "{:?}",
        seen[0]
    );
    assert_eq!(seen[0].body, r#"{"input":"hi"}"#);
    let header = |k: &str| {
        seen[0]
            .headers
            .iter()
            .find(|(h, _)| h == k)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(header("x").as_deref(), Some("y"));
    assert_eq!(
        header("authorization").as_deref(),
        Some("Bearer sk-plaintext-secret")
    );

    let again = ok(&call(
        &r,
        "POST",
        &format!("_predict/remote/{m}"),
        Some(&json!({"parameters": {"input": "yo"}})),
    ));
    assert_eq!(again, out);
    assert_eq!(fx.requests()[1].body, r#"{"input":"yo"}"#);
    assert_eq!(
        call(
            &r,
            "POST",
            &format!("_predict/kmeans/{m}"),
            Some(&json!({}))
        )
        .status,
        501
    );
    ok(&call(&r, "POST", &format!("models/{m}/_deploy"), None));
    ok(&predict(&r, &m, json!({"input": "after deploy"})));
}

#[test]
fn a_predict_missing_its_parameter_or_naming_a_wrong_model_is_stocks_400_or_404() {
    let fx = Fixture::start(echo());
    let r = rig(fx.port, Limits::default(), Duration::from_secs(60), true);
    let m = model(&r, fx.port);
    let want = "Some parameter placeholder not filled in payload: input";
    refused(
        &call(
            &r,
            "POST",
            &format!("models/{m}/_predict"),
            Some(&json!({})),
        ),
        400,
        want,
    );
    refused(&predict(&r, &m, json!({"nope": "x"})), 400, want);
    assert_eq!(
        call(&r, "POST", "models/gone/_predict", Some(&json!({}))).status,
        404
    );
    assert_eq!(
        call(
            &r,
            "POST",
            &format!("models/{m}/_predict"),
            Some(&json!({"parameters": 3}))
        )
        .status,
        400
    );
    assert!(
        fx.requests().is_empty(),
        "no request left for a refused call"
    );
}

// AC-9
#[test]
fn a_warm_predict_makes_no_request_and_a_changed_model_or_connector_is_seen() {
    let fx = Fixture::start(echo());
    let r = rig(fx.port, Limits::default(), Duration::from_secs(3600), true);
    let m = model(&r, fx.port);
    let (g, p) = (total(&r, S3Op::Get), total(&r, S3Op::Put));
    ok(&predict(&r, &m, json!({"input": "a"})));
    assert_eq!(
        total(&r, S3Op::Get) - g,
        4,
        "the model 2 and its connector 2"
    );
    let g = total(&r, S3Op::Get);
    for _ in 0..5 {
        ok(&predict(&r, &m, json!({"input": "b"})));
    }
    assert_eq!(
        (total(&r, S3Op::Get) - g, total(&r, S3Op::Put) - p),
        (0, 0),
        "warm predicts are free"
    );
    assert_eq!(total(&r, S3Op::List), 0);

    // An update on this node is seen at once.
    let model_doc = ok(&call(&r, "GET", &format!("models/{m}"), None));
    let cid = model_doc["connector_id"].as_str().unwrap().to_owned();
    ok(&call(
        &r,
        "PUT",
        &format!("connectors/{cid}"),
        Some(&json!({"parameters": {"endpoint": "127.0.0.1:1"}})),
    ));
    let g = total(&r, S3Op::Get);
    let moved = predict(&r, &m, json!({"input": "c"}));
    assert_ne!(
        moved.status, 200,
        "the changed connector is used, not the cached one"
    );
    assert_eq!(total(&r, S3Op::Get) - g, 4);
}

#[test]
fn with_no_ttl_every_predict_reads_again() {
    let fx = Fixture::start(echo());
    let r = rig(fx.port, Limits::default(), Duration::ZERO, true);
    let m = model(&r, fx.port);
    ok(&predict(&r, &m, json!({"input": "a"})));
    let g = total(&r, S3Op::Get);
    ok(&predict(&r, &m, json!({"input": "a"})));
    assert_eq!(total(&r, S3Op::Get) - g, 4);
}

// AC-19
#[test]
fn the_client_refuses_plain_http_private_hosts_and_redirects_without_following() {
    let fx = Fixture::start(Box::new(|_| (302, String::new())));
    // Private addresses not allowed: the loopback fixture is never connected to.
    let public = rig(fx.port, Limits::default(), Duration::from_secs(60), false);
    let m = model(&public, fx.port);
    let reply = predict(&public, &m, json!({"input": "a"}));
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert!(reply.text().contains("must use https"), "{}", reply.text());
    assert!(fx.requests().is_empty());

    // Private allowed (http too): a redirect is an answer, never followed.
    let open = rig(fx.port, Limits::default(), Duration::from_secs(60), true);
    let m = model(&open, fx.port);
    refused(
        &predict(&open, &m, json!({"input": "a"})),
        502,
        "the remote service answered with a redirect",
    );
    assert_eq!(fx.requests().len(), 1, "the Location was not followed");
}

#[test]
fn a_host_the_policy_refuses_is_never_connected_to_and_the_text_names_no_address() {
    let fx = Fixture::start(echo());
    // https allowed by the client's own rule, but loopback is not a public address.
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = Arc::clone(&store) as Arc<dyn BlobStore>;
    let policy = Policy {
        egress: EgressPolicy {
            allow_private: false,
            ports: vec![fx.port],
            deny_hosts: Vec::new(),
        },
        allow_http: true,
    };
    let service = MlService::new(&blobs, "", BUF)
        .with_trusted(Trusted::parse(r"^http://127\.0\.0\.1:\d+/.*$").unwrap())
        .with_outbound(Outbound::new(system_lookup(), policy, Limits::default()));
    let router = routes(Router::new(), &Arc::new(service)).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let budget = MemoryBudget::new(&support::budget_config());
    let server = Server::start(listener, support::server_config(), router, &budget).unwrap();
    let r = Rig { server, store };
    let m = model(&r, fx.port);
    let reply = predict(&r, &m, json!({"input": "a"}));
    refused(&reply, 400, "the connector's host is not allowed");
    assert!(fx.requests().is_empty());
}

// AC-6, AC-7
#[test]
fn a_remote_error_a_slow_remote_a_big_response_and_a_full_queue_are_each_bounded() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let fx = Fixture::start(Box::new(move |s| {
        counter.fetch_add(1, Ordering::SeqCst);
        match s.body.as_str() {
            r#"{"input":"boom"}"# => (500, "boom".to_owned()),
            r#"{"input":"big"}"# => (200, format!("{{\"v\":\"{}\"}}", "x".repeat(4096))),
            r#"{"input":"slow"}"# => {
                std::thread::sleep(Duration::from_millis(1500));
                (200, r#"{"ok":1}"#.to_owned())
            }
            _ => (200, r#"{"ok":1}"#.to_owned()),
        }
    }));
    let limits = Limits {
        running: 1,
        waiting: 0,
        max_response: 1024,
        total: Duration::from_millis(600),
        connect: Duration::from_secs(2),
    };
    let r = Arc::new(rig(fx.port, limits, Duration::from_secs(60), true));
    let m = model(&r, fx.port);
    refused(
        &predict(&r, &m, json!({"input": "boom"})),
        500,
        "Error from remote service: boom",
    );
    refused(
        &predict(&r, &m, json!({"input": "big"})),
        502,
        "the remote service's response is too large or unreadable",
    );
    refused(
        &predict(&r, &m, json!({"input": "slow"})),
        408,
        "the remote service did not answer in time",
    );
    ok(&predict(&r, &m, json!({"input": "fine"})));

    // One call in flight and no queue: a second at the same time is 429.
    let (r2, m2) = (Arc::clone(&r), m.clone());
    let held = std::thread::spawn(move || predict(&r2, &m2, json!({"input": "slow"})).status);
    std::thread::sleep(Duration::from_millis(200));
    refused(
        &predict(&r, &m, json!({"input": "fine"})),
        429,
        "too many calls to remote services are in flight",
    );
    assert_eq!(held.join().unwrap(), 408);
}

// AC-2
#[test]
fn the_opened_credential_is_sent_but_never_stored_or_shown() {
    let fx = Fixture::start(echo());
    let r = rig(fx.port, Limits::default(), Duration::from_secs(60), true);
    let m = model(&r, fx.port);
    let reply = predict(&r, &m, json!({"input": "a"}));
    assert!(!reply.text().contains("sk-plaintext-secret"));
    let got = call(&r, "GET", &format!("models/{m}"), None).text();
    assert!(!got.contains("sk-plaintext-secret"));
    let mut keys = Vec::new();
    // LIST: a test inspecting what the store holds, after the requests under test.
    r.store
        .list_prefix("", None, 1000, &mut |k, _| keys.push(k.to_owned()))
        .unwrap();
    let mut buf = [0u8; 4096];
    for key in keys {
        let mut bytes = Vec::new();
        let mut sink = |c: &[u8]| {
            bytes.extend_from_slice(c);
            Ok(())
        };
        let _ = r
            .store
            .get(&key, oss_blob::ByteRange::Full, &mut buf, &mut sink);
        assert!(
            !String::from_utf8_lossy(&bytes).contains("sk-plaintext-secret"),
            "{key}"
        );
    }
}
