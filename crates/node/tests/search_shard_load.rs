//! docs/specs/java-search-shard-action AC-11: the write-and-refresh time on
//! an owner that is answering forwarded searches at its pool's limit, against
//! the same owner with none. A measurement, not a gate: the numbers come from
//! an in-process emulator on a shared machine and are provisional.

// ARITH: counts and indexes are small; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]
#![cfg(all(feature = "reader", feature = "writer", feature = "coordinator"))]

mod suite_rig;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use oss_blob::S3Op;
use oss_cluster::lease::SystemClock;
use oss_cluster::transport_secret::TransportSecretStore;
use oss_mem::{BudgetConfig, MemoryBudget, Pool};
use oss_transport::actions::search_shard::{Request, SEARCH_ACTION, Source};
use oss_transport::auth::{AuthConfig, Authenticator, MAC_HEADER, SecretSource, Secrets, identity};
use oss_transport::client::{Call, Client, ClientConfig, PeerId};
use oss_transport::stream::{Input, Output, TaskId};
use suite_rig::{Cluster, wait_for};

const TTL_MS: u64 = 3_000;

/// A number the run is tuned by, from the environment.
fn knob(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

struct Fixed(Arc<Secrets>);

impl SecretSource for Fixed {
    fn secrets(&self) -> Option<Arc<Secrets>> {
        Some(Arc::clone(&self.0))
    }

    fn refresh(&self) {}
}

fn now_millis() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after the epoch")
            .as_millis(),
    )
    .expect("fits")
}

fn write_round(cluster: &Cluster, round: usize) -> Duration {
    let body = format!("{{\"index\":{{\"_index\":\"logs\",\"_id\":\"d{round}\"}}}}\n{{\"n\":1}}\n");
    let started = Instant::now();
    let r = cluster.call(0, "POST", "/_bulk?refresh=true", Some(&body));
    let took = started.elapsed();
    assert!(
        r.status == 200 && r.text().contains("\"errors\":false"),
        "{}",
        r.text()
    );
    took
}

fn percentile(sorted: &[Duration], p: usize) -> Duration {
    sorted[(sorted.len() * p / 100).saturating_sub(1)]
}

fn rounds(cluster: &Cluster, from: usize, count: usize) -> Vec<Duration> {
    let mut took: Vec<Duration> = (from..from + count)
        .map(|round| write_round(cluster, round))
        .collect();
    took.sort();
    took
}

