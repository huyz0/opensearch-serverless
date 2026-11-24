//! docs/specs/search-slots: the measurements ADR 0047 asks for before the
//! gate's default moves: search latency with the gate on and off below its
//! slots, and a noisy tenant against a quiet one with the gate on and off.
//! Evidence run by hand with `--ignored --nocapture`; a time is not a
//! predicate, so the only assertions are that every search answered 200.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

#[path = "support/mod.rs"]
mod support;

// ARITH: test arithmetic on counts and durations; an overflow panics the test.
#[allow(
    clippy::arithmetic_side_effects,
    clippy::cast_precision_loss,
    clippy::disallowed_methods
)]
mod run {
    use super::support;

    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use oss_node::boot::{Node, NodeConfig, start};
    use oss_testkit::TempDir;

    const DOCS: usize = 3_000;
    const SLOTS: &str = "8";
    const ROUNDS: usize = 3;
    const RUN: Duration = Duration::from_secs(6);
    const QUERY: &str = r#"{"size":20,"query":{"bool":{"should":[{"match":{"body":"alpha"}},{"match":{"body":"beta gamma"}}]}},"aggs":{"k":{"terms":{"field":"k.keyword"}}}}"#;

    fn boot(root: &str, slots: &str) -> Node {
        let lookup = |key: &str| match key {
            "OSS_NODE_ID" => Some("n1".to_owned()),
            "OSS_STORE" => Some("fs".to_owned()),
            "OSS_FS_ROOT" => Some(root.to_owned()),
            "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => Some("127.0.0.1:0".to_owned()),
            "OSS_STORE_USAGE" => Some("true".to_owned()),
            "OSS_SEARCH_SLOTS" => Some(slots.to_owned()),
            _ => None,
        };
        start(&NodeConfig::from_lookup(lookup).expect("valid")).expect("starts")
    }

    fn fill(node: &Node, index: &str) {
        let addr = node.addr();
        assert_eq!(
            support::request(addr, "PUT", &format!("/{index}"), None).status,
            200
        );
        let mut body = String::new();
        for i in 0..DOCS {
            body.push_str(&format!(
                "{{\"index\":{{\"_index\":\"{index}\",\"_id\":\"{i}\"}}}}\n"
            ));
            body.push_str(&format!(
                "{{\"body\":\"alpha beta gamma delta word{} word{}\",\"k\":\"k{}\"}}\n",
                i % 97,
                i % 13,
                i % 26
            ));
        }
        let reply = support::request(addr, "POST", "/_bulk?refresh=true", Some(body.as_bytes()));
        assert_eq!(
            reply.status,
            200,
            "{}",
            &reply.text()[..reply.text().len().min(300)]
        );
    }

    /// `clients` closed-loop searchers of `index` for `RUN`; every latency in microseconds.
    fn drive(node: &Node, index: &str, clients: usize, stop: &AtomicBool) -> Vec<u64> {
        let addr = node.addr();
        let target = format!("/{index}/_search");
        std::thread::scope(|s| {
            let handles: Vec<_> = (0..clients)
                .map(|_| {
                    let target = target.clone();
                    s.spawn(move || {
                        let mut seen = Vec::new();
                        while !stop.load(Ordering::Relaxed) {
                            let at = Instant::now();
                            let reply =
                                support::request(addr, "POST", &target, Some(QUERY.as_bytes()));
                            assert_eq!(
                                reply.status,
                                200,
                                "{}",
                                &reply.text()[..reply.text().len().min(300)]
                            );
                            seen.push(u64::try_from(at.elapsed().as_micros()).unwrap_or(u64::MAX));
                        }
                        seen
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().expect("client"))
                .collect()
        })
    }

    fn pct(sorted: &[u64], p: f64) -> f64 {
        if sorted.is_empty() {
            return 0.0;
        }
        let at = ((sorted.len() as f64 - 1.0) * p).round() as usize;
        sorted[at] as f64 / 1000.0
    }

    fn line(what: &str, mut seen: Vec<u64>) {
        seen.sort_unstable();
        println!(
            "{what}: n={} p50={:.1}ms p99={:.1}ms max={:.1}ms",
            seen.len(),
            pct(&seen, 0.5),
            pct(&seen, 0.99),
            pct(&seen, 1.0)
        );
    }

    /// Runs `quiet` and `noisy` searchers together for `RUN`; the latencies of each.
    fn together(node: &Node, quiet: usize, noisy: usize) -> (Vec<u64>, Vec<u64>) {
        let stop = AtomicBool::new(false);
        std::thread::scope(|s| {
            let q = s.spawn(|| drive(node, "quiet", quiet, &stop));
            let n = s.spawn(|| drive(node, "noisy", noisy, &stop));
            std::thread::sleep(RUN);
            stop.store(true, Ordering::Relaxed);
            (q.join().expect("quiet"), n.join().expect("noisy"))
        })
    }

    #[test]
    #[ignore = "a measurement, run by hand"]
    fn the_gate_on_and_off_below_its_slots_and_at_saturation() {
        let dir = TempDir::new("search-slots-measure");
        let root = dir.path().display().to_string();
        {
            let node = boot(&root, "0");
            fill(&node, "quiet");
            fill(&node, "noisy");
            // Publishes what it holds, so a node started after reads it.
            let _ = node.retire(20_000);
        }
        for (label, quiet, noisy) in [("below the slots", 6, 0), ("saturated", 2, 60)] {
            let mut off = (Vec::new(), Vec::new());
            let mut on = (Vec::new(), Vec::new());
            for _ in 0..ROUNDS {
                for (slots, into) in [("0", &mut off), (SLOTS, &mut on)] {
                    let node = boot(&root, slots);
                    // Warm the shards before the clock runs.
                    warm(&node);
                    let (q, n) = together(&node, quiet, noisy);
                    if slots != "0" {
                        let usage =
                            support::request(node.addr(), "GET", "/_serverless/usage", None).text();
                        let at = usage.find("\"search_slots\"").unwrap_or(0);
                        let end = usage[at..].find('}').map_or(usage.len(), |e| at + e + 1);
                        println!("   gate counters: {}", &usage[at..end]);
                    }
                    into.0.extend(q);
                    into.1.extend(n);
                    drop(node);
                }
            }
            println!(
                "== {label} ({quiet} quiet clients, {noisy} noisy clients, {ROUNDS} rounds of {RUN:?})"
            );
            line("gate off, quiet", off.0);
            line("gate on,  quiet", on.0);
            if noisy > 0 {
                line("gate off, noisy", off.1);
                line("gate on,  noisy", on.1);
            }
        }
    }

    /// Searches both indexes until each answers: a node just started may
    /// still be opening its shards.
    fn warm(node: &Node) {
        for index in ["quiet", "noisy"] {
            let started = Instant::now();
            loop {
                let reply = support::request(
                    node.addr(),
                    "POST",
                    &format!("/{index}/_search"),
                    Some(QUERY.as_bytes()),
                );
                if reply.status == 200 {
                    break;
                }
                assert!(
                    started.elapsed() < Duration::from_secs(20),
                    "{}: {}",
                    reply.status,
                    &reply.text()[..reply.text().len().min(300)]
                );
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}
