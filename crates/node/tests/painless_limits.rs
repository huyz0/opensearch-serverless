//! docs/specs/painless-limits: what a Painless script may allocate and how
//! large it may be. A process of its own, because the switch and the limits
//! are process-wide.

// ARITH: test sizes and counts are small constants; an overflow panics
// the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

#[path = "support/mod.rs"]
mod support;

use support::{Node, request, start};

fn script(node: &Node, source: &str) -> (u16, String) {
    let body = format!(
        r#"{{"size":3,"query":{{"function_score":{{"query":{{"match_all":{{}}}},"boost_mode":"replace","script_score":{{"script":{{"source":"{source}"}}}}}}}}}}"#
    );
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

fn rss_kib() -> u64 {
    let s = std::fs::read_to_string("/proc/self/status").expect("status");
    let line = s.lines().find(|l| l.starts_with("VmRSS:")).expect("VmRSS");
    line.split_whitespace()
        .nth(1)
        .expect("kib")
        .parse()
        .expect("n")
}

const DOUBLED: &str =
    "String s = 'xxxxxxxx'; for (int i = 0; i < 18; i++) { s += s; } return s.length();";
const SMALL: &str =
    "String s = 'xxxxxxxx'; for (int i = 0; i < 10; i++) { s += s; } return s.length();";
const MANY: &str = "int n = 0; for (int i = 0; i < 100; i++) { String s = 'x'; for (int j = 0; j < 16; j++) { s += s; } n += s.length(); } return n;";

// One test: the switch and the limits are process-wide.
#[test]
fn a_script_past_its_limits_is_refused_and_the_node_goes_on() {
    // AC-5: off, Painless is refused and nothing is limited.
    oss_reader::painless_switch::set_native_compile(false);
    let node = start();
    let (status, _) = script(&node, DOUBLED);
    assert_eq!(status, 400);
    drop(node);

    oss_reader::painless_switch::set_native_compile(true);
    let node = start();
    // AC-1
    let (status, body) = script(&node, DOUBLED);
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("script_exception"), "{body}");
    assert!(body.contains("allocated more than"), "{body}");
    let (status, body) = script(&node, SMALL);
    assert_eq!(status, 200, "{body}");
    // AC-2
    let (status, body) = script(&node, MANY);
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("allocated more than"), "{body}");
    // AC-3
    let big = format!("int a = 0; {} return a;", "a += 1;".repeat(9_000));
    let (status, body) = script(&node, &big);
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("over the limit of 20000"), "{body}");
    // AC-4
    // The allocator's high-water mark settles in the first few failures.
    for _ in 0..20 {
        script(&node, DOUBLED);
    }
    let before = rss_kib();
    for _ in 0..200 {
        script(&node, DOUBLED);
    }
    let after = rss_kib();
    println!("rss {before} to {after} KiB");
    assert!(after * 10 <= before * 11, "rss {before} to {after} KiB");
    assert_eq!(script(&node, SMALL).0, 200);
}
