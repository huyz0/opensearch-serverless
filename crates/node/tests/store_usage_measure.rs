//! docs/specs/store-usage: what counting the store's requests per index
//! costs a write (`OSS_STORE_USAGE`), measured before its default moves.
//! Evidence run by hand with `--ignored --nocapture`; a time is not a
//! predicate, so the only assertion is that every write was acknowledged.

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

    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    use oss_node::boot::{Node, NodeConfig, start};
    use oss_testkit::TempDir;

    const ROUNDS: usize = 4;
    const RUN: Duration = Duration::from_secs(6);
    const CLIENTS: usize = 8;
    const BULK: usize = 20;

    fn boot(root: &str, usage: &str) -> Node {
        let lookup = |key: &str| match key {
            "OSS_NODE_ID" => Some("n1".to_owned()),
            "OSS_STORE" => Some("fs".to_owned()),
            "OSS_FS_ROOT" => Some(root.to_owned()),
            "OSS_HTTP_ADDR" | "OSS_TRANSPORT_ADDR" => Some("127.0.0.1:0".to_owned()),
            "OSS_STORE_USAGE" => Some(usage.to_owned()),
            _ => None,
        };
        start(&NodeConfig::from_lookup(lookup).expect("valid")).expect("starts")
    }

    /// One bulk of `BULK` small documents, to index `index`, ids from `base`.
    fn bulk(index: &str, base: u64) -> String {
        let mut body = String::new();
        for i in 0..BULK as u64 {
            body.push_str(&format!(
                "{{\"index\":{{\"_index\":\"{index}\",\"_id\":\"{}\"}}}}\n",
                base + i
            ));
            body.push_str(&format!(
                "{{\"n\":{},\"text\":\"alpha beta {}\"}}\n",
                base + i,
                i % 7
            ));
        }
        body
    }

    /// `CLIENTS` closed-loop writers for `RUN`: each request one bulk, each
    /// to its own index so the usage has several to count.
    fn drive(node: &Node, round: usize) -> Vec<u64> {
        let addr = node.addr();
        let stop = AtomicBool::new(false);
        let next = AtomicU64::new(1);
        std::thread::scope(|s| {
            let handles: Vec<_> = (0..CLIENTS)
                .map(|c| {
                    let (stop, next) = (&stop, &next);
                    s.spawn(move || {
                        let index = format!("w{round}-{c}");
                        assert_eq!(
                            support::request(addr, "PUT", &format!("/{index}"), None).status,
                            200
                        );
                        let mut seen = Vec::new();
                        while !stop.load(Ordering::Relaxed) {
                            let base = next.fetch_add(BULK as u64, Ordering::Relaxed);
                            let body = bulk(&index, base);
                            let at = Instant::now();
                            let reply =
                                support::request(addr, "POST", "/_bulk", Some(body.as_bytes()));
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
            std::thread::sleep(RUN);
            stop.store(true, Ordering::Relaxed);
            handles
                .into_iter()
                .flat_map(|h| h.join().expect("client"))
                .collect()
        })
    }

    fn pct(sorted: &[u64], p: f64) -> f64 {
        let at = ((sorted.len() as f64 - 1.0) * p).round() as usize;
        sorted[at] as f64 / 1000.0
    }

    fn line(what: &str, mut seen: Vec<u64>) {
        seen.sort_unstable();
        println!(
            "{what}: bulks={} p50={:.1}ms p99={:.1}ms max={:.1}ms",
            seen.len(),
            pct(&seen, 0.5),
            pct(&seen, 0.99),
            pct(&seen, 1.0)
        );
    }

    #[test]
    #[ignore = "a measurement, run by hand"]
    fn store_usage_on_and_off_for_a_write() {
        let mut off = Vec::new();
        let mut on = Vec::new();
        for round in 0..ROUNDS {
            for (usage, into) in [("false", &mut off), ("true", &mut on)] {
                let dir = TempDir::new("store-usage-measure");
                let root = dir.path().display().to_string();
                let node = boot(&root, usage);
                // One bulk first, so the node's start is not in the clock.
                let _ = drive_warm(&node);
                into.extend(drive(&node, round));
                drop(node);
            }
        }
        println!("== {CLIENTS} clients, bulks of {BULK}, {ROUNDS} rounds of {RUN:?}");
        line("usage off", off);
        line("usage on ", on);
    }

    fn drive_warm(node: &Node) -> u16 {
        let addr = node.addr();
        assert_eq!(support::request(addr, "PUT", "/warm", None).status, 200);
        let body = bulk("warm", 1);
        support::request(addr, "POST", "/_bulk", Some(body.as_bytes())).status
    }
}
