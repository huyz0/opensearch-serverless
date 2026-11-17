//! The node log's acknowledgement time under an open load, the shell's
//! `WriteAckMeterTests` open mode reduced to the log: writes arrive at
//! random times at a fixed rate whatever is acknowledged, as many
//! independent clients would, over many shards, against a store whose PUT
//! takes 20 ms. Off unless `OSS_ACK_SECONDS` is set; `OSS_ACK_RATE` (100
//! writes/s), `OSS_ACK_SHARDS` (200) and `OSS_ACK_WINDOW_MS` (250) as in
//! the shell. Emulator numbers: provisional, relative only.

mod common;

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use common::committer::{rec, rig, shard, window};
use oss_blob::S3Op;
use oss_blob::emulator::faults::Latency;

fn env(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// A seeded xorshift: the arrival times repeat run to run.
struct Gen(u64);

impl Gen {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Uniform in (0, 1].
    fn unit(&mut self) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let x = (self.next() >> 11) as f64;
        (x + 1.0) / 9_007_199_254_740_992.0
    }
}

fn percentile(sorted: &[Duration], p: f64) -> f64 {
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let at = ((p * sorted.len() as f64).ceil() as usize).saturating_sub(1);
    sorted
        .get(at.min(sorted.len().saturating_sub(1)))
        .map_or(0.0, |d| d.as_secs_f64() * 1e3)
}

#[test]
#[ignore = "a meter, minutes long: set OSS_ACK_SECONDS and run with --ignored"]
fn open_load_ack_meter() {
    let seconds = env("OSS_ACK_SECONDS", 0);
    if seconds == 0 {
        return;
    }
    let rate = env("OSS_ACK_RATE", 100);
    let shards = u32::try_from(env("OSS_ACK_SHARDS", 200)).expect("fits");
    let r = rig(window(env("OSS_ACK_WINDOW_MS", 250)));
    r.store
        .set_latency(S3Op::Put, Latency::fixed(Duration::from_millis(20)));
    let committer = Arc::clone(&r.committer);
    // Warm up: open the epoch and admit every shard, as a node that has been
    // writing has.
    let warm: Vec<_> = (0..shards)
        .map(|n| {
            let c = Arc::clone(&committer);
            thread::spawn(move || c.commit(&shard(n), 1, &[rec("warm", 0)]).expect("lands"))
        })
        .collect();
    warm.into_iter().for_each(|h| drop(h.join()));
    committer.reset_timings();

    let acks = Arc::new(Mutex::new(Vec::new()));
    let mut draws = Gen(0x9E37_79B9_7F4A_7C15);
    let start = Instant::now();
    let end = start + Duration::from_secs(seconds);
    #[allow(clippy::cast_precision_loss)]
    let mean_gap = 1.0 / rate as f64;
    let mut at = start;
    let mut seq: i64 = 1;
    let mut writers = Vec::new();
    while at < end {
        // Exponential gaps: a Poisson stream of arrivals.
        at += Duration::from_secs_f64(-draws.unit().ln() * mean_gap);
        if let Some(wait) = at.checked_duration_since(Instant::now()) {
            thread::sleep(wait);
        }
        let n = u32::try_from(draws.next() % u64::from(shards)).expect("fits");
        let c = Arc::clone(&committer);
        let out = Arc::clone(&acks);
        let id = format!("d{seq}");
        writers.push(thread::spawn(move || {
            let began = Instant::now();
            c.commit(&shard(n), 1, &[rec(&id, seq)]).expect("lands");
            out.lock().expect("unpoisoned").push(began.elapsed());
        }));
        seq = seq.saturating_add(1);
    }
    writers.into_iter().for_each(|h| drop(h.join()));
    let mut acks = acks.lock().expect("unpoisoned").clone();
    acks.sort();
    let t = committer.timings();
    println!(
        "open load: {} writes in {seconds} s over {shards} shards, {rate} writes/s offered: ack p50 {:.1} ms, p90 {:.1} ms, p99 {:.1} ms; {t}",
        acks.len(),
        percentile(&acks, 0.50),
        percentile(&acks, 0.90),
        percentile(&acks, 0.99),
    );
    assert!(!acks.is_empty());
}
