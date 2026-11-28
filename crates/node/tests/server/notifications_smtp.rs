//! docs/specs/notifications slice 3: email through an SMTP relay the operator
//! declared, against an in-process relay that speaks the protocol and TLS.

use crate::notifications_send::open_policy;
use crate::support;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use oss_blob::{BlobStore, Emulator};
use oss_mem::MemoryBudget;
use oss_node::notifications_route::{
    Accounts, Deliverer, Limits, NotificationsService, Senders, routes, system_lookup,
};
use oss_rest::{Router, Server};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::{Value, json};

const ROOT: &str = "/_plugins/_notifications";

#[derive(Clone, Copy, PartialEq)]
enum Tls {
    None,
    Upgrade,
    Implicit,
}

#[derive(Default)]
struct Seen {
    commands: Vec<String>,
    data: Vec<String>,
}

struct Relay {
    port: u16,
    seen: Arc<Mutex<Seen>>,
    ca: String,
}

fn line<S: Read>(s: &mut S) -> Option<String> {
    let mut out = Vec::new();
    let mut b = [0u8; 1];
    loop {
        if s.read(&mut b).ok()? == 0 {
            return None;
        }
        if b[0] == b'\n' {
            return Some(String::from_utf8_lossy(&out).trim_end().to_owned());
        }
        out.push(b[0]);
    }
}

/// One half of the dialog; true when the client asked for TLS and the
/// caller must wrap the stream and call it again.
fn dialog<S: Read + Write>(s: &mut S, greet: bool, offer: bool, seen: &Mutex<Seen>) -> bool {
    if greet {
        let _ = s.write_all(b"220 relay ready\r\n");
    }
    while let Some(l) = line(s) {
        seen.lock().unwrap().commands.push(l.clone());
        let reply: &[u8] = if l.starts_with("EHLO") {
            if offer {
                b"250-relay\r\n250-STARTTLS\r\n250 AUTH PLAIN\r\n"
            } else {
                b"250-relay\r\n250 AUTH PLAIN\r\n"
            }
        } else if l == "STARTTLS" {
            let _ = s.write_all(b"220 go ahead\r\n");
            return true;
        } else if l.starts_with("AUTH PLAIN") {
            // "\0user\0secret" in base64.
            if l == "AUTH PLAIN AHVzZXIAc2VjcmV0" {
                b"235 ok\r\n"
            } else {
                b"535 no\r\n"
            }
        } else if l.starts_with("RCPT TO:<bad@") {
            b"550 no such user\r\n"
        } else if l.starts_with("MAIL FROM") || l.starts_with("RCPT TO") {
            b"250 ok\r\n"
        } else if l == "DATA" {
            let _ = s.write_all(b"354 go\r\n");
            let mut body = String::new();
            while let Some(d) = line(s) {
                if d == "." {
                    break;
                }
                body.push_str(&d);
                body.push('\n');
            }
            seen.lock().unwrap().data.push(body);
            b"250 queued\r\n"
        } else if l == "QUIT" {
            let _ = s.write_all(b"221 bye\r\n");
            return false;
        } else {
            b"500 what\r\n"
        };
        let _ = s.write_all(reply);
    }
    false
}

fn relay(mode: Tls) -> Relay {
    let made = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).expect("cert");
    let cert = made.cert.der().clone();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(made.key_pair.serialize_der()));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = Arc::new(
        rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(tcp) = stream else { continue };
            let (config, log) = (Arc::clone(&config), Arc::clone(&log));
            std::thread::spawn(move || serve(tcp, mode, &config, &log));
        }
    });
    Relay {
        port,
        seen,
        ca: made.cert.pem(),
    }
}

fn serve(tcp: TcpStream, mode: Tls, config: &Arc<rustls::ServerConfig>, seen: &Mutex<Seen>) {
    let wrap = |tcp: TcpStream| {
        rustls::StreamOwned::new(
            rustls::ServerConnection::new(Arc::clone(config)).unwrap(),
            tcp,
        )
    };
    match mode {
        Tls::None => {
            let mut tcp = tcp;
            dialog(&mut tcp, true, false, seen);
        }
        Tls::Implicit => {
            let mut s = wrap(tcp);
            dialog(&mut s, true, false, seen);
        }
        Tls::Upgrade => {
            let mut tcp = tcp;
            if dialog(&mut tcp, true, true, seen) {
                let mut s = wrap(tcp);
                dialog(&mut s, false, false, seen);
            }
        }
    }
}

