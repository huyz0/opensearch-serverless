//! docs/specs/notifications slice 2: the test route and the deliverer, to a
//! local sink, under the egress rules.

use crate::support;

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use oss_blob::{BlobStore, Emulator};
use oss_mem::MemoryBudget;
use oss_node::notifications_route::{
    Deliverer, EgressPolicy, Limits, NotificationsService, permitted, routes, system_lookup,
};
use oss_rest::{Router, Server};
use serde_json::{Value, json};

const ROOT: &str = "/_plugins/_notifications";

/// A target that records each request and answers with `status` and `extra`.
pub(crate) struct Sink {
    pub(crate) addr: SocketAddr,
    pub(crate) seen: Arc<Mutex<Vec<String>>>,
}

pub(crate) fn sink(
    status: &'static str,
    extra: &'static str,
    body: &'static str,
    delay: Duration,
) -> Sink {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let log = Arc::clone(&log);
            std::thread::spawn(move || {
                let mut raw = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = s.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&raw).to_string();
                    if let Some(split) = text.find("\r\n\r\n") {
                        let len = text
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if raw.len() >= split.saturating_add(4).saturating_add(len) {
                            break;
                        }
                    }
                }
                log.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&raw).to_string());
                std::thread::sleep(delay);
                let reply = format!(
                    "HTTP/1.1 {status}\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(reply.as_bytes());
            });
        }
    });
    Sink { addr, seen }
}

pub(crate) fn open_policy(port: u16) -> EgressPolicy {
    EgressPolicy {
        allow_private: true,
        ports: vec![port, 80, 443],
        deny_hosts: vec![],
    }
}

fn rig(deliverer: Option<Deliverer>) -> Server {
    let store = Arc::new(Emulator::new(1 << 28, 5));
    let blobs = store as Arc<dyn BlobStore>;
    let mut service = NotificationsService::new(&blobs, "", 1 << 16);
    if let Some(d) = deliverer {
        service = service.with_deliverer(Arc::new(d));
    }
    let router = routes(Router::new(), &Arc::new(service)).expect("routes");
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let budget = MemoryBudget::new(&support::budget_config());
    Server::start(listener, support::server_config(), router, &budget).expect("starts")
}

fn call(s: &Server, method: &str, target: &str, body: Option<Value>) -> (u16, Value) {
    let bytes = body.map(|b| b.to_string().into_bytes());
    let r = support::request(s.local_addr(), method, target, bytes.as_deref());
    (
        r.status,
        serde_json::from_slice(&r.body).unwrap_or(Value::Null),
    )
}

fn make(s: &Server, config: Value) -> String {
    let (status, body) = call(
        s,
        "POST",
        &format!("{ROOT}/configs"),
        Some(json!({"config": config})),
    );
    assert_eq!(status, 200, "{body}");
    body["config_id"].as_str().unwrap().to_owned()
}

/// The delivery status of a test send: in the answer when it worked, and in
/// the plugin's error, whose message is JSON, when it did not.
fn test_it(s: &Server, id: &str) -> Value {
    let (status, body) = call(s, "POST", &format!("{ROOT}/feature/test/{id}"), None);
    if status == 200 {
        return body["status_list"][0]["delivery_status"].clone();
    }
    let reason = body["error"]["reason"].as_str().unwrap_or_default();
    let shown: Value = serde_json::from_str(reason).unwrap_or(Value::Null);
    let delivery = shown["event_status_list"][0]["delivery_status"].clone();
    assert_eq!(delivery["status_code"], status.to_string(), "{body}");
    delivery
}

/// The plugin's words for a send that failed with `reason`.
fn fail(reason: &str) -> String {
    format!("Failed to send webhook message Failed: {reason}")
}

fn hook(url: &str) -> Value {
    json!({"name": "w", "config_type": "webhook", "webhook": {"url": url}})
}

fn deliverer(port: u16) -> Deliverer {
    Deliverer::new(system_lookup(), open_policy(port), Limits::default())
}

// AC-6
#[test]
fn a_webhook_is_posted_once_with_its_method_headers_and_body() {
    let t = sink("200 OK", "", "", Duration::ZERO);
    let s = rig(Some(deliverer(t.addr.port())));
    let id = make(
        &s,
        json!({"name": "w", "config_type": "webhook", "webhook": {
        "url": format!("http://127.0.0.1:{}/hook", t.addr.port()),
        "method": "PUT", "header_params": {"X-Token": "abc"}}}),
    );
    let status = test_it(&s, &id);
    assert_eq!(status["status_code"], "200", "{status}");
    assert_eq!(status["status_text"], "Success");
    let seen = t.seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "exactly one request");
    let req = seen[0].to_lowercase();
    assert!(req.starts_with("put /hook http/1.1"), "{req}");
    assert!(
        req.contains("x-token: abc") && req.contains("application/json"),
        "{req}"
    );
    assert!(seen[0].contains("\"text\""), "{}", seen[0]);
}

