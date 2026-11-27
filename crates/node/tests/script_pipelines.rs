//! docs/specs/pipeline-aggs-script: `bucket_script` and `bucket_selector`
//! over a REST search, on a node that compiles Painless.

#[path = "support/mod.rs"]
mod support;

use support::{request, start};

fn search(addr: std::net::SocketAddr, aggs: &str) -> (u16, String) {
    let body = format!(
        r#"{{"size":0,"aggs":{{"h":{{"histogram":{{"field":"year","interval":10}},"aggs":{{"s":{{"sum":{{"field":"year"}}}}{aggs}}}}}}}}}"#
    );
    let r = request(
        addr,
        "POST",
        "/compat-paging-sort/_search?typed_keys=false",
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

/// AC-8: a `bucket_script` adds a value to each bucket and a
/// `bucket_selector` drops the buckets its script rejects.
#[test]
fn the_scripts_of_two_pipelines_run_over_the_buckets() {
    oss_reader::painless_switch::set_native_compile(true);
    let node = start();
    let (status, text) = search(
        node.addr(),
        r#","half":{"bucket_script":{"buckets_path":{"t":"s","n":"_count"},"script":"return params.t / params.n"}}"#,
    );
    assert_eq!(status, 200, "{text}");
    // Every year in the data is in the 1990s or 2000s: the mean is the sum over the count.
    assert!(text.contains(r#""half":{"value":"#), "{text}");
    let (status, none) = search(
        node.addr(),
        r#","keep":{"bucket_selector":{"buckets_path":{"n":"_count"},"script":"return params.n > 1000000"}}"#,
    );
    assert_eq!(status, 200, "{none}");
    assert!(none.contains(r#""buckets":[]"#), "{none}");
    let (status, all) = search(
        node.addr(),
        r#","keep":{"bucket_selector":{"buckets_path":{"n":"_count"},"script":"return params.n > 0"}}"#,
    );
    assert_eq!(status, 200, "{all}");
    assert!(all.contains(r#""doc_count""#), "{all}");
    let (status, bad) = search(
        node.addr(),
        r#","half":{"bucket_script":{"buckets_path":{"t":"s"},"script":"return params.t","gap_policy":"nope"}}"#,
    );
    assert_eq!(status, 400, "{bad}");
    assert!(bad.contains("Invalid gap policy"), "{bad}");
    // A script that throws, or returns the wrong type, is a 400 naming the pipeline.
    for (ty, script) in [
        ("bucket_script", "throw new IllegalStateException('no')"),
        ("bucket_script", "return 'text'"),
        ("bucket_selector", "return 1"),
    ] {
        let (status, text) = search(
            node.addr(),
            &format!(r#","half":{{"{ty}":{{"buckets_path":{{"t":"s"}},"script":"{script}"}}}}"#),
        );
        assert_eq!(status, 400, "{text}");
        assert!(
            text.contains("script_exception") && text.contains("[half]"),
            "{text}"
        );
    }
}

/// docs/specs/pipeline-aggs-moving-fn AC-8: a `moving_fn` adds a windowed
/// value to each bucket, and a script that throws is a 400.
#[test]
fn a_moving_fn_runs_over_the_buckets() {
    oss_reader::painless_switch::set_native_compile(true);
    let node = start();
    let moving = |script: &str| {
        search(
            node.addr(),
            &format!(
                r#","m":{{"moving_fn":{{"buckets_path":"s","window":2,"shift":1,"script":"{script}"}}}}"#
            ),
        )
    };
    let (status, text) = moving("return MovingFunctions.max(values)");
    assert_eq!(status, 200, "{text}");
    assert!(text.contains(r#""m":{"value":"#), "{text}");
    let (status, text) = moving("return values.length");
    assert_eq!(status, 200, "{text}");
    assert!(text.contains(r#""m":{"value":1.0}"#), "{text}");
    let (status, text) = moving("throw new IllegalStateException('no')");
    assert_eq!(status, 400, "{text}");
    assert!(
        text.contains("script_exception") && text.contains("[m]"),
        "{text}"
    );
}
