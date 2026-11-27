//! docs/specs/node-delete-by-query: delete-by-query over a writer node's own
//! shards, with the search half faked.

// ARITH: offsets into a reply are small; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

use super::by_query_rig::{QUERY, call, gone, put, rig, source_of};
use crate::support;
use oss_coordinator::docs::by_query::Hit;

#[test]
fn the_matching_documents_are_deleted_and_counted() {
    let r = rig();
    let hits: Vec<Hit> = ["a", "b", "c"].iter().map(|id| put(&r, id)).collect();
    put(&r, "keep");
    *r.matches.hits.lock().unwrap() = hits;
    let reply = call(&r, "POST", "/docs/_delete_by_query", Some(QUERY));
    let text = reply.text();
    assert_eq!(reply.status, 200, "{text}");
    for part in [
        r#""timed_out":false,"total":3,"deleted":3,"batches":1,"version_conflicts":0,"noops":0"#,
        r#""failures":[]"#,
    ] {
        assert!(text.contains(part), "{part} in {text}");
    }
    assert!(["a", "b", "c"].iter().all(|id| gone(&r, id)));
    assert!(!gone(&r, "keep"), "a document that did not match stays");
    let queries = r.matches.queries.lock().unwrap();
    assert!(queries[0].contains("match"), "{queries:?}");
}

