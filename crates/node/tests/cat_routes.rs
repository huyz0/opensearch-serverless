//! docs/specs/node-cluster-routes: the cat and list index tables over REST.

#[path = "cluster_support/mod.rs"]
mod cluster_support;

use cluster_support::{call, get, index, rig, sent};
use oss_blob::S3Op;

#[test]
fn cat_indices_answers_names_lists_and_prefixes_and_refuses_paging() {
    let r = rig();
    for (name, shards) in [("logs-a", 2), ("logs-b", 1), ("docs", 3)] {
        index(&r, name, shards);
    }
    let all = get(&r, "/_cat/indices");
    assert_eq!(all.status, 200, "{}", all.text());
    assert_eq!(
        all.text(),
        "docs   docs-uuid   3 0 open 1\nlogs-a logs-a-uuid 2 0 open 1\nlogs-b logs-b-uuid 1 0 open 1\n"
    );
    let prefix = get(&r, "/_cat/indices/logs-*?format=json");
    assert_eq!(
        prefix.text(),
        r#"[{"index":"logs-a","uuid":"logs-a-uuid","pri":"2","rep":"0","status":"open","mapping_version":"1"},{"index":"logs-b","uuid":"logs-b-uuid","pri":"1","rep":"0","status":"open","mapping_version":"1"}]"#
    );
    assert_eq!(
        get(&r, "/_cat/indices/docs,logs-b?v=true").text(),
        "index  uuid        pri rep status mapping_version\ndocs   docs-uuid   3   0   open   1\nlogs-b logs-b-uuid 1   0   open   1\n"
    );
    assert_eq!(get(&r, "/_cat/indices/zzz-*").text(), "");
    let absent = get(&r, "/_cat/indices/docs,nope");
    assert_eq!(absent.status, 404, "{}", absent.text());
    assert!(
        absent.text().contains("no such index: nope"),
        "{}",
        absent.text()
    );
    for target in ["/_cat/indices?size=2", "/_cat/indices?next_token=x"] {
        let reply = get(&r, target);
        assert_eq!(reply.status, 400, "{target}: {}", reply.text());
        assert!(reply.text().contains("_cat/indices is not paginated"));
    }
}

#[test]
fn a_prefix_past_the_pattern_cap_is_refused_rather_than_cut() {
    let r = rig();
    for n in 0..1_001 {
        index(&r, &format!("many-{n:04}"), 1);
    }
    let reply = get(&r, "/_cat/indices/many-*");
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert!(
        reply.text().contains("too_many_indices"),
        "{}",
        reply.text()
    );
}

#[test]
fn list_indices_pages_in_name_order_and_the_token_resumes() {
    let r = rig();
    for name in ["a1", "a2", "a3", "a4", "a5", "b1"] {
        index(&r, name, 1);
    }
    let mut seen = Vec::new();
    let mut target = "/_list/indices/a*?size=2&format=json".to_owned();
    let mut pages = 0;
    loop {
        let reply = get(&r, &target);
        assert_eq!(reply.status, 200, "{}", reply.text());
        let text = reply.text();
        pages += 1;
        for part in text.split("\"index\":\"").skip(1) {
            seen.push(part.split('"').next().expect("name").to_owned());
        }
        let token = text
            .split("\"next_token\":")
            .nth(1)
            .expect("a token member")
            .split([',', '}'])
            .next()
            .expect("value")
            .trim_matches('"')
            .to_owned();
        if token == "null" {
            break;
        }
        target = format!("/_list/indices/a*?size=2&format=json&next_token={token}");
    }
    assert_eq!(seen, ["a1", "a2", "a3", "a4", "a5"]);
    assert_eq!(pages, 3);

    let first = get(&r, "/_list/indices/a*?size=2");
    assert!(
        first.text().contains("\nnext_token ") && !first.text().contains("next_token null"),
        "{}",
        first.text()
    );

    let named = get(&r, "/_list/indices/a1,b1?format=json").text();
    assert!(
        named.starts_with(r#"{"next_token":null,"indices":["#),
        "{named}"
    );
    let bare = get(&r, "/_list/indices?format=json").text();
    assert!(bare.contains("\"b1\""), "{bare}");
}

#[test]
fn list_indices_refuses_what_plan_refuses() {
    let r = rig();
    index(&r, "a1", 1);
    for (target, words) in [
        (
            "/_list/indices?size=abc",
            "size must be a number, got [abc]",
        ),
        (
            "/_list/indices?size=0",
            "size must be between 1 and [5000], got [0]",
        ),
        (
            "/_list/indices?size=5001",
            "size must be between 1 and [5000], got [5001]",
        ),
        (
            "/_list/indices/a1,b1?next_token=x",
            "next_token resumes a walk over one prefix pattern",
        ),
        (
            "/_list/indices/a*?next_token=!!!",
            "Parameter [next_token] has been tainted",
        ),
    ] {
        let reply = get(&r, target);
        assert_eq!(reply.status, 400, "{target}: {}", reply.text());
        assert!(reply.text().contains(words), "{target}: {}", reply.text());
    }
    assert_eq!(get(&r, "/_list/indices?h=index").status, 501);
    assert_eq!(get(&r, "/_cat/indices?s=index").status, 501);
}

#[test]
fn requests_are_what_the_spec_says() {
    let r = rig();
    index(&r, "logs", 1);
    index(&r, "docs", 1);
    let (_, read) = sent(&r, || get(&r, "/_cluster/settings"));
    assert_eq!(read, [(S3Op::Get, 1)], "a settings read");
    let (_, write) = sent(&r, || {
        call(
            &r,
            "PUT",
            "/_cluster/settings",
            Some(r#"{"persistent":{"a":"b"}}"#),
        )
    });
    assert_eq!(write, [(S3Op::Get, 1), (S3Op::Put, 1)], "a settings update");
    let (_, refused) = sent(&r, || {
        call(
            &r,
            "PUT",
            "/_cluster/settings",
            Some(r#"{"transient":{"a":"b"}}"#),
        )
    });
    assert!(refused.is_empty(), "{refused:?}");
    let (_, unscoped) = sent(&r, || get(&r, "/_cluster/health"));
    assert!(unscoped.is_empty(), "{unscoped:?}");
    let (_, named) = sent(&r, || get(&r, "/_cat/indices/logs,docs"));
    assert_eq!(named, [(S3Op::Get, 2)], "a cat of two names");
}
