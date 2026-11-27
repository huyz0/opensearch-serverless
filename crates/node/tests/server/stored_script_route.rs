//! docs/specs/stored-script-ids AC-3, AC-4: a script named by `id` in
//! update-by-query and reindex, resolved through the service's own resolver.

// ARITH: offsets into a reply are small; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects)]

use std::collections::HashMap;
use std::sync::Arc;

use oss_coordinator::docs::by_query::Hit;
use oss_reader::stored_scripts::{ResolveError, Stored, StoredScripts};

use super::by_query_rig::{Rig, call, put, rig, rig_scripts, source_of};
use crate::support;

struct Fixed(HashMap<&'static str, (&'static str, &'static str)>);

impl StoredScripts for Fixed {
    fn resolve(&self, id: &str) -> Result<Stored, ResolveError> {
        self.0
            .get(id)
            .map(|(lang, source)| Stored {
                lang: (*lang).to_owned(),
                source: (*source).to_owned(),
            })
            .ok_or(ResolveError::Missing)
    }
}

fn scripted() -> Rig {
    let rig = rig_scripts(Arc::new(Fixed(HashMap::from([
        ("bump", ("painless", "ctx._source.n += params.by")),
        ("template", ("mustache", "{}")),
    ]))));
    let hits: Vec<Hit> = ["a", "b"].iter().map(|id| put(&rig, id)).collect();
    *rig.matches.hits.lock().unwrap() = hits;
    rig
}

const MATCH: &str = r#""query":{"match":{"n":1}}"#;

// docs/specs/stored-script-ids AC-3
#[test]
fn update_by_query_and_reindex_run_a_stored_script() {
    let _switch = support::PAINLESS.lock().unwrap_or_else(|e| e.into_inner());
    oss_reader::painless_switch::set_native_compile(true);
    let r = scripted();
    let body = format!(r#"{{{MATCH},"script":{{"id":"bump","params":{{"by":4}}}}}}"#);
    let reply = call(&r, "POST", "/docs/_update_by_query", Some(&body));
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert!(reply.text().contains(r#""updated":2"#), "{}", reply.text());
    assert!(source_of(&r, "a").contains(r#""n":5"#));
    let copy = format!(
        r#"{{"source":{{"index":"docs",{MATCH}}},"dest":{{"index":"dest"}},"script":{{"id":"bump","params":{{"by":10}}}}}}"#
    );
    let reply = call(&r, "POST", "/_reindex", Some(&copy));
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert!(reply.text().contains(r#""created":2"#), "{}", reply.text());
    let copied = call(&r, "GET", "/dest/_doc/a", None).text();
    assert!(copied.contains(r#""n":15"#), "{copied}");
    oss_reader::painless_switch::set_native_compile(false);
}

// docs/specs/stored-script-ids AC-4
#[test]
fn a_missing_mixed_or_foreign_id_and_a_node_with_no_resolver_are_refused() {
    let _switch = support::PAINLESS.lock().unwrap_or_else(|e| e.into_inner());
    oss_reader::painless_switch::set_native_compile(true);
    let r = scripted();
    for (script, status, word) in [
        (r#"{"id":"nope"}"#, 404, "unable to find script [nope]"),
        (
            r#"{"id":"bump","source":"1"}"#,
            400,
            "takes no [source] or [lang]",
        ),
        (
            r#"{"id":"bump","lang":"painless"}"#,
            400,
            "takes no [source] or [lang]",
        ),
        (r#"{"id":"template"}"#, 400, "mustache"),
        (r#"{"id":7}"#, 400, "must be a string"),
    ] {
        let body = format!(r#"{{{MATCH},"script":{script}}}"#);
        let reply = call(&r, "POST", "/docs/_update_by_query", Some(&body));
        assert_eq!(reply.status, status, "{script}: {}", reply.text());
        assert!(reply.text().contains(word), "{script}: {}", reply.text());
    }
    let bare = rig();
    let body = format!(r#"{{{MATCH},"script":{{"id":"bump"}}}}"#);
    let reply = call(&bare, "POST", "/docs/_update_by_query", Some(&body));
    assert_eq!(reply.status, 400, "{}", reply.text());
    assert!(
        reply.text().contains("not served on this node"),
        "{}",
        reply.text()
    );
    oss_reader::painless_switch::set_native_compile(false);
}

// docs/specs/stored-script-ids AC-8: the node that takes a put or delete
// sees it at its next resolve, however long the TTL.
#[test]
fn the_node_that_took_a_put_or_delete_sees_it_at_once() {
    use oss_blob::{BlobStore, Emulator};
    use oss_mem::MemoryBudget;
    use oss_node::script_route::{ScriptService, routes};
    use oss_node::stored_script_source::ScriptSource;
    use oss_rest::{Router, Server};

    let store: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 28, 5));
    let budget = MemoryBudget::new(&support::budget_config());
    let source =
        ScriptSource::open(&store, "", &budget, 3_600_000, Box::new(|| 1_000)).expect("opens");
    let service = ScriptService::new(&store, "", 1 << 20)
        .expect("service")
        .with_source(Arc::clone(&source));
    let router = routes(Router::new(), &Arc::new(service)).expect("routes");
    let server = Server::start(
        std::net::TcpListener::bind("127.0.0.1:0").expect("binds"),
        support::server_config(),
        router,
        &budget,
    )
    .expect("starts");
    let send = |method: &str, body: Option<&str>| {
        support::request(
            server.local_addr(),
            method,
            "/_scripts/x",
            body.map(str::as_bytes),
        )
    };
    assert_eq!(source.resolve("x"), Err(ResolveError::Missing));
    let put = |source_text: &str| {
        let body = format!(r#"{{"script":{{"lang":"painless","source":"{source_text}"}}}}"#);
        assert_eq!(send("PUT", Some(&body)).status, 200);
    };
    put("1");
    assert_eq!(source.resolve("x").expect("seen").source, "1");
    put("2");
    assert_eq!(source.resolve("x").expect("changed").source, "2");
    assert_eq!(send("DELETE", None).status, 200);
    assert_eq!(source.resolve("x"), Err(ResolveError::Missing));
}
