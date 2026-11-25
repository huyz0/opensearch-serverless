//! docs/specs/writer-hand-off: what a node joining a full fleet gets from
//! hand-off (and headroom) and what it costs, on the emulator with request
//! latency. Ignored: it prints numbers for `VERIFIED.md` and takes minutes.
//!
//! `cargo test -p oss-node --all-features --test hand_off_fill_evidence -- --ignored --nocapture`
//! Knobs: `HAND_OFF_EVIDENCE_RUNS` (default 3), `HAND_OFF_EVIDENCE_SECONDS`
//! (default 30).

#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]
// ARITH: sums and means of small test numbers.
#![allow(clippy::arithmetic_side_effects)]

mod suite_rig;

use std::time::{Duration, Instant};

use oss_blob::S3Op;
use suite_rig::{Cluster, wait_for};

const CAP: usize = 20;

fn knob(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let at = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[at]
}

struct Arm {
    name: &'static str,
    hand_off: bool,
    headroom: u32,
}

struct Run {
    p50: f64,
    p99: f64,
    bulks: usize,
    errors: usize,
    held: [i32; 2],
    puts: u64,
    gets: u64,
    heads: u64,
    lists: u64,
    count: u64,
    acked: u64,
    thirds: Vec<(f64, u64)>,
}

fn run(arm: &Arm, seconds: u64) -> Run {
    let mut c = Cluster::new(3_000, 1, false).expect("emulator");
    c.set("OSS_MAX_SHARDS", &CAP.to_string());
    c.set("OSS_EVICT_AFTER_MS", "1500");
    c.set("OSS_EVICT_HEADROOM_PCT", &arm.headroom.to_string());
    c.set("OSS_HAND_OFF", if arm.hand_off { "true" } else { "false" });
    c.add("n1");
    let body = format!("{{\"settings\":{{\"index\":{{\"number_of_shards\":{CAP}}}}}}}");
    assert_eq!(c.call(0, "PUT", "/logs", Some(&body)).status, 200);
    // Fill every shard so n1 holds its whole cap.
    let mut fill = String::new();
    for i in 0..400 {
        fill.push_str(&format!(
            "{{\"index\":{{\"_id\":\"f{i}\"}}}}\n{{\"n\":{i}}}\n"
        ));
    }
    let r = c.call(0, "POST", "/logs/_bulk?refresh=true", Some(&fill));
    assert_eq!(r.status, 200, "{}", r.text());
    c.add("n2");
    wait_for("both see two", 30, || (0..2).all(|s| c.nodes_seen(s) == 2));
    let sent = |c: &Cluster, op| c.sent(op);
    let before = [
        sent(&c, S3Op::Put),
        sent(&c, S3Op::Get) + sent(&c, S3Op::GetRange),
        sent(&c, S3Op::Head),
        sent(&c, S3Op::List),
    ];
    let end = Instant::now() + Duration::from_secs(seconds);
    let mut lat = Vec::new();
    let mut stamped: Vec<(Instant, f64)> = Vec::new();
    let started = Instant::now();
    let mut marks = vec![before[1]];
    let mut errors = 0;
    let mut acked = 400;
    let mut n = 0_u64;
    while Instant::now() < end {
        let slot = (n % 2) as usize;
        let mut b = String::new();
        for k in 0..2 {
            let id = format!("w{n}-{k}");
            b.push_str(&format!(
                "{{\"index\":{{\"_id\":\"{id}\"}}}}\n{{\"n\":{n}}}\n"
            ));
        }
        let t = Instant::now();
        let r = c.call(slot, "POST", "/logs/_bulk", Some(&b));
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        if r.status == 200 && r.text().contains("\"errors\":false") {
            lat.push(ms);
            stamped.push((Instant::now(), ms));
            acked += 2;
        } else {
            errors += 1;
        }
        n += 1;
        let third = (started.elapsed().as_secs() * 3 / seconds.max(1)) as usize;
        if marks.len() <= third.min(2) {
            marks.push(sent(&c, S3Op::Get) + sent(&c, S3Op::GetRange));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let after = [
        sent(&c, S3Op::Put),
        sent(&c, S3Op::Get) + sent(&c, S3Op::GetRange),
        sent(&c, S3Op::Head),
        sent(&c, S3Op::List),
    ];
    let held = [c.held_of(0), c.held_of(1)];
    let counted = c.call(0, "GET", "/logs/_count", None);
    let count = counted
        .text()
        .split_once("\"count\":")
        .and_then(|(_, r)| {
            r.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .ok()
        })
        .unwrap_or(0);
    marks.push(after[1]);
    let thirds = (0..3)
        .map(|t| {
            let lo = started + Duration::from_secs(seconds * t / 3);
            let hi = started + Duration::from_secs(seconds * (t + 1) / 3);
            let mut v: Vec<f64> = stamped
                .iter()
                .filter(|(at, _)| *at >= lo && *at < hi)
                .map(|(_, ms)| *ms)
                .collect();
            v.sort_by(f64::total_cmp);
            let gets = marks
                .get(t as usize + 1)
                .zip(marks.get(t as usize))
                .map_or(0, |(b, a)| b.saturating_sub(*a));
            (percentile(&v, 0.99), gets)
        })
        .collect();
    lat.sort_by(f64::total_cmp);
    Run {
        p50: percentile(&lat, 0.5),
        p99: percentile(&lat, 0.99),
        bulks: lat.len(),
        errors,
        held,
        puts: after[0] - before[0],
        gets: after[1] - before[1],
        heads: after[2] - before[2],
        lists: after[3] - before[3],
        count,
        acked,
        thirds,
    }
}

#[test]
#[ignore = "evidence: prints numbers, takes minutes"]
fn a_node_joining_a_full_fleet_with_and_without_hand_off() {
    let runs = knob("HAND_OFF_EVIDENCE_RUNS", 3);
    let seconds = knob("HAND_OFF_EVIDENCE_SECONDS", 30);
    println!(
        "HO cap={CAP} shards={CAP} runs={runs} seconds={seconds} (emulator, Java latency profile; provisional)"
    );
    let only = std::env::var("HAND_OFF_EVIDENCE_ARMS").unwrap_or_default();
    let arms = [
        Arm {
            name: "off",
            hand_off: false,
            headroom: 0,
        },
        Arm {
            name: "hand_off",
            hand_off: true,
            headroom: 0,
        },
        Arm {
            name: "hand_off+headroom25",
            hand_off: true,
            headroom: 25,
        },
    ];
    for arm in arms
        .iter()
        .filter(|a| only.is_empty() || only.contains(a.name))
    {
        for n in 0..runs {
            let r = run(arm, seconds);
            println!(
                "HO arm={} run={n} write_p50={:.1}ms p99={:.1}ms bulks={} errors={} held=[n1 {}, n2 {}] puts={} gets={} heads={} lists={} acked={} counted={} thirds(p99ms,gets)={:?}",
                arm.name,
                r.p50,
                r.p99,
                r.bulks,
                r.errors,
                r.held[0],
                r.held[1],
                r.puts,
                r.gets,
                r.heads,
                r.lists,
                r.acked,
                r.count,
                r.thirds
            );
        }
    }
}