#[test]
fn a_document_rewritten_since_the_search_conflicts() {
    for (conflicts, status, deleted) in [("", 409, 0), ("?conflicts=proceed", 200, 1)] {
        let r = rig();
        let stale = put(&r, "a");
        put(&r, "a");
        let fresh = put(&r, "b");
        *r.matches.hits.lock().unwrap() = vec![stale, fresh];
        let reply = call(
            &r,
            "POST",
            &format!("/docs/_delete_by_query{conflicts}"),
            Some(QUERY),
        );
        let text = reply.text();
        assert_eq!(reply.status, status, "{conflicts}: {text}");
        assert!(text.contains(r#""version_conflicts":1"#), "{text}");
        assert!(text.contains(&format!(r#""deleted":{deleted}"#)), "{text}");
        if status == 409 {
            assert!(text.contains(r#""id":"a""#), "{text}");
            assert!(text.contains("version_conflict_engine_exception"), "{text}");
        } else {
            assert!(gone(&r, "b"), "the walk went on");
        }
    }
}

#[test]
fn max_docs_stops_the_walk() {
    let r = rig();
    let hits: Vec<Hit> = ["a", "b", "c"].iter().map(|id| put(&r, id)).collect();
    *r.matches.hits.lock().unwrap() = hits;
    let reply = call(&r, "POST", "/docs/_delete_by_query?max_docs=2", Some(QUERY));
    assert!(reply.text().contains(r#""deleted":2"#), "{}", reply.text());
    assert!(gone(&r, "a") && gone(&r, "b") && !gone(&r, "c"));
}

#[test]
fn a_request_this_node_cannot_serve_is_refused() {
    let r = rig();
    for (target, body, status, word) in [
        ("/nope/_delete_by_query", QUERY, 404, "index_not_found"),
        ("/docs/_delete_by_query", "{}", 400, "query is missing"),
        ("/docs/_delete_by_query", "{", 400, "failed to parse"),
        (
            "/docs/_delete_by_query?conflicts=bad",
            QUERY,
            400,
            "conflicts",
        ),
        ("/docs/_delete_by_query?max_docs=-1", QUERY, 400, "max_docs"),
        (
            "/docs/_delete_by_query?slices=2",
            QUERY,
            400,
            "not supported",
        ),
    ] {
        let reply = call(&r, "POST", target, Some(body));
        assert_eq!(reply.status, status, "{target}: {}", reply.text());
        assert!(reply.text().contains(word), "{target}: {}", reply.text());
    }
}

fn update_query(script: &str) -> String {
    format!(
        r#"{{"query":{{"match":{{"n":1}}}},"script":{{"source":"{script}","params":{{"by":4}}}}}}"#
    )
}

// docs/specs/node-update-by-query AC-1, AC-2
#[test]
fn a_script_rewrites_deletes_or_skips_the_matching_documents() {
    let _switch = support::PAINLESS.lock().unwrap_or_else(|e| e.into_inner());
    oss_reader::painless_switch::set_native_compile(true);
    let r = rig();
    let hits: Vec<Hit> = ["a", "b", "c"].iter().map(|id| put(&r, id)).collect();
    put(&r, "keep");
    *r.matches.hits.lock().unwrap() = hits;
    let reply = call(
        &r,
        "POST",
        "/docs/_update_by_query",
        Some(&update_query(
            "if (ctx._id == 'a') { ctx.op = 'delete' } else if (ctx._id == 'b') { ctx.op = 'noop' } else { ctx._source.n += params.by }",
        )),
    );
    let text = reply.text();
    assert_eq!(reply.status, 200, "{text}");
    for part in [
        r#""total":3,"updated":1,"deleted":1,"batches":1,"version_conflicts":0,"noops":1"#,
        r#""failures":[]"#,
    ] {
        assert!(text.contains(part), "{part} in {text}");
    }
    assert!(source_of(&r, "a").contains(r#""found":false"#));
    assert!(source_of(&r, "b").contains(r#""n":1"#));
    assert!(source_of(&r, "c").contains(r#""n":5"#));
    assert!(
        source_of(&r, "keep").contains(r#""n":1"#),
        "unmatched stays"
    );
    oss_reader::painless_switch::set_native_compile(false);
}

// docs/specs/node-update-by-query AC-3, AC-4
#[test]
fn an_update_conflicts_on_a_rewritten_document_and_stops_at_max_docs() {
    let _switch = support::PAINLESS.lock().unwrap_or_else(|e| e.into_inner());
    oss_reader::painless_switch::set_native_compile(true);
    let q = update_query("ctx._source.n += params.by");
    for (target, status, updated) in [
        ("/docs/_update_by_query", 409, 0),
        ("/docs/_update_by_query?conflicts=proceed", 200, 1),
    ] {
        let r = rig();
        let stale = put(&r, "a");
        put(&r, "a");
        let fresh = put(&r, "b");
        *r.matches.hits.lock().unwrap() = vec![stale, fresh];
        let reply = call(&r, "POST", target, Some(&q));
        let text = reply.text();
        assert_eq!(reply.status, status, "{target}: {text}");
        assert!(text.contains(r#""version_conflicts":1"#), "{text}");
        assert!(text.contains(&format!(r#""updated":{updated}"#)), "{text}");
    }
    let r = rig();
    let hits: Vec<Hit> = ["a", "b", "c"].iter().map(|id| put(&r, id)).collect();
    *r.matches.hits.lock().unwrap() = hits;
    let reply = call(&r, "POST", "/docs/_update_by_query?max_docs=2", Some(&q));
    assert!(reply.text().contains(r#""updated":2"#), "{}", reply.text());
    assert!(source_of(&r, "c").contains(r#""n":1"#));
    oss_reader::painless_switch::set_native_compile(false);
}

// docs/specs/node-update-by-query AC-5
#[test]
fn an_update_by_query_this_node_cannot_serve_is_refused() {
    let _switch = support::PAINLESS.lock().unwrap_or_else(|e| e.into_inner());
    let r = rig();
    oss_reader::painless_switch::set_native_compile(false);
    let off = call(
        &r,
        "POST",
        "/docs/_update_by_query",
        Some(&update_query("1")),
    );
    assert_eq!(off.status, 501, "{}", off.text());
    oss_reader::painless_switch::set_native_compile(true);
    let match_all = r#"{"query":{"match_all":{}}}"#.to_owned();
    let stored = r#"{"script":{"id":"s"}}"#.to_owned();
    for (target, body, word) in [
        (
            "/nope/_update_by_query",
            update_query("1"),
            "index_not_found",
        ),
        ("/docs/_update_by_query", match_all, "[script] is required"),
        (
            "/docs/_update_by_query?slices=2",
            update_query("1"),
            "not supported",
        ),
        ("/docs/_update_by_query", stored, "[script]"),
    ] {
        let reply = call(&r, "POST", target, Some(&body));
        assert!(
            reply.status == 400 || reply.status == 404,
            "{target}: {}",
            reply.text()
        );
        assert!(reply.text().contains(word), "{target}: {}", reply.text());
    }
    // A script that does not compile, or throws, fails the request with its error.
    let hits = vec![put(&r, "a")];
    *r.matches.hits.lock().unwrap() = hits;
    for script in ["ctx._source.n = ", "ctx._source.n = ctx.nope.x"] {
        let reply = call(
            &r,
            "POST",
            "/docs/_update_by_query",
            Some(&update_query(script)),
        );
        assert_eq!(reply.status, 400, "{script}: {}", reply.text());
        assert!(
            reply.text().contains("script_exception"),
            "{}",
            reply.text()
        );
    }
    oss_reader::painless_switch::set_native_compile(false);
}