#[test]
#[ignore = "a measurement, not a gate"]
fn owner_write_time_with_and_without_forwarded_searches_at_the_pool_limit() {
    // OSS_LOAD_SEARCHERS searcher threads, OSS_LOAD_POOL as the node's limit,
    // OSS_LOAD_PAUSE_MS between one searcher's requests, OSS_LOAD_ROUNDS writes.
    let searchers = knob("OSS_LOAD_SEARCHERS", 2);
    let pool = knob("OSS_LOAD_POOL", 2);
    let pause = Duration::from_millis(u64::try_from(knob("OSS_LOAD_PAUSE_MS", 0)).unwrap_or(0));
    let count = knob("OSS_LOAD_ROUNDS", 100);
    let mut cluster = Cluster::new(TTL_MS, 4, false).expect("emulated");
    cluster.set("OSS_SEARCH_SHARD_ACTION_MAX_CONCURRENT", &pool.to_string());
    cluster.add("n1");
    wait_for("the node sees itself", 30, || cluster.nodes_seen(0) == 1);
    std::thread::sleep(Duration::from_millis(TTL_MS));
    let made = cluster.call(
        0,
        "PUT",
        "/logs",
        Some(r#"{"settings":{"index":{"number_of_shards":1}},"mappings":{"properties":{"n":{"type":"integer"}}}}"#),
    );
    assert_eq!(made.status, 200, "{}", made.text());

    // The transport secret is the bucket's, read as a node reads it.
    let (endpoint, bucket, region, access, secret) = cluster.store();
    let store = oss_blob::S3Store::new(oss_blob::S3Config::new(
        &endpoint,
        bucket,
        region,
        oss_blob::Credentials::new(access, secret),
    ))
    .expect("a store");
    let secrets = TransportSecretStore::new(Arc::new(store), "", Arc::new(SystemClock))
        .expect("register")
        .read()
        .expect("secrets");
    let auth = Arc::new(Authenticator::new(
        AuthConfig {
            identity: identity("tester", "e1"),
            window_millis: 60_000,
            replay_capacity: 4_096,
        },
        Arc::new(Fixed(Arc::new(secrets))),
        Arc::new(now_millis),
    ));
    let budget = MemoryBudget::new(
        &BudgetConfig::new(64 << 20)
            .with_pool(Pool::Transport, 64 << 20)
            .expect("fits"),
    );
    let client = Arc::new(Client::new(ClientConfig::default(), budget));
    let node = cluster.node(0);
    let peer = PeerId {
        address: node.transport_addr(),
        ephemeral_id: Some(node.ephemeral_id().to_owned()),
    };
    let recipient = node.transport_identity();

    let search = {
        let (client, auth, peer, recipient) = (
            Arc::clone(&client),
            Arc::clone(&auth),
            peer.clone(),
            recipient.clone(),
        );
        move || -> Result<i64, String> {
            let request = Request {
                parent: TaskId::EMPTY,
                index: "logs".to_owned(),
                shard: 0,
                source: Source {
                    query: None,
                    from: 0,
                    size: 10,
                    min_score: None,
                    terminate_after: 0,
                    timeout_millis: None,
                    track_total_hits_up_to: Some(10_000),
                    fetch_source: None,
                    aggregations: Vec::new(),
                    pipelines: Vec::new(),
                },
                index_uuid: None,
                now_millis: now_millis(),
            };
            let mut message = Vec::new();
            request
                .write(&mut Output::new(&mut message, 1 << 20))
                .map_err(|e| format!("{e:?}"))?;
            let mac = auth
                .sign(SEARCH_ACTION, &message, &recipient, &[])
                .ok_or("no secret")?;
            let headers = [(MAC_HEADER.to_owned(), mac)];
            let call = Call {
                action: SEARCH_ACTION,
                headers: &headers,
                compress: false,
                deadline: Instant::now() + Duration::from_secs(10),
            };
            let answer = client
                .call(&peer, call, |out| out.bytes(&message))
                .map_err(|e| format!("{e:?}"))?;
            Input::new(answer.message())
                .vlong("total")
                .map_err(|e| format!("{e:?}"))
        }
    };

    // One answered search before anything is measured: the action serves.
    for round in 0..5 {
        write_round(&cluster, 10_000 + round);
    }
    assert_eq!(search(), Ok(5), "the owner answers the forwarded search");

    for round in 0..50 {
        write_round(&cluster, 20_000 + round);
    }
    let before = S3Op::ALL.map(|op| cluster.sent(op));
    let alone = rounds(&cluster, 0, count);
    let after_alone = S3Op::ALL.map(|op| cluster.sent(op));

    let stop = Arc::new(AtomicBool::new(false));
    let answered = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(AtomicU64::new(0));
    let threads: Vec<_> = (0..searchers)
        .map(|_| {
            let (stop, answered, failed, search) = (
                Arc::clone(&stop),
                Arc::clone(&answered),
                Arc::clone(&failed),
                search.clone(),
            );
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match search() {
                        Ok(_) => answered.fetch_add(1, Ordering::Relaxed),
                        Err(_) => failed.fetch_add(1, Ordering::Relaxed),
                    };
                    std::thread::sleep(pause);
                }
            })
        })
        .collect();
    std::thread::sleep(Duration::from_secs(1));
    let loaded = rounds(&cluster, 1_000, count);
    let after_loaded = S3Op::ALL.map(|op| cluster.sent(op));
    stop.store(true, Ordering::Relaxed);
    for t in threads {
        t.join().expect("a searcher");
    }
    eprintln!(
        "AC11 alone p50 {:?} p99 {:?} | {} searchers, pool {}, pause {:?}: p50 {:?} p99 {:?} ({} answered, {} refused)",
        percentile(&alone, 50),
        percentile(&alone, 99),
        searchers,
        pool,
        pause,
        percentile(&loaded, 50),
        percentile(&loaded, 99),
        answered.load(Ordering::Relaxed),
        failed.load(Ordering::Relaxed),
    );
    eprintln!(
        "AC11 requests per write round, alone {:?}, with searchers {:?} (all {} kinds, in S3Op::ALL order)",
        S3Op::ALL
            .iter()
            .enumerate()
            .map(|(i, _)| (after_alone[i] - before[i]) as f64 / count as f64)
            .collect::<Vec<_>>(),
        S3Op::ALL
            .iter()
            .enumerate()
            .map(|(i, _)| (after_loaded[i] - after_alone[i]) as f64 / count as f64)
            .collect::<Vec<_>>(),
        S3Op::ALL.len(),
    );
    eprintln!(
        "AC11 slowest rounds alone {:?}, with searchers {:?}",
        &alone[alone.len().saturating_sub(5)..],
        &loaded[loaded.len().saturating_sub(5)..],
    );
    assert!(
        answered.load(Ordering::Relaxed) > 0,
        "no search was answered"
    );
}
