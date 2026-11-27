//! docs/specs/nested-write-soak: the write side of the nested switch's
//! default flip. 10,000 documents of two or three comments go to a nested
//! index and to a plain one through one node, in alternating bulks so noise
//! is shared; the acknowledgement latency of each bulk and the store
//! requests of each index are compared. A measurement, run by hand:
//! `cargo test -p oss-node --features reader,writer,coordinator --test
//! nested_write_soak -- --ignored --nocapture`.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: test sizes are small constants; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects, clippy::disallowed_methods)]

mod suite_rig;

use std::time::{Duration, Instant};

use oss_blob::S3Op;
use suite_rig::{Cluster, wait_for};

const DOCS: usize = 10_000;
const BULK: usize = 50;
const PROPS: &str =
    r#""properties":{"who":{"type":"keyword"},"n":{"type":"long"},"note":{"type":"text"}}"#;

fn mapping(nested: bool) -> String {
    let kind = if nested { r#""type":"nested","# } else { "" };
    format!(
        r#"{{"settings":{{"index":{{"number_of_shards":1}}}},"mappings":{{"properties":{{"title":{{"type":"keyword"}},"comments":{{{kind}{PROPS}}}}}}}}}"#
    )
}

/// The ndjson bulk of documents `from..from + BULK` for `index`.
fn bulk_body(index: &str, from: usize) -> String {
    let mut body = String::new();
    for i in from..from + BULK {
        body.push_str(&format!(
            "{{\"index\":{{\"_index\":\"{index}\",\"_id\":\"d{i}\"}}}}\n"
        ));
        let comments: Vec<String> = (0..2 + i % 2)
            .map(|c| {
                format!(
                    r#"{{"who":"user{}","n":{},"note":"comment {c} of document {i} about alpha beta"}}"#,
                    (i + c) % 50,
                    i * 3 + c
                )
            })
            .collect();
        body.push_str(&format!(
            "{{\"title\":\"t{i}\",\"comments\":[{}]}}\n",
            comments.join(",")
        ));
    }
    body
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let rank = ((sorted.len() as f64) * p).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

struct Run {
    latencies: Vec<Duration>,
    puts: u64,
    gets: u64,
}

#[test]
#[ignore = "a measurement, run by hand"]
fn nested_writes_cost_the_same_acknowledgement_and_requests_as_plain_ones() {
    let mut cluster = Cluster::new(3_000, 4, false).expect("emulated");
    cluster.set("OSS_NESTED", "true");
    cluster.add("n1");
    wait_for("the node sees itself", 30, || cluster.nodes_seen(0) == 1);
    for (name, nested) in [("flat", false), ("nest", true)] {
        let made = cluster.call(0, "PUT", &format!("/{name}"), Some(&mapping(nested)));
        assert_eq!(made.status, 200, "{}", made.text());
    }
    let mut runs = ["flat", "nest"].map(|n| (n, Vec::<std::time::Duration>::new(), (0u64, 0u64)));
    let started = Instant::now();
    for (round, from) in (0..DOCS).step_by(BULK).enumerate() {
        // Alternate which index goes first so neither always runs warmer.
        let order = if round % 2 == 0 { [0, 1] } else { [1, 0] };
        for slot in order {
            let name = runs[slot].0;
            let (puts, gets) = (cluster.sent(S3Op::Put), cluster.sent(S3Op::Get));
            let body = bulk_body(name, from);
            let began = Instant::now();
            let r = cluster.call(0, "POST", "/_bulk", Some(&body));
            let took = began.elapsed();
            assert!(r.text().contains("\"errors\":false"), "{}", r.text());
            runs[slot].1.push(took);
            runs[slot].2.0 += cluster.sent(S3Op::Put) - puts;
            runs[slot].2.1 += cluster.sent(S3Op::Get) - gets;
        }
    }
    println!("{DOCS} documents a index in {:?}", started.elapsed());
    // Everything published and visible, parents counted once.
    for (name, ..) in &runs {
        let r = cluster.call(0, "POST", &format!("/{name}/_refresh"), None);
        assert_eq!(r.status, 200, "{}", r.text());
        wait_for("all documents are counted", 60, || {
            cluster
                .call(0, "GET", &format!("/{name}/_count"), None)
                .text()
                .contains(&format!("\"count\":{DOCS}"))
        });
    }
    let result: Vec<Run> = runs
        .into_iter()
        .map(|(_, mut l, (puts, gets))| {
            l.sort();
            Run {
                latencies: l,
                puts,
                gets,
            }
        })
        .collect();
    for (name, run) in ["flat", "nest"].iter().zip(&result) {
        println!(
            "{name}: bulks {}, p50 {:?}, p99 {:?}, max {:?}; store PUT {} GET {} over the writes",
            run.latencies.len(),
            percentile(&run.latencies, 0.5),
            percentile(&run.latencies, 0.99),
            run.latencies.last().expect("some"),
            run.puts,
            run.gets,
        );
    }
    let (flat, nest) = (&result[0], &result[1]);
    let ratio = |p| {
        percentile(&nest.latencies, p).as_secs_f64() / percentile(&flat.latencies, p).as_secs_f64()
    };
    println!(
        "nested over plain: p50 x{:.3}, p99 x{:.3}; PUT {} vs {}, GET {} vs {}",
        ratio(0.5),
        ratio(0.99),
        nest.puts,
        flat.puts,
        nest.gets,
        flat.gets
    );
    // The request count does not depend on children: a nested write makes the
    // requests a plain one makes, within one blob in ten.
    assert!(
        nest.puts <= flat.puts + flat.puts / 10 + 4,
        "nested PUT {} against plain {}",
        nest.puts,
        flat.puts
    );
}
