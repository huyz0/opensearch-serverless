//! docs/specs/painless-default-on: what a Painless script costs a search, and
//! whether its bounds hold. Ignored; run by hand, a process of its own
//! because the node's switch is process-wide.

// ARITH: test sizes and counts are small constants; an overflow panics
// the test, which fails it as it should.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

#[path = "support/mod.rs"]
mod support;

use std::time::Instant;

use oss_mem::{MemoryBudget, Pool};
use support::{Node, budget_config, counts, request, start_with, store};

fn search(node: &Node, function: &str) -> (u16, String) {
    let body = format!(
        r#"{{"size":10,"query":{{"function_score":{{"query":{{"match_all":{{}}}},"boost_mode":"replace",{function}}}}}}}"#
    );
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

fn plain(node: &Node) -> (u16, String) {
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(br#"{"size":10,"query":{"match_all":{}}}"#),
    );
    (r.status, r.text())
}

const EXPRESSION: &str = r#""script_score":{"script":{"lang":"expression","source":"doc['year'].value * factor","params":{"factor":2}}}"#;
const WEIGHT: &str = r#""weight":2"#;
const SCRIPT: &str = r#""script_score":{"script":{"source":"doc['year'].value * params.factor","params":{"factor":2}}}"#;

fn delta(node: &Node, f: impl FnOnce()) -> Vec<u64> {
    let before = counts(&node.store);
    f();
    counts(&node.store)
        .iter()
        .zip(before.iter())
        .map(|(a, b)| a - b)
        .collect()
}

fn percentile(sorted: &[u128], p: usize) -> u128 {
    sorted[(sorted.len() * p / 100).min(sorted.len() - 1)]
}

fn timed(n: usize, mut f: impl FnMut()) -> (u128, u128) {
    let mut micros: Vec<u128> = (0..n)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed().as_micros()
        })
        .collect();
    micros.sort_unstable();
    (percentile(&micros, 50), percentile(&micros, 99))
}

fn rss_kib() -> u64 {
    let s = std::fs::read_to_string("/proc/self/status").expect("status");
    let line = s.lines().find(|l| l.starts_with("VmRSS:")).expect("VmRSS");
    line.split_whitespace()
        .nth(1)
        .expect("kib")
        .parse()
        .expect("number")
}

fn script(node: &Node, source: &str) -> (u16, String) {
    search(
        node,
        &format!(r#""script_score":{{"script":{{"source":"{source}"}}}}"#),
    )
}

#[test]
#[ignore = "measurement; run by hand"]
fn what_a_painless_script_costs_and_where_its_bounds_hold() {
    oss_reader::painless_switch::set_native_compile(true);
    let budget = MemoryBudget::new(
        &budget_config()
            .with_pool(Pool::QueryExecution, 16 << 20)
            .expect("fits"),
    );
    let node = start_with(store(), &budget);
    println!(
        "cache bytes reserved: {}",
        budget.in_use(Pool::QueryExecution)
    );
    let mut failed: Vec<String> = Vec::new();
    // Warm both paths.
    for _ in 0..50 {
        assert_eq!(plain(&node).0, 200);
        assert_eq!(search(&node, SCRIPT).0, 200);
    }

    // AC-1
    let cold_free = delta(&node, || {
        plain(&node);
    });
    let with_script = delta(&node, || {
        search(&node, SCRIPT);
    });
    println!("AC-1 requests (no script): {cold_free:?}; (script): {with_script:?}");
    if cold_free != with_script {
        failed.push("AC-1: request counts differ".into());
    }

    // AC-2 and AC-5: alternate rounds so drift hits every path. The baseline
    // is the same function_score with a weight and no script, then with an
    // expression, so the script's own cost is not the query shape's.
    let mut worst = 0.0_f64;
    for _ in 0..3 {
        let a = timed(2000, || {
            search(&node, WEIGHT);
        });
        let e = timed(2000, || {
            search(&node, EXPRESSION);
        });
        let b = timed(2000, || {
            search(&node, SCRIPT);
        });
        println!(
            "AC-2 round (us): weight p50 {} p99 {}; expression p50 {} p99 {}; painless p50 {} p99 {}",
            a.0, a.1, e.0, e.1, b.0, b.1
        );
        worst = worst
            .max(b.0 as f64 / a.0 as f64 - 1.0)
            .max(b.1 as f64 / a.1 as f64 - 1.0);
    }
    println!(
        "AC-2 worst excess over the weight baseline: {:.0}%",
        worst * 100.0
    );
    if worst > 0.05 {
        failed.push(format!(
            "AC-2: painless costs {:.0}% over no script",
            worst * 100.0
        ));
    }
    let first = Instant::now();
    search(
        &node,
        r#""script_score":{"script":{"source":"double a = 1; for (int i = 0; i < 50; i++) { a += doc['year'].value * i; } return a;"}}"#,
    );
    println!(
        "AC-5 first request of a new source: {} us",
        first.elapsed().as_micros()
    );
    let (p50, _) = timed(200, || {
        search(
            &node,
            r#""script_score":{"script":{"source":"double a = 1; for (int i = 0; i < 50; i++) { a += doc['year'].value * i; } return a;"}}"#,
        );
    });
    println!("AC-5 repeats of that source: p50 {p50} us");

    // AC-3
    let before = rss_kib();
    let big = "String s = 'xxxxxxxx'; for (int i = 0; i < 18; i++) { s += s; } return s.length();";
    let (status, body) = script(&node, big);
    println!(
        "AC-3 2 MiB string: status {status} {}",
        &body[..body.len().min(160)]
    );
    let string_bound_held = status == 400 && body.contains("script_exception");
    for _ in 0..200 {
        script(&node, big);
    }
    let after = rss_kib();
    println!("AC-3 rss before {before} KiB after {after} KiB");
    if !string_bound_held {
        failed.push("AC-3: a 2 MiB string was served, no memory bound".into());
    }
    if after * 10 > before * 11 {
        failed.push("AC-3: rss grew past 10%".into());
    }

    // AC-4
    let t = Instant::now();
    let (status, body) = script(&node, "long n = 0; while (true) { n++; } return n;");
    println!(
        "AC-4 runaway loop: status {status} in {} ms: {}",
        t.elapsed().as_millis(),
        &body[..body.len().min(160)]
    );
    println!("AC-4 next search: {}", plain(&node).0);
    if status != 400 || t.elapsed().as_secs() >= 5 {
        failed.push("AC-4: the runaway loop was not stopped in time".into());
    }
    assert!(failed.is_empty(), "criteria not met: {failed:?}");
}
