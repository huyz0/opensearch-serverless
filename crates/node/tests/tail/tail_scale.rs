//! docs/specs/tail-overlay-scale: what the writer's tail machinery costs a
//! node that holds thousands of shards, every one acknowledging a write in
//! the same flush interval and followed by one reader. Evidence run by hand
//! with `--ignored --nocapture`; a time is not a predicate, so the one
//! assertion is the bytes kept, which are counted and not timed.

#![cfg(feature = "writer")]
// ARITH: test arithmetic on counts, bytes and durations; an overflow panics the test.
#![allow(clippy::arithmetic_side_effects, clippy::cast_precision_loss)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use oss_cluster::lease::LeaseClock;
use oss_format::WalRecord;
use oss_node::push_sender::LinkError;
use oss_node::tail_keep::TailKeep;
use oss_node::tail_push::{TailLink, TailPush};
use oss_node::tail_serve::{ShardFacts, TailServe};
use oss_transport::actions::tail::{FetchRequest, NEW, PushRequest, PushResponse, ShardRef};

struct Facts;

impl ShardFacts for Facts {
    fn writes(&self, _: &str, _: u32) -> bool {
        true
    }
    fn ownership_in_doubt(&self) -> bool {
        false
    }
    fn overlay_on(&self, _: &str, _: u32) -> bool {
        true
    }
    fn max_seq_no(&self, _: &str, _: u32) -> i64 {
        0
    }
}

struct Clock;

impl LeaseClock for Clock {
    fn now_millis(&self) -> i64 {
        0
    }
    fn pause(&self, _: Duration) {}
}

/// Takes every push and wants all of it.
struct Sink;

impl TailLink for Sink {
    fn send(&self, _: &str, _: &PushRequest) -> Result<PushResponse, LinkError> {
        Ok(PushResponse::default())
    }
}

/// The process's resident set in bytes, from `/proc/self/statm`; zero where
/// there is none.
fn rss() -> u64 {
    use std::io::Read;
    let mut text = [0_u8; 128];
    let Ok(mut file) = std::fs::File::open("/proc/self/statm") else {
        return 0;
    };
    let read = file.read(&mut text).unwrap_or(0);
    std::str::from_utf8(&text[..read])
        .ok()
        .and_then(|s| {
            s.split_whitespace()
                .nth(1)
                .and_then(|p| p.parse::<u64>().ok())
        })
        .map_or(0, |pages| pages * 4096)
}

fn uuid(i: u32) -> String {
    format!("uuid-{i:06}")
}

/// One run at `shards` shards: every shard subscribed by one reader, then
/// acknowledging one 200-byte write, then one flush.
fn run(shards: u32) -> (u64, Duration, Duration, u64) {
    let keep = Arc::new(TailKeep::new());
    let serve = Arc::new(TailServe::new(
        keep.clone(),
        Arc::new(Facts),
        Arc::new(Clock),
    ));
    let push = TailPush::new(keep.clone(), serve.clone(), Arc::new(Sink), Arc::new(Facts));
    let before = rss();
    for i in 0..shards {
        serve.answer(&FetchRequest {
            shard: ShardRef {
                index_name: "logs".into(),
                index_uuid: uuid(i),
                shard: 0,
            },
            after_seq_no: 0,
            subscriber: "reader".into(),
            subscription: NEW,
            ..FetchRequest::default()
        });
    }
    let source = "x".repeat(180);
    let started = Instant::now();
    for i in 0..shards {
        let record = WalRecord::index(format!("d{i}"), source.clone(), 1, 1, 1);
        push.acked("logs", &uuid(i), 0, &[record], true);
    }
    let acking = started.elapsed();
    let started = Instant::now();
    push.flush();
    let flushing = started.elapsed();
    (
        rss().saturating_sub(before),
        acking,
        flushing,
        keep.kept_bytes(),
    )
}

#[test]
#[ignore = "evidence: run by hand with --nocapture"]
fn the_writers_tail_machinery_at_thousands_of_shards() {
    for shards in [100, 1_000, 5_000, 20_000] {
        let (bytes, acking, flushing, kept) = run(shards);
        println!(
            "shards={shards:>6} rss+={:>7} KiB ({:>5} B/shard) kept={kept} B \
             ack-all={acking:?} ({:.1} us/ack) flush={flushing:?}",
            bytes / 1024,
            bytes / u64::from(shards),
            acking.as_secs_f64() * 1e6 / f64::from(shards),
        );
        assert!(kept >= u64::from(shards) * 180, "every write is kept");
    }
}
