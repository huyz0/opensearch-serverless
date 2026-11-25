//! docs/specs/search-slots: the cold-cache measurement ADR 0047 asks for. A
//! noisy index too big for the node's resident pages and a quiet one that
//! fits, on a store with request latency, with the gate off and on. Evidence
//! run by hand with `--ignored --nocapture`: a time is not a predicate, so the
//! only assertions are that every search answered 200.

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

#[path = "suite_rig/mod.rs"]
mod suite_rig;

// ARITH: test arithmetic on counts and durations; an overflow panics the test.
#[allow(
    clippy::arithmetic_side_effects,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::disallowed_methods
)]
mod run {
    use super::suite_rig::{Cluster, wait_for};

    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use oss_blob::S3Op;

    const NOISY_DOCS: usize = 60_000;
    const QUIET_DOCS: usize = 500;
    const RUN: Duration = Duration::from_secs(15);
    const ROUNDS: usize = 3;
    const MAPPING: &[u8] = br#"{"mappings":{"properties":{"body":{"type":"text"}}}}"#;

    fn fill(c: &Cluster, index: &str, docs: usize) {
        assert_eq!(
            c.call(
                0,
                "PUT",
                &format!("/{index}"),
                Some(std::str::from_utf8(MAPPING).expect("utf8"))
            )
            .status,
            200
        );
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for chunk in 0..docs.div_ceil(5_000) {
            let mut body = String::new();
            for i in chunk * 5_000..((chunk + 1) * 5_000).min(docs) {
                body.push_str(&format!("{{\"index\":{{\"_id\":\"{i}\"}}}}\n"));
                let words: Vec<String> = (0..30).map(|_| format!("w{}", next() % 50_000)).collect();
                let noise: String = (0..12).map(|_| format!("{:016x}", next())).collect();
                body.push_str(&format!("{{\"body\":\"{} {noise}\"}}\n", words.join(" ")));
            }
            let r = c.call(0, "POST", &format!("/{index}/_bulk"), Some(&body));
            assert_eq!(r.status, 200, "{}", &r.text()[..r.text().len().min(300)]);
        }
    }

    /// `clients` closed-loop searchers of `index` for `RUN` on node `slot`;
    /// each search is a different term, so a cold page is a likely read.
    fn drive(c: &Cluster, slot: usize, index: &str, clients: usize, stop: &AtomicBool) -> Vec<u64> {
        std::thread::scope(|s| {
            let handles: Vec<_> = (0..clients)
                .map(|k| {
                    s.spawn(move || {
                        let mut seen = Vec::new();
                        let mut i = k * 7919;
                        while !stop.load(Ordering::Relaxed) {
                            i += 1;
                            let q = format!(
                                r#"{{"size":20,"query":{{"match":{{"body":"w{} w{}"}}}}}}"#,
                                i * 37 % 50_000,
                                i * 101 % 50_000
                            );
                            let at = Instant::now();
                            let r = c.call(slot, "POST", &format!("/{index}/_search"), Some(&q));
                            assert_eq!(r.status, 200, "{}", &r.text()[..r.text().len().min(300)]);
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

    fn line(what: &str, mut seen: Vec<u64>) {
        seen.sort_unstable();
        let at = |p: f64| {
            seen.get(((seen.len() as f64 - 1.0) * p).round() as usize)
                .map_or(0.0, |v| *v as f64 / 1000.0)
        };
        println!(
            "{what}: n={} p50={:.1}ms p99={:.1}ms max={:.1}ms",
            seen.len(),
            at(0.5),
            at(0.99),
            at(1.0)
        );
    }

    /// `NOISY_CLIENTS`, default 40.
    fn noisy_clients() -> usize {
        std::env::var("NOISY_CLIENTS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(40)
    }

    fn ready(c: &Cluster, slot: usize) {
        for index in ["quiet", "noisy"] {
            wait_for("shard answers", 120, || {
                c.call(
                    slot,
                    "POST",
                    &format!("/{index}/_search"),
                    Some(r#"{"size":1}"#),
                )
                .status
                    == 200
            });
        }
    }

    /// One configuration: a fresh search-only node, `bound` resident bytes
    /// ("" for none), the gate with `slots`; the noisy and quiet latencies and
    /// the store's GETs during the run.
    fn round(c: &mut Cluster, id: &str, slots: &str, bound: &str) -> (Vec<u64>, Vec<u64>, u64) {
        c.set("OSS_SEARCH_SLOTS", slots);
        c.set("OSS_RESIDENT_PAGE_BYTES", bound);
        let slot = c.add(id);
        ready(c, slot);
        // RustFS has no meter: its GETs are not counted.
        let metered = std::env::var_os("RUSTFS_BIN").is_none();
        let gets = |c: &Cluster| {
            if metered {
                c.sent(S3Op::Get) + c.sent(S3Op::GetRange)
            } else {
                0
            }
        };
        let before = gets(c);
        let stop = AtomicBool::new(false);
        let (q, n) = std::thread::scope(|s| {
            let q = s.spawn(|| drive(c, slot, "quiet", 2, &stop));
            let n = s.spawn(|| drive(c, slot, "noisy", noisy_clients(), &stop));
            std::thread::sleep(RUN);
            stop.store(true, Ordering::Relaxed);
            (q.join().expect("quiet"), n.join().expect("noisy"))
        });
        (q, n, gets(c) - before)
    }

    #[test]
    #[ignore = "a measurement, run by hand"]
    fn the_gate_with_a_cold_noisy_tenant() {
        let rustfs = std::env::var_os("RUSTFS_BIN").is_some();
        let mut c = Cluster::new(30_000, 2, rustfs).expect("a store");
        c.set("OSS_ROLES", "ingest");
        let w = c.add("w");
        fill(&c, "noisy", NOISY_DOCS);
        fill(&c, "quiet", QUIET_DOCS);
        let _ = w;
        c.retire(0);
        let dir = std::env::temp_dir().join(format!("oss-cold-{}", std::process::id()));
        c.set("OSS_ROLES", "search");
        c.set("OSS_PAGE_CACHE_BYTES", "2097152");
        for (label, bound) in [
            ("resident bound off", ""),
            ("resident bound 4 MiB", "4194304"),
        ] {
            if bound.is_empty() && std::env::var_os("BOUND_ONLY").is_some() {
                continue;
            }
            let mut all = [
                (Vec::new(), Vec::new(), 0_u64),
                (Vec::new(), Vec::new(), 0_u64),
            ];
            for r in 0..ROUNDS {
                for (k, slots) in ["0", "8"].into_iter().enumerate() {
                    c.set(
                        "OSS_PAGE_CACHE_DIR",
                        &dir.join(format!("{slots}-{}-{r}", bound.len()))
                            .display()
                            .to_string(),
                    );
                    let (q, n, gets) = round(
                        &mut c,
                        &format!("s{slots}{}-{r}", bound.len()),
                        slots,
                        bound,
                    );
                    all[k].0.extend(q);
                    all[k].1.extend(n);
                    all[k].2 += gets;
                }
            }
            for (k, slots) in ["0", "8"].into_iter().enumerate() {
                let (q, n, gets) = std::mem::take(&mut all[k]);
                println!(
                    "== {label}, slots {slots}: store GETs {gets} over {ROUNDS} rounds of {RUN:?}"
                );
                line("   quiet", q);
                line("   noisy", n);
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
