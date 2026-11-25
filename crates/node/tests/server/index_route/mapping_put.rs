//! docs/specs/node-index-mapping-put: adding fields to an index.

use super::*;

fn parsed(text: &str) -> serde_json::Value {
    serde_json::from_str(text).expect("json")
}

fn create(r: &Rig, name: &str, mapping: Option<&str>) {
    let body = mapping.map(|m| format!(r#"{{"mappings":{m}}}"#));
    assert_eq!(
        call(r, "PUT", &format!("/{name}"), body.as_deref()).status,
        200
    );
}

fn stored(r: &Rig, name: &str) -> (serde_json::Value, i64) {
    let mut buf = vec![0u8; BUF];
    let rec = r
        .descriptors
        .get(name, &mut buf)
        .expect("read")
        .expect("there");
    (
        parsed(rec.mapping.as_deref().unwrap_or("{}")),
        rec.mapping_version,
    )
}

fn spent(r: &Rig) -> u64 {
    ops(r).iter().map(|(_, n)| n).sum()
}

const ADD_N: &str = r#"{"properties":{"n":{"type":"integer"}}}"#;

// AC-1
#[test]
fn an_update_adds_a_field_and_moves_the_version_by_one() {
    let r = rig();
    create(
        &r,
        "logs",
        Some(r#"{"properties":{"t":{"type":"keyword"}}}"#),
    );
    let (_, before) = stored(&r, "logs");
    for (n, (method, path)) in [("PUT", "_mapping"), ("POST", "_mappings")]
        .iter()
        .enumerate()
    {
        let field = format!("f{n}");
        let body = format!(r#"{{"properties":{{"{field}":{{"type":"integer"}}}}}}"#);
        let reply = call(&r, method, &format!("/logs/{path}"), Some(&body));
        assert_eq!(reply.status, 200, "{}", reply.text());
        assert_eq!(parsed(&reply.text()), parsed(r#"{"acknowledged":true}"#));
    }
    let (mapping, version) = stored(&r, "logs");
    assert_eq!(version, before + 2);
    let props = &mapping["properties"];
    assert_eq!(props["t"]["type"], "keyword", "{mapping}");
    assert_eq!(props["f0"]["type"], "integer", "{mapping}");
    assert_eq!(props["f1"]["type"], "integer", "{mapping}");
    let got = call(&r, "GET", "/logs/_mapping", None);
    assert_eq!(parsed(&got.text())["logs"]["mappings"], mapping);
}

// AC-1, an index created with no mapping
#[test]
fn an_index_with_no_mapping_takes_the_first_one() {
    let r = rig();
    create(&r, "bare", None);
    assert_eq!(call(&r, "PUT", "/bare/_mapping", Some(ADD_N)).status, 200);
    assert_eq!(stored(&r, "bare").0["properties"]["n"]["type"], "integer");
}

// AC-2, AC-6
#[test]
fn an_update_that_changes_nothing_writes_nothing() {
    let r = rig();
    create(&r, "logs", None);
    assert_eq!(call(&r, "PUT", "/logs/_mapping", Some(ADD_N)).status, 200);
    let (_, version) = stored(&r, "logs");
    let before = spent(&r);
    assert_eq!(call(&r, "PUT", "/logs/_mapping", Some(ADD_N)).status, 200);
    assert_eq!(spent(&r) - before, 1, "one read: {:?}", ops(&r));
    assert_eq!(stored(&r, "logs").1, version);
    let before = spent(&r);
    let other = r#"{"properties":{"m":{"type":"integer"}}}"#;
    assert_eq!(call(&r, "PUT", "/logs/_mapping", Some(other)).status, 200);
    assert_eq!(spent(&r) - before, 2, "a read and a swap: {:?}", ops(&r));
}

// AC-3
#[test]
fn a_conflicting_or_unreadable_body_is_refused_and_writes_nothing() {
    let r = rig();
    create(&r, "logs", Some(ADD_N));
    let (kept, version) = stored(&r, "logs");
    let conflict = call(
        &r,
        "PUT",
        "/logs/_mapping",
        Some(r#"{"properties":{"n":{"type":"keyword"}}}"#),
    );
    assert_eq!(conflict.status, 400, "{}", conflict.text());
    let junk = call(&r, "PUT", "/logs/_mapping", Some("{not json"));
    assert_eq!(junk.status, 400, "{}", junk.text());
    assert_eq!(stored(&r, "logs"), (kept, version));
}

// AC-4
#[test]
fn a_missing_body_or_a_name_that_is_not_an_index_is_refused() {
    let r = rig();
    create(&r, "logs", None);
    let alias = oss_format::AliasRecord::new("a", &["logs"]);
    let mut buf = vec![0u8; BUF];
    r.descriptors
        .create_alias(&alias, 1, &mut buf)
        .expect("alias");
    let before = spent(&r);
    let none = call(&r, "PUT", "/logs/_mapping", None);
    assert_eq!(none.status, 400, "{}", none.text());
    assert!(none.text().contains("missing_body"), "{}", none.text());
    assert!(none.text().contains("a mapping body is required"));
    assert_eq!(spent(&r), before, "no request to the store");
    for name in ["nothing", "a", "logs,other", "log*"] {
        let reply = call(&r, "PUT", &format!("/{name}/_mapping"), Some(ADD_N));
        assert_eq!(reply.status, 404, "{name}: {}", reply.text());
    }
    assert_eq!(call(&r, "PUT", "/_all/_mapping", Some(ADD_N)).status, 501);
}

// AC-8
#[test]
fn a_body_of_one_mebibyte_is_read_and_one_byte_more_is_refused() {
    let r = rig();
    create(&r, "logs", None);
    let body = |len: usize| {
        let pad = " ".repeat(len - ADD_N.len());
        format!("{ADD_N}{pad}")
    };
    let over = call(&r, "PUT", "/logs/_mapping", Some(&body((1 << 20) + 1)));
    assert_eq!(over.status, 400, "{}", over.text());
    assert_eq!(stored(&r, "logs").0["properties"], serde_json::Value::Null);
    let at = call(&r, "PUT", "/logs/_mapping", Some(&body(1 << 20)));
    assert_eq!(at.status, 200, "{}", at.text());
}
