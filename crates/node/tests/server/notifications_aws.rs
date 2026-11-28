//! docs/specs/notifications slice 4: SNS and SES sends, signed with SigV4
//! under an operator's account, to a local service that checks the signature.

use crate::notifications_send::{open_policy, sink};
use crate::support;

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use oss_blob::s3::sigv4::{Scope, canonical_request, sha256_hex, signature, string_to_sign};
use oss_blob::{BlobStore, Emulator};
use oss_mem::MemoryBudget;
use oss_node::notifications_route::{
    Accounts, Deliverer, Limits, NotificationsService, Senders, routes, system_lookup,
};
use oss_rest::{Router, Server};
use serde_json::{Value, json};

const ROOT: &str = "/_plugins/_notifications";
const ROLE: &str = "arn:aws:iam::1:role/notify";
const SECRET: &str = "SECRETKEYVALUE";

fn rig(port: u16) -> Server {
    let accounts = Accounts::parse(
        &json!({"aws": [{
            "role_arn": ROLE, "access_key": "AKTEST", "secret_key": SECRET,
            "endpoint": format!("http://127.0.0.1:{port}"),
        }]})
        .to_string(),
    )
    .expect("accounts");
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

fn header(raw: &str, name: &str) -> String {
    raw.lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case(name).then(|| v.trim().to_owned())
        })
        .unwrap_or_default()
}

/// Recomputes the signature of a request the service received, from the
/// secret only the test knows, and asserts it is the one sent.
fn assert_signed(raw: &str, service: &str, region: &str) -> String {
    let (head, body) = raw.split_once("\r\n\r\n").expect("a body");
    let auth = header(head, "authorization");
    assert!(auth.contains("Credential=AKTEST/"), "{auth}");
    let sent = auth.rsplit("Signature=").next().unwrap().to_owned();
    let date = header(head, "x-amz-date");
    let content_type = header(head, "content-type");
    let host = header(head, "host");
    let path = head
        .lines()
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .unwrap_or("/");
    let signed = [
        ("content-type", content_type.as_str()),
        ("host", host.as_str()),
        ("x-amz-date", date.as_str()),
    ];
    let canonical = canonical_request("POST", path, "", &signed, &sha256_hex(body.as_bytes()));
    let scope = Scope {
        date: &date[..8],
        region,
        service,
    };
    let want = signature(
        SECRET,
        &scope,
        &string_to_sign(&date, &scope, &canonical.request),
    );
    assert_eq!(sent, want, "the signature does not verify: {head}");
    assert!(!raw.contains(SECRET), "the secret is never sent");
    body.to_owned()
}

fn sent(s: &Server, id: &str) -> (u16, Value) {
    call(s, "POST", &format!("{ROOT}/feature/test/{id}"), None)
}

#[test]
fn an_sns_test_publishes_a_signed_request_under_the_declared_role() {
    let t = sink("200 OK", "", "<PublishResponse/>", Duration::ZERO);
    let s = rig(t.addr.port());
    let id = make(
        &s,
        json!({"name": "t", "config_type": "sns", "sns": {
            "topic_arn": "arn:aws:sns:eu-west-1:1:topic", "role_arn": ROLE}}),
    );
    let (status, body) = sent(&s, &id);
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body["status_list"][0]["delivery_status"]["status_code"],
        "200"
    );
    let seen = t.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    let payload = assert_signed(&seen[0], "sns", "eu-west-1");
    assert!(payload.contains("Action=Publish"), "{payload}");
    assert!(
        payload.contains("TopicArn=arn%3Aaws%3Asns%3Aeu-west-1%3A1%3Atopic"),
        "{payload}"
    );
    assert!(
        !body.to_string().contains("PublishResponse"),
        "target body is never echoed"
    );
}

#[test]
fn an_sns_config_naming_an_undeclared_role_sends_nothing() {
    let t = sink("200 OK", "", "", Duration::ZERO);
    let s = rig(t.addr.port());
    let id = make(
        &s,
        json!({"name": "t", "config_type": "sns", "sns": {
            "topic_arn": "arn:aws:sns:eu-west-1:1:topic",
            "role_arn": "arn:aws:iam::2:role/other"}}),
    );
    let (status, body) = sent(&s, &id);
    assert_eq!(status, 500, "{body}");
    assert!(!body.to_string().contains(SECRET));
    assert_eq!(t.seen.lock().unwrap().len(), 0);
}

#[test]
fn an_email_through_an_ses_account_is_signed_and_addressed_to_each_recipient() {
    let t = sink("200 OK", "", "{}", Duration::ZERO);
    let s = rig(t.addr.port());
    let account = make(
        &s,
        json!({"name": "a", "config_type": "ses_account", "ses_account": {
            "region": "us-east-1", "role_arn": ROLE, "from_address": "from@example.com"}}),
    );
    let id = make(
        &s,
        json!({"name": "e", "config_type": "email", "email": {
            "email_account_id": account,
            "recipient_list": [{"recipient": "a@example.com"}, {"recipient": "b@example.com"}]}}),
    );
    let (status, body) = sent(&s, &id);
    assert_eq!(status, 200, "{body}");
    let seen = t.seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "one request carries both recipients");
    let payload: Value =
        serde_json::from_str(&assert_signed(&seen[0], "ses", "us-east-1")).expect("json");
    assert_eq!(payload["FromEmailAddress"], "from@example.com");
    assert_eq!(
        payload["Destination"]["ToAddresses"],
        json!(["a@example.com", "b@example.com"])
    );
}

#[test]
fn an_email_to_more_than_fifty_recipients_is_two_ses_requests() {
    let t = sink("200 OK", "", "{}", Duration::ZERO);
    let s = rig(t.addr.port());
    let account = make(
        &s,
        json!({"name": "a", "config_type": "ses_account", "ses_account": {
            "region": "us-east-1", "role_arn": ROLE, "from_address": "from@example.com"}}),
    );
    let list: Vec<Value> = (0..51)
        .map(|i| json!({"recipient": format!("u{i}@example.com")}))
        .collect();
    let id = make(
        &s,
        json!({"name": "e", "config_type": "email", "email": {
            "email_account_id": account, "recipient_list": list}}),
    );
    let (status, body) = sent(&s, &id);
    assert_eq!(status, 200, "{body}");
    assert_eq!(t.seen.lock().unwrap().len(), 2);
}

#[test]
fn an_ses_account_naming_an_undeclared_role_sends_nothing() {
    let t = sink("200 OK", "", "", Duration::ZERO);
    let s = rig(t.addr.port());
    let account = make(
        &s,
        json!({"name": "a", "config_type": "ses_account", "ses_account": {
            "region": "us-east-1", "role_arn": "arn:aws:iam::2:role/other",
            "from_address": "from@example.com"}}),
    );
    let id = make(
        &s,
        json!({"name": "e", "config_type": "email", "email": {
            "email_account_id": account, "recipient_list": [{"recipient": "a@example.com"}]}}),
    );
    let (status, _) = sent(&s, &id);
    assert_ne!(status, 200);
    assert_eq!(t.seen.lock().unwrap().len(), 0);
}