fn rig(accounts: &Value, port: u16) -> Server {
    let accounts = Accounts::parse(&accounts.to_string()).expect("accounts");
    let deliverer = Arc::new(Deliverer::new(
        system_lookup(),
        open_policy(port),
        Limits::default(),
    ));
    let senders = Arc::new(Senders::new(deliverer, accounts));
    let blobs = Arc::new(Emulator::new(1 << 28, 5)) as Arc<dyn BlobStore>;
    let service = NotificationsService::new(&blobs, "", 1 << 16).with_senders(senders);
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

/// An email config through an smtp_account on `port` with `method`.
fn email(s: &Server, port: u16, method: &str, to: &[&str]) -> String {
    let account = make(
        s,
        json!({"name": "a", "config_type": "smtp_account", "smtp_account": {
            "host": "localhost", "port": port, "method": method,
            "from_address": "from@example.com"}}),
    );
    let list: Vec<Value> = to.iter().map(|a| json!({"recipient": a})).collect();
    make(
        s,
        json!({"name": "e", "config_type": "email", "email": {
            "email_account_id": account, "recipient_list": list}}),
    )
}

fn accounts(r: &Relay, login: bool) -> Value {
    let mut a = json!({"host": "localhost", "port": r.port, "ca_pem": r.ca});
    if login {
        a["username"] = json!("user");
        a["password"] = json!("secret");
    }
    json!({"smtp": [a]})
}

fn statuses(body: &Value) -> Vec<String> {
    body["status_list"][0]["email_recipient_status"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|r| {
                    r["delivery_status"]["status_code"]
                        .as_str()
                        .unwrap()
                        .to_owned()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn test_it(s: &Server, id: &str) -> (u16, Value) {
    call(s, "POST", &format!("{ROOT}/feature/test/{id}"), None)
}

fn decoded(body: &str) -> String {
    let text = body
        .split_once("\n\n")
        .map_or("", |(_, b)| b)
        .replace('\n', "");
    // Standard base64, decoded by hand to keep the test free of a crate.
    let table = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let values: Vec<u32> = text
        .bytes()
        .filter(|c| *c != b'=')
        .map(|c| u32::try_from(table.iter().position(|x| *x == c).unwrap()).unwrap())
        .collect();
    let mut out = Vec::new();
    for g in values.chunks(4) {
        let n = g
            .iter()
            .fold(0u32, |a, v| a.saturating_mul(64).saturating_add(*v));
        let missing = 4usize.saturating_sub(g.len());
        let n = n.saturating_mul(1u32 << u32::try_from(missing.saturating_mul(6)).unwrap());
        let bytes = n.to_be_bytes();
        out.extend_from_slice(&bytes[1..g.len()]);
    }
    String::from_utf8(out).unwrap()
}

#[test]
fn start_tls_then_auth_then_one_message_to_every_recipient() {
    let r = relay(Tls::Upgrade);
    let s = rig(&accounts(&r, true), r.port);
    let id = email(&s, r.port, "start_tls", &["a@example.com", "b@example.com"]);
    let (status, body) = test_it(&s, &id);
    assert_eq!(status, 200, "{body}");
    assert_eq!(statuses(&body), ["200", "200"]);
    let seen = r.seen.lock().unwrap();
    let cmds = seen.commands.join("|");
    let at = |p: &str| cmds.find(p).unwrap_or_else(|| panic!("{p} in {cmds}"));
    assert!(
        at("STARTTLS") < at("AUTH PLAIN") && at("AUTH PLAIN") < at("MAIL FROM:<from@example.com>")
    );
    assert!(cmds.contains("RCPT TO:<a@example.com>") && cmds.contains("RCPT TO:<b@example.com>"));
    assert_eq!(seen.data.len(), 1);
    assert!(seen.data[0].contains("To: <a@example.com>, <b@example.com>"));
    assert!(decoded(&seen.data[0]).contains("Test message"));
    assert!(!body.to_string().contains("secret"));
}

#[test]
fn implicit_tls_sends_and_a_rejected_recipient_is_its_own_failure() {
    let r = relay(Tls::Implicit);
    let s = rig(&accounts(&r, false), r.port);
    let id = email(&s, r.port, "ssl", &["a@example.com", "bad@example.com"]);
    let (status, body) = test_it(&s, &id);
    assert_eq!(status, 503, "{body}");
    assert_eq!(
        r.seen.lock().unwrap().data.len(),
        1,
        "the good recipient is still sent"
    );
    assert!(
        !body.to_string().contains("no such user"),
        "the relay's words are not echoed"
    );
}

#[test]
fn a_login_is_never_sent_without_tls() {
    let r = relay(Tls::None);
    let s = rig(&accounts(&r, true), r.port);
    let id = email(&s, r.port, "none", &["a@example.com"]);
    let (status, _) = test_it(&s, &id);
    assert_eq!(status, 503);
    assert!(
        r.seen.lock().unwrap().commands.is_empty(),
        "no connection was made"
    );
}

#[test]
fn a_relay_the_operator_did_not_declare_is_never_contacted() {
    let r = relay(Tls::None);
    let s = rig(&json!({}), r.port);
    let id = email(&s, r.port, "none", &["a@example.com"]);
    let (status, _) = test_it(&s, &id);
    assert_eq!(status, 503);
    assert!(r.seen.lock().unwrap().commands.is_empty());
}

#[test]
fn a_plain_relay_without_a_login_takes_the_message() {
    let r = relay(Tls::None);
    let mut a = accounts(&r, false);
    a["smtp"][0].as_object_mut().unwrap().remove("ca_pem");
    let s = rig(&a, r.port);
    let id = email(&s, r.port, "none", &["a@example.com"]);
    let (status, body) = test_it(&s, &id);
    assert_eq!(status, 200, "{body}");
    assert_eq!(r.seen.lock().unwrap().data.len(), 1);
}

#[test]
fn an_address_with_a_line_break_never_reaches_a_relay() {
    let r = relay(Tls::None);
    let s = rig(&accounts(&r, false), r.port);
    let account = make(
        &s,
        json!({"name": "a", "config_type": "smtp_account", "smtp_account": {
            "host": "localhost", "port": r.port, "method": "none", "from_address": "f@example.com"}}),
    );
    let (status, _) = call(
        &s,
        "POST",
        &format!("{ROOT}/configs"),
        Some(
            json!({"config": {"name": "e", "config_type": "email", "email": {
            "email_account_id": account,
            "recipient_list": [{"recipient": "a@example.com\r\nRCPT TO:<x@evil.com>"}]}}}),
        ),
    );
    assert_eq!(status, 400);
    assert!(r.seen.lock().unwrap().commands.is_empty());
}