#[test]
fn slack_chime_and_teams_post_their_own_body_key() {
    for (ty, key) in [
        ("slack", "\"text\""),
        ("chime", "\"Content\""),
        ("microsoft_teams", "\"text\""),
    ] {
        let t = sink("200 OK", "", "", Duration::ZERO);
        let s = rig(Some(deliverer(t.addr.port())));
        let id = make(
            &s,
            json!({"name": ty, "config_type": ty, ty: {"url": format!(
                "http://127.0.0.1:{}/hooks.slack.com/services/hooks.chime.aws/incomingwebhooks/x?token=t",
                t.addr.port())}}),
        );
        assert_eq!(test_it(&s, &id)["status_code"], "200", "{ty}");
        assert!(t.seen.lock().unwrap()[0].contains(key), "{ty}");
    }
}

#[test]
fn a_failed_delivery_is_a_status_never_a_5xx_and_a_redirect_is_not_followed() {
    let bad = sink(
        "500 Internal Server Error",
        "",
        "MARKERSECRET",
        Duration::ZERO,
    );
    let s = rig(Some(deliverer(bad.addr.port())));
    let id = make(&s, hook(&format!("http://127.0.0.1:{}/", bad.addr.port())));
    let status = test_it(&s, &id);
    assert_eq!(
        (
            status["status_code"].as_str(),
            status["status_text"].as_str()
        ),
        (Some("500"), Some(fail("Internal Server Error").as_str()))
    );
    assert!(
        !status.to_string().contains("MARKERSECRET"),
        "the target's body is never shown"
    );

    let other = sink("200 OK", "", "", Duration::ZERO);
    let extra: &'static str = Box::leak(
        format!("location: http://127.0.0.1:{}/\r\n", other.addr.port()).into_boxed_str(),
    );
    let moved = sink("302 Found", extra, "", Duration::ZERO);
    let s = rig(Some(Deliverer::new(
        system_lookup(),
        EgressPolicy {
            allow_private: true,
            ports: vec![moved.addr.port(), other.addr.port()],
            deny_hosts: vec![],
        },
        Limits::default(),
    )));
    let id = make(
        &s,
        hook(&format!("http://127.0.0.1:{}/", moved.addr.port())),
    );
    assert_eq!(test_it(&s, &id)["status_text"], fail("Found").as_str());
    assert_eq!(
        other.seen.lock().unwrap().len(),
        0,
        "the redirect was not followed"
    );
    assert_eq!(moved.seen.lock().unwrap().len(), 1, "and not repeated");

    // nothing listening
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let s = rig(Some(deliverer(port)));
    let id = make(&s, hook(&format!("http://127.0.0.1:{port}/")));
    assert_eq!(
        test_it(&s, &id)["status_text"],
        fail("Connection failed").as_str()
    );
}

#[test]
fn a_disabled_config_and_a_type_not_served_are_not_sent() {
    let t = sink("200 OK", "", "", Duration::ZERO);
    let s = rig(Some(deliverer(t.addr.port())));
    let mut c = hook(&format!("http://127.0.0.1:{}/", t.addr.port()));
    c["is_enabled"] = json!(false);
    let id = make(&s, c);
    assert_eq!(test_it(&s, &id)["status_code"], "423");
    let id = make(
        &s,
        json!({"name": "e", "config_type": "sns", "sns": {"topic_arn": "arn:aws:sns:us-east-1:1:t"}}),
    );
    // No operator account declares the topic's role: nothing is signed or sent.
    assert_eq!(test_it(&s, &id)["status_code"], "500");
    assert_eq!(t.seen.lock().unwrap().len(), 0);
    let (status, _) = call(&s, "GET", &format!("{ROOT}/feature/test/missing"), None);
    assert_eq!(status, 404);
}

#[test]
fn without_a_deliverer_the_test_route_is_501() {
    let s = rig(None);
    let id = make(&s, hook("http://example.invalid/"));
    let (status, _) = call(&s, "POST", &format!("{ROOT}/feature/test/{id}"), None);
    assert_eq!(status, 501);
}

// AC-7
#[test]
fn the_default_policy_refuses_a_private_target_and_connects_to_nothing() {
    let t = sink("200 OK", "", "", Duration::ZERO);
    let s = rig(Some(Deliverer::new(
        system_lookup(),
        EgressPolicy {
            ports: vec![t.addr.port()],
            ..EgressPolicy::default()
        },
        Limits::default(),
    )));
    for url in [
        format!("http://127.0.0.1:{}/", t.addr.port()),
        format!("http://localhost:{}/", t.addr.port()),
        format!("http://[::1]:{}/", t.addr.port()),
        format!("http://[::ffff:127.0.0.1]:{}/", t.addr.port()),
    ] {
        let id = make(&s, hook(&url));
        let status = test_it(&s, &id);
        assert_eq!(
            status["status_text"],
            fail("Forbidden").as_str(),
            "{url}: {status}"
        );
    }
    assert_eq!(t.seen.lock().unwrap().len(), 0);
}

