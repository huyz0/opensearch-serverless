//! Blocking fetches under injected latency: one thread per request in
//! flight, up to the node's cap, and a request that does not wait behind
//! them. Timings are against the emulator and provisional.

use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use crate::{config, credentials, fixture, get_all, pattern, put};
use oss_blob::emulator::faults::Latency;
use oss_blob::*;

/// Fetches in flight at once: the 256-thread cap ADR 0005 sizes a node
/// for, less headroom for the probe's and the pool's connections under the
/// front end's own limit of 256.
const IN_FLIGHT: usize = 192;

/// The latency injected on every GET, about one S3 round trip.
const GET_LATENCY: Duration = Duration::from_millis(50);

/// GETs each fetching thread issues, back to back.
const ROUNDS: usize = 4;

#[test]
fn fetches_up_to_the_cap_run_in_parallel_and_starve_nothing() {
    let f = fixture();
    let body = pattern(4096);
    put(&f.client, "seg", &body);
    // A client that keeps a connection per fetching thread, as a node's
    // pool sized to its cap does, so the run measures requests rather
    // than the loopback accept backlog.
    let mut c = config(&f.server.endpoint(), credentials());
    c.max_idle_connections = IN_FLIGHT + 1;
    let client = Arc::new(S3Store::new(c).unwrap());
    f.server
        .store()
        .set_latency(S3Op::Get, Latency::fixed(GET_LATENCY));
    let ready = Arc::new(Barrier::new(IN_FLIGHT + 1));
    let start = Arc::new(Barrier::new(IN_FLIGHT + 1));
    let fetchers: Vec<_> = (0..IN_FLIGHT)
        .map(|_| {
            let client = Arc::clone(&client);
            let ready = Arc::clone(&ready);
            let start = Arc::clone(&start);
            let body = body.clone();
            std::thread::spawn(move || {
                // Opens this thread's connection before the run starts.
                client.head("seg").unwrap();
                ready.wait();
                start.wait();
                for _ in 0..ROUNDS {
                    assert_eq!(get_all(&*client, "seg", ByteRange::Full).unwrap(), body);
                }
            })
        })
        .collect();
    ready.wait();
    start.wait();
    let began = Instant::now();
    // HEADs carry no injected latency: each one should take a loopback
    // round trip, not a GET's 50 ms queued behind the fetches.
    let mut probes = Vec::new();
    while began.elapsed() < GET_LATENCY.saturating_mul(2) {
        let t = Instant::now();
        assert_eq!(client.head("seg").unwrap().map(|m| m.len), Some(4096));
        probes.push(t.elapsed());
    }
    for t in fetchers {
        t.join().unwrap();
    }
    let wall = began.elapsed();
    let gets = u64::try_from(IN_FLIGHT.saturating_mul(ROUNDS)).unwrap();
    assert_eq!(client.meter().count(S3Op::Get), gets);
    assert_eq!(client.meter().errors(), 0);
    // In parallel the GETs take about ROUNDS * 50 ms, 200 ms; serially
    // IN_FLIGHT * ROUNDS * 50 ms, 38 s. Four times the parallel time
    // leaves room for a busy runner and none for queueing.
    let parallel = GET_LATENCY.saturating_mul(u32::try_from(ROUNDS).unwrap());
    assert!(
        wall < parallel.saturating_mul(4),
        "{IN_FLIGHT} x {ROUNDS} GETs took {wall:?}"
    );
    probes.sort();
    let median = probes[probes.len() / 2];
    assert!(
        median < GET_LATENCY / 2,
        "median HEAD {median:?} over {} probes",
        probes.len()
    );
    eprintln!(
        "provisional: {gets} GETs at {GET_LATENCY:?} in {wall:?}; {} HEADs, median {median:?}, max {:?}",
        probes.len(),
        probes.last().unwrap()
    );
}
