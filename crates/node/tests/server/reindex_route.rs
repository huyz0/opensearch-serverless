//! docs/specs/node-reindex: `POST /_reindex` between two indexes of a writer
//! node, with the search half faked.

// ARITH: offsets into a reply are small; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

use super::by_query_rig::{Rig, call, put, rig, source_of};
use crate::support;
use oss_coordinator::docs::by_query::Hit;

fn reindex_body(extra: &str) -> String {
    format!(
        r#"{{"source":{{"index":"docs","query":{{"match":{{"n":1}}}}}},"dest":{{"index":"dest"{extra}}}}}"#
    )
}

/// `body` without its closing brace, ready for a `"script"` member.
fn with_script(body: &str) -> String {
    body.strip_suffix('}').expect("an object").to_owned()
}

fn dest_of(r: &Rig, id: &str) -> String {
    call(r, "GET", &format!("/dest/_doc/{id}"), None).text()
}

// docs/specs/node-reindex AC-1
#[test]
fn matching_documents_are_copied_to_the_destination() {
    let r = rig();
    let hits: Vec<Hit> = ["a", "b"].iter().map(|id| put(&r, id)).collect();
    put(&r, "keep");
    *r.matches.hits.lock().unwrap() = hits;
    let reply = call(&r, "POST", "/_reindex", Some(&reindex_body("")));
    let text = reply.text();
    assert_eq!(reply.status, 200, "{text}");
    assert!(
        text.contains(r#""total":2,"updated":0,"created":2"#),
        "{text}"
    );
    assert!(dest_of(&r, "a").contains(r#""n":1"#));
    assert!(dest_of(&r, "b").contains(r#""found":true"#));
    assert!(dest_of(&r, "keep").contains(r#""found":false"#));
    assert!(
        source_of(&r, "a").contains(r#""found":true"#),
        "source stays"
    );
}

// docs/specs/node-reindex AC-2
#[test]
fn a_script_edits_skips_or_deletes_in_the_destination() {
    let _switch = support::PAINLESS.lock().unwrap_or_else(|e| e.into_inner());
    oss_reader::painless_switch::set_native_compile(true);
    let r = rig();
    let hits: Vec<Hit> = ["a", "b", "c"].iter().map(|id| put(&r, id)).collect();
    *r.matches.hits.lock().unwrap() = hits;
    // `a` exists in the destination, so the script's delete has something to remove.
    call(&r, "PUT", "/dest/_doc/a", Some(r#"{"n":9}"#));
    let script = r#","script":{"source":"if (ctx._id == 'a') { ctx.op = 'delete' } else if (ctx._id == 'b') { ctx.op = 'noop' } else { ctx._source.n += params.by }","params":{"by":4}}"#;
    let body = with_script(&reindex_body("")) + script + "}";
    let reply = call(&r, "POST", "/_reindex", Some(&body));
    let text = reply.text();
    assert_eq!(reply.status, 200, "{text}");
    assert!(
        text.contains(r#""total":3"#) && text.contains(r#""created":1"#),
        "{text}"
    );
    assert!(
        text.contains(r#""deleted":1"#) && text.contains(r#""noops":1"#),
        "{text}"
    );
    assert!(dest_of(&r, "a").contains(r#""found":false"#));
    assert!(dest_of(&r, "b").contains(r#""found":false"#));
    assert!(dest_of(&r, "c").contains(r#""n":5"#));
    oss_reader::painless_switch::set_native_compile(false);
}

// docs/specs/node-reindex AC-3
#[test]
fn op_type_create_conflicts_on_an_existing_document_and_index_overwrites() {
    for (extra, query, status, created, updated, conflicts) in [
        (r#","op_type":"create""#, "", 409, 0, 0, 1),
        (r#","op_type":"create""#, "?conflicts=proceed", 200, 1, 0, 1),
        ("", "", 200, 1, 1, 0),
    ] {
        let r = rig();
        let hits: Vec<Hit> = ["a", "b"].iter().map(|id| put(&r, id)).collect();
        *r.matches.hits.lock().unwrap() = hits;
        call(&r, "PUT", "/dest/_doc/a", Some(r#"{"n":9}"#));
        let reply = call(
            &r,
            "POST",
            &format!("/_reindex{query}"),
            Some(&reindex_body(extra)),
        );
        let text = reply.text();
        assert_eq!(reply.status, status, "{extra}{query}: {text}");
        assert!(
            text.contains(&format!(r#""version_conflicts":{conflicts}"#)),
            "{text}"
        );
        if status == 200 {
            assert!(text.contains(&format!(r#""created":{created}"#)), "{text}");
            assert!(text.contains(&format!(r#""updated":{updated}"#)), "{text}");
        } else {
            assert!(text.contains("version_conflict_engine_exception"), "{text}");
        }
        if extra.is_empty() {
            assert!(dest_of(&r, "a").contains(r#""n":1"#), "overwritten");
        }
    }
}

// docs/specs/node-reindex AC-4
#[test]
fn reindex_max_docs_bounds_the_copy() {
    let r = rig();
    let hits: Vec<Hit> = ["a", "b", "c"].iter().map(|id| put(&r, id)).collect();
    *r.matches.hits.lock().unwrap() = hits;
    let reply = call(&r, "POST", "/_reindex?max_docs=2", Some(&reindex_body("")));
    assert!(reply.text().contains(r#""created":2"#), "{}", reply.text());
    assert!(dest_of(&r, "c").contains(r#""found":false"#));
}

// docs/specs/node-reindex AC-5
#[test]
fn a_reindex_this_node_cannot_serve_is_refused() {
    let _switch = support::PAINLESS.lock().unwrap_or_else(|e| e.into_inner());
    oss_reader::painless_switch::set_native_compile(false);
    let r = rig();
    let scripted = with_script(&reindex_body("")) + r#","script":{"source":"1"}}"#;
    let off = call(&r, "POST", "/_reindex", Some(&scripted));
    assert_eq!(off.status, 501, "{}", off.text());
    oss_reader::painless_switch::set_native_compile(true);
    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "/_reindex",
            r#"{"source":{"index":"nope"},"dest":{"index":"dest"}}"#.into(),
            404,
            "index_not_found",
        ),
        (
            "/_reindex",
            r#"{"source":{"index":"docs"},"dest":{"index":"nope"}}"#.into(),
            404,
            "index_not_found",
        ),
        (
            "/_reindex",
            r#"{"source":{"index":"docs"},"dest":{"index":"docs"}}"#.into(),
            400,
            "cannot write into",
        ),
        (
            "/_reindex",
            r#"{"source":{"index":["docs","dest"]},"dest":{"index":"dest"}}"#.into(),
            400,
            "index",
        ),
        (
            "/_reindex",
            r#"{"source":{"index":"do*"},"dest":{"index":"dest"}}"#.into(),
            400,
            "index",
        ),
        (
            "/_reindex",
            r#"{"source":{"index":"docs","size":5},"dest":{"index":"dest"}}"#.into(),
            400,
            "size",
        ),
        (
            "/_reindex",
            r#"{"source":{"index":"docs"},"dest":{"index":"dest","pipeline":"p"}}"#.into(),
            400,
            "pipeline",
        ),
        (
            "/_reindex",
            r#"{"dest":{"index":"dest"}}"#.into(),
            400,
            "index",
        ),
        ("/_reindex", "[]".into(), 400, "object"),
        ("/_reindex?slices=2", reindex_body(""), 400, "not supported"),
    ];
    for (target, body, status, word) in cases {
        let reply = call(&r, "POST", target, Some(&body));
        assert_eq!(reply.status, status, "{target} {body}: {}", reply.text());
        assert!(
            reply.text().contains(word),
            "{target} {body}: {}",
            reply.text()
        );
    }
    let hits = vec![put(&r, "a")];
    *r.matches.hits.lock().unwrap() = hits;
    for script in ["ctx._source.n = ", "ctx._source.n = ctx.nope.x"] {
        let body =
            with_script(&reindex_body("")) + &format!(r#","script":{{"source":"{script}"}}}}"#);
        let reply = call(&r, "POST", "/_reindex", Some(&body));
        assert_eq!(reply.status, 400, "{script}: {}", reply.text());
        assert!(
            reply.text().contains("script_exception"),
            "{}",
            reply.text()
        );
    }
    oss_reader::painless_switch::set_native_compile(false);
}