#[test]
fn a_name_with_one_bad_record_among_good_ones_is_refused() {
    let t = sink("200 OK", "", "", Duration::ZERO);
    let port = t.addr.port();
    let lookup: oss_node::notifications_route::Lookup = Arc::new(move |_, p| {
        Ok(vec![
            SocketAddr::new("8.8.8.8".parse().unwrap(), p),
            SocketAddr::new("127.0.0.1".parse().unwrap(), port),
        ])
    });
    let s = rig(Some(Deliverer::new(
        lookup,
        EgressPolicy {
            ports: vec![port],
            ..EgressPolicy::default()
        },
        Limits::default(),
    )));
    let id = make(&s, hook(&format!("http://mixed.example:{port}/")));
    assert_eq!(test_it(&s, &id)["status_text"], fail("Forbidden").as_str());
    assert_eq!(t.seen.lock().unwrap().len(), 0);
}

#[test]
fn a_port_outside_the_list_a_denied_host_and_odd_literals_are_refused() {
    let t = sink("200 OK", "", "", Duration::ZERO);
    let port = t.addr.port();
    let s = rig(Some(Deliverer::new(
        system_lookup(),
        EgressPolicy {
            allow_private: true,
            ports: vec![80],
            deny_hosts: vec!["denied.example".into()],
        },
        Limits::default(),
    )));
    for url in [
        format!("http://127.0.0.1:{port}/"),
        "http://denied.example/".to_owned(),
        "http://2130706433/".to_owned(),
        "http://0x7f.0.0.1/".to_owned(),
    ] {
        let id = make(&s, hook(&url));
        assert_eq!(
            test_it(&s, &id)["status_text"],
            fail("Forbidden").as_str(),
            "{url}"
        );
    }
    assert_eq!(t.seen.lock().unwrap().len(), 0);
}

#[test]
fn address_classes_are_refused_unless_the_operator_allows_private() {
    let refused = [
        "0.0.0.0",
        "10.1.2.3",
        "100.64.0.1",
        "127.0.0.1",
        "169.254.169.254",
        "172.16.0.1",
        "192.168.1.1",
        "198.18.0.1",
        "224.0.0.1",
        "240.0.0.1",
        "255.255.255.255",
        "::",
        "::1",
        "fe80::1",
        "fc00::1",
        "fd12::1",
        "ff02::1",
        "::ffff:10.0.0.1",
        "::ffff:127.0.0.1",
        "64:ff9b::7f00:1",
        "64:ff9b::a9fe:a9fe",
        "2002:7f00:1::1",
        "2001:db8::1",
    ];
    for a in refused {
        let ip: IpAddr = a.parse().unwrap();
        assert!(!permitted(ip, false), "{a} must be refused");
        assert!(
            permitted(ip, true),
            "{a} is allowed when the operator allows private"
        );
    }
    for a in [
        "8.8.8.8",
        "1.1.1.1",
        "93.184.216.34",
        "2606:4700:4700::1111",
        "::ffff:8.8.8.8",
    ] {
        assert!(permitted(a.parse().unwrap(), false), "{a} must be allowed");
    }
}

#[test]
fn a_body_over_the_bound_is_refused_before_any_connection_and_load_is_bounded() {
    let t = sink("200 OK", "", "", Duration::ZERO);
    let s = rig(Some(Deliverer::new(
        system_lookup(),
        open_policy(t.addr.port()),
        Limits {
            max_body: 8,
            ..Limits::default()
        },
    )));
    let id = make(&s, hook(&format!("http://127.0.0.1:{}/", t.addr.port())));
    assert_eq!(
        test_it(&s, &id)["status_text"],
        fail("Payload Too Large").as_str()
    );
    assert_eq!(t.seen.lock().unwrap().len(), 0);

    let slow = sink("200 OK", "", "", Duration::from_millis(800));
    let s = Arc::new(rig(Some(Deliverer::new(
        system_lookup(),
        open_policy(slow.addr.port()),
        Limits {
            running: 1,
            waiting: 0,
            ..Limits::default()
        },
    ))));
    let id = make(&s, hook(&format!("http://127.0.0.1:{}/", slow.addr.port())));
    let done = Arc::new(AtomicUsize::new(0));
    let texts: Vec<String> = (0..2)
        .map(|_| {
            let (s, id, done) = (Arc::clone(&s), id.clone(), Arc::clone(&done));
            std::thread::spawn(move || {
                let t = test_it(&s, &id)["status_text"].as_str().unwrap().to_owned();
                done.fetch_add(1, Ordering::SeqCst);
                t
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect();
    assert!(
        texts.contains(&"Success".to_owned()) && texts.contains(&fail("Too Many Requests")),
        "{texts:?}"
    );
}
