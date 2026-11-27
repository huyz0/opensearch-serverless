//! docs/specs/stored-script-ids: a script named by `id` runs the stored
//! source at every search site. A process of its own: the resolver and the
//! node's switch are process-wide.

// ARITH: test sizes and counts are small constants; an overflow panics
// the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use oss_reader::stored_scripts::{ResolveError, Stored, StoredScripts, install};
use support::{Node, request, start};

/// Stored scripts by id, counting how often each search resolves one.
struct Fixed {
    scripts: HashMap<&'static str, (&'static str, &'static str)>,
    calls: AtomicUsize,
}

impl StoredScripts for Fixed {
    fn resolve(&self, id: &str) -> Result<Stored, ResolveError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.scripts
            .get(id)
            .map(|(lang, source)| Stored {
                lang: (*lang).to_owned(),
                source: (*source).to_owned(),
            })
            .ok_or(ResolveError::Missing)
    }
}

/// The resolver is process-wide, so one test at a time holds it.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn scripts() -> (Arc<Fixed>, MutexGuard<'static, ()>) {
    let held = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
    let fixed = Arc::new(Fixed {
        scripts: HashMap::from([
            ("score", ("painless", "doc['year'].value * params.factor")),
            ("years", ("painless", "doc['year'].value")),
            ("late", ("painless", "doc['year'].value >= params.from")),
            (
                "mapper",
                ("painless", "state.n = (state.n == null ? 0 : state.n) + 1"),
            ),
            ("template", ("mustache", "{}")),
        ]),
        calls: AtomicUsize::new(0),
    });
    install(Arc::clone(&fixed) as Arc<dyn StoredScripts>);
    oss_reader::painless_switch::set_native_compile(true);
    (fixed, held)
}

fn post(node: &Node, body: &str) -> (u16, String) {
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

// AC-1, AC-7
#[test]
fn script_score_by_id_scores_like_the_same_script_inline_and_resolves_once() {
    let (fixed, _held) = scripts();
    let node = start();
    let wrap = |script: &str| {
        format!(
            r#"{{"size":10,"query":{{"function_score":{{"query":{{"match_all":{{}}}},"boost_mode":"replace","script_score":{{"script":{script}}}}}}}}}"#
        )
    };
    let (status, inline) = post(
        &node,
        &wrap(r#"{"source":"doc['year'].value * params.factor","params":{"factor":2}}"#),
    );
    assert_eq!(status, 200, "{inline}");
    let before = fixed.calls.load(Ordering::SeqCst);
    let (status, stored) = post(&node, &wrap(r#"{"id":"score","params":{"factor":2}}"#));
    assert_eq!(status, 200, "{stored}");
    assert_eq!(
        fixed.calls.load(Ordering::SeqCst) - before,
        1,
        "one resolve a search, whatever its shards"
    );
    let scores = |text: &str| {
        let mut out: Vec<String> = text
            .split(r#""_score":"#)
            .skip(1)
            .map(|s| s.split([',', '}']).next().unwrap_or("").to_owned())
            .collect();
        out.sort();
        out
    };
    assert!(!scores(&stored).is_empty(), "{stored}");
    assert_eq!(scores(&stored), scores(&inline));
}

// AC-2
#[test]
fn script_fields_sort_query_and_scripted_metric_run_a_stored_script() {
    let (_fixed, _held) = scripts();
    let node = start();
    let (status, text) = post(
        &node,
        r#"{"size":10,"query":{"match_all":{}},"script_fields":{"y":{"script":{"id":"years"}}}}"#,
    );
    assert_eq!(status, 200, "{text}");
    assert!(text.contains(r#""y":[2010.0]"#), "{text}");
    let (status, text) = post(
        &node,
        r#"{"size":10,"query":{"match_all":{}},"sort":[{"_script":{"type":"number","order":"desc","script":{"id":"years"}}}]}"#,
    );
    assert_eq!(status, 200, "{text}");
    let first = text.find(r#""sort":["#).expect("sort") + 8;
    assert!(text[first..].starts_with("2010"), "{text}");
    let (status, text) = post(
        &node,
        r#"{"size":10,"query":{"script":{"script":{"id":"late","params":{"from":2002}}}}}"#,
    );
    assert_eq!(status, 200, "{text}");
    assert!(text.contains(r#""value":3"#), "{text}");
    let (status, text) = post(
        &node,
        r#"{"size":0,"query":{"match_all":{}},"aggs":{"sm":{"scripted_metric":{"map_script":{"id":"mapper"},"combine_script":"return state.n","reduce_script":"long t = 0; for (s in states) { t = t + s; } return t;"}}}}"#,
    );
    assert_eq!(status, 200, "{text}");
    assert!(text.contains(r#""sm":{"value":5}"#), "{text}");
}

// AC-4
#[test]
fn a_missing_mixed_or_foreign_stored_script_is_refused() {
    let (_fixed, _held) = scripts();
    let node = start();
    let body = |script: &str| format!(r#"{{"size":1,"query":{{"script":{{"script":{script}}}}}}}"#);
    for (script, status, word) in [
        (r#"{"id":"nope"}"#, 404, "unable to find script [nope]"),
        (
            r#"{"id":"late","source":"true"}"#,
            400,
            "takes no [source] or [lang]",
        ),
        (
            r#"{"id":"late","lang":"painless"}"#,
            400,
            "takes no [source] or [lang]",
        ),
        (r#"{"id":"template"}"#, 400, "mustache"),
        (r#"{"id":5}"#, 400, "must be a string"),
    ] {
        let (got, text) = post(&node, &body(script));
        assert_eq!(got, status, "{script}: {text}");
        assert!(text.contains(word), "{script}: {text}");
    }
}
