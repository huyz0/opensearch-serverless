//! docs/specs/scripted-metric AC-4: a `scripted_metric` over an index of two
//! shards, each shard folding its own documents and the reduce script
//! running once over both values. A process of its own, because the node's
//! switch is process-wide.

// ARITH: test sizes and counts are small constants; an overflow panics
// the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

#[path = "support/mod.rs"]
mod support;

use support::{put_shard_copy, put_variant, request, start};

#[test]
fn a_scripted_metric_reduces_the_values_of_two_shards() {
    oss_reader::painless_switch::set_native_compile(true);
    let node = start();
    put_variant(&node.store, "compat-term", "two", |j| {
        j.replace(r#""number_of_shards":1"#, r#""number_of_shards":2"#)
    });
    put_shard_copy(&node.store, "compat-term", "compat-term-uuid", 1);
    let search = |reduce: &str| {
        let body = format!(
            r#"{{"size":0,"aggs":{{"sm":{{"scripted_metric":{{"init_script":"state.sum = 0L","map_script":"state.sum = state.sum + doc['year'].value","combine_script":"return state.sum","reduce_script":"{reduce}"}}}}}}}}"#
        );
        request(node.addr(), "POST", "/two/_search", Some(body.as_bytes()))
    };
    // Each shard holds years 2010, 2002, 2002, 2001, 1999: 10,014.
    let r = search("long t = 0; for (s in states) { t = t + s; } return [t, states.size()];");
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(
        r.text().contains(r#""sm":{"value":[20028,2]}"#),
        "{}",
        r.text()
    );
    // A reduce that throws is a 400 naming the aggregation.
    let r = search("return states[0].nope.x;");
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(
        r.text().contains("script_exception") && r.text().contains("[sm]"),
        "{}",
        r.text()
    );
}
