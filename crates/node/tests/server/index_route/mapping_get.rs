//! docs/specs/node-index-mapping-get: reading an index's mapping back.

use super::*;

const MAPPING: &str = r#"{"properties":{"n":{"type":"integer"},"t":{"type":"keyword"}}}"#;

fn parsed(text: &str) -> serde_json::Value {
    serde_json::from_str(text).expect("json")
}

fn create(r: &Rig, name: &str, body: Option<&str>) {
    assert_eq!(call(r, "PUT", &format!("/{name}"), body).status, 200);
}

// AC-1
#[test]
fn a_get_answers_the_stored_mapping_under_the_index_name() {
    let r = rig();
    create(&r, "logs", Some(&format!(r#"{{"mappings":{MAPPING}}}"#)));
    for path in ["/logs/_mapping", "/logs/_mappings"] {
        let reply = call(&r, "GET", path, None);
        assert_eq!(reply.status, 200, "{}", reply.text());
        let want = parsed(&format!(r#"{{"logs":{{"mappings":{MAPPING}}}}}"#));
        assert_eq!(parsed(&reply.text()), want, "{path}");
    }
}

// AC-2
#[test]
fn an_index_with_no_mapping_answers_an_empty_one() {
    let r = rig();
    create(&r, "bare", None);
    let reply = call(&r, "GET", "/bare/_mapping", None);
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(parsed(&reply.text()), parsed(r#"{"bare":{"mappings":{}}}"#));
}

// AC-3
#[test]
fn what_is_not_an_index_is_refused_before_it_costs_more_than_a_read() {
    let r = rig();
    create(&r, "logs", None);
    let alias = oss_format::AliasRecord::new("a", &["logs"]);
    let mut buf = vec![0u8; BUF];
    r.descriptors
        .create_alias(&alias, 1, &mut buf)
        .expect("alias");
    for name in ["nothing", "a", "logs,other", "log*"] {
        let reply = call(&r, "GET", &format!("/{name}/_mapping"), None);
        assert_eq!(reply.status, 404, "{name}: {}", reply.text());
        assert!(reply.text().contains("index_not_found"), "{}", reply.text());
        assert!(
            reply.text().contains(&format!("no such index: {name}")),
            "{}",
            reply.text()
        );
    }
    let before: u64 = ops(&r).iter().map(|(_, n)| n).sum();
    assert_eq!(call(&r, "GET", "/_all/_mapping", None).status, 501);
    let after: u64 = ops(&r).iter().map(|(_, n)| n).sum();
    assert!(after - before <= 1, "{:?}", ops(&r));
}

// AC-5
#[test]
fn a_get_is_one_read() {
    let r = rig();
    create(&r, "logs", Some(&format!(r#"{{"mappings":{MAPPING}}}"#)));
    let before = ops(&r);
    assert_eq!(call(&r, "GET", "/logs/_mapping", None).status, 200);
    let spent: u64 =
        ops(&r).iter().map(|(_, n)| n).sum::<u64>() - before.iter().map(|(_, n)| n).sum::<u64>();
    assert_eq!(spent, 1, "{:?}", ops(&r));
    assert!(ops(&r).iter().all(|(op, _)| *op != S3Op::List));
}

// AC-6
#[test]
fn the_parameters_the_java_consumes_are_accepted() {
    let r = rig();
    create(&r, "logs", None);
    let query = "local=true&include_defaults=true&ignore_unavailable=true&allow_no_indices=true&expand_wildcards=open&master_timeout=1s&cluster_manager_timeout=1s&timeout=1s&wait_for_active_shards=1&include_type_name=false&flat_settings=true";
    let reply = call(&r, "GET", &format!("/logs/_mapping?{query}"), None);
    assert_eq!(reply.status, 200, "{}", reply.text());
}
