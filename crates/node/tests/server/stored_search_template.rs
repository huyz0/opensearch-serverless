//! docs/specs/stored-search-templates AC-7 and AC-9: a template named by id is
//! read from the stored-script set on the one assembled node, and costs what
//! the budget holds. `_render/template` needs no index, so the cost measured is
//! the read of the set alone.

use crate::assemble::{call, config, start_with};

use oss_blob::{BlobStore, S3Op};
use oss_testkit::budget;

const SCRIPT: &str =
    r#"{"script":{"lang":"mustache","source":"{\"query\":{\"match_all\":{}},\"size\":{{n}}}"}}"#;

// AC-7, AC-9
#[test]
fn a_stored_template_follows_its_script_and_costs_one_marker_get_warm() {
    let (r, store) = start_with(config(), 256 << 20);
    let total = || -> u64 { S3Op::ALL.iter().map(|op| store.meter().count(*op)).sum() };
    let spent = |target: &str, body: &str| {
        let before = total();
        let reply = call(&r, "POST", target, Some(body));
        assert_eq!(reply.status, 200, "{}", reply.text());
        (total() - before, reply.text())
    };
    assert_eq!(call(&r, "PUT", "/_scripts/t", Some(SCRIPT)).status, 200);
    let params = r#"{"params":{"n":1}}"#;
    let inline = r#"{"source":"{\"query\":{\"match_all\":{}},\"size\":{{n}}}","params":{"n":1}}"#;
    // The first named request reads the set; the next finds it unchanged.
    spent("/_render/template/t", params);
    let (named, answer) = spent("/_render/template/t", params);
    assert_eq!(
        answer,
        r#"{"template_output":{"query":{"match_all":{}},"size":1}}"#
    );
    let (plain, _) = spent("/_render/template", inline);
    budget::check(
        "search_template.stored.requests",
        named - plain,
        "a stored template against an inline one",
    );
    budget::check(
        "search_template.inline.requests",
        plain,
        "an inline template",
    );
    assert_eq!(store.meter().count(S3Op::List), 0);

    // A script put after the request is read by the next one.
    let other = SCRIPT.replace("{{n}}", "7");
    assert_eq!(call(&r, "PUT", "/_scripts/t", Some(&other)).status, 200);
    let (_, answer) = spent("/_render/template/t", "{}");
    assert!(answer.contains(r#""size":7"#), "{answer}");
    // And a deleted one is missing.
    assert_eq!(call(&r, "DELETE", "/_scripts/t", None).status, 200);
    let gone = call(&r, "POST", "/_render/template/t", Some(params));
    assert_eq!(gone.status, 404, "{}", gone.text());
}
