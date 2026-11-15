//! Blocking fetches at the node's cap: one thread per request in flight,
//! every one of them reaching the store at once, none beyond, and a
//! request that does not wait behind them. Asserted from counts the gate
//! proxy observes, never from elapsed time, so a slow or instrumented
//! runner cannot fail it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::gate::Gate;
use crate::{config, credentials, fixture, get_all, pattern, put};
use oss_blob::*;

/// Fetches in flight at once: the 256-thread cap ADR 0005 sizes a node
/// for, less headroom for the probe's and the pool's connections under the
/// front end's own limit of 256.
const IN_FLIGHT: usize = 192;

/// GETs each fetching thread issues, back to back.
const ROUNDS: usize = 4;

/// Fetching threads started at once.
const BATCH: usize = 32;
const _: () = assert!(IN_FLIGHT.is_multiple_of(BATCH), "every thread starts");

/// How long the test waits, in all, for every fetch to arrive and the HEAD
/// to be answered before calling the client broken. Only a broken client
/// gets near it.
const ARRIVAL_LIMIT: Duration = Duration::from_secs(30);

#[test]
fn fetches_up_to_the_cap_run_in_parallel_and_starve_nothing() {
    let f = fixture();
    let body = pattern(4096);
    put(&f.client, "seg", &body);
    let gate = Gate::start(&f.server.endpoint());
    // A client that keeps a connection per fetching thread, as a node's
    // pool sized to its cap does. Held GETs wait on the gate, not on the
    // network, so its I/O timeout must outlast the whole arrival limit.
    let mut c = config(&gate.endpoint(), credentials());
    c.max_idle_connections = IN_FLIGHT + 1;
    c.io_timeout = ARRIVAL_LIMIT.saturating_mul(2);
    let client = Arc::new(S3Store::new(c).unwrap());
    gate.hold();
    let deadline = Instant::now().checked_add(ARRIVAL_LIMIT).unwrap();
    // Threads start a batch at a time, each batch once the last is held:
    // a held GET keeps its connection, so every thread opens its own, and
    // a burst of IN_FLIGHT connects at once would overflow the listeners'
    // accept backlog of 128 and stall a second on a SYN retransmit.
    let mut fetchers = Vec::with_capacity(IN_FLIGHT);
    for n in (BATCH..=IN_FLIGHT).step_by(BATCH) {
        while fetchers.len() < n {
            let client = Arc::clone(&client);
            let body = body.clone();
            fetchers.push(std::thread::spawn(move || {
                for _ in 0..ROUNDS {
                    assert_eq!(get_all(&*client, "seg", ByteRange::Full).unwrap(), body);
                }
            }));
        }
        // Every thread started so far has its first GET at the store, none
        // answered: the client sends them in parallel, not one at a time.
        assert_eq!(gate.wait_held(n, deadline), n, "GETs in flight at once");
    }
    // With all of them in flight, a HEAD is still answered: it does not
    // queue behind the fetches.
    // It runs on its own thread so a HEAD that does queue fails the test
    // rather than hanging it on the GETs it waits for.
    let (answered, answer) = std::sync::mpsc::sync_channel(1);
    let prober = Arc::clone(&client);
    std::thread::spawn(move || answered.send(prober.head("seg").map(|m| m.map(|m| m.len))));
    let head = answer
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .expect("a HEAD queued behind the GETs in flight");
    assert_eq!(head.unwrap(), Some(4096));
    assert_eq!(gate.heads(), 1);
    gate.release();
    for t in fetchers {
        t.join().unwrap();
    }
    // Never more in flight than threads fetching, over every round.
    assert_eq!(gate.peak(), IN_FLIGHT, "peak GETs in flight");
    // Every GET the client meters is one the server was sent, no more.
    let gets = u64::try_from(IN_FLIGHT.saturating_mul(ROUNDS)).unwrap();
    assert_eq!(u64::try_from(gate.gets()).unwrap(), gets, "GETs sent");
    assert_eq!(client.meter().count(S3Op::Get), gets);
    assert_eq!(client.meter().count(S3Op::Head), 1);
    assert_eq!(client.meter().errors(), 0);
}
