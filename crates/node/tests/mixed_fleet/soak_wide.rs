//! docs/specs/mixed-fleet-wide-soak: several live Java and Rust nodes on one
//! bucket, concurrent writers aimed at each kind, one node killed and
//! replaced a cycle while the writers go on, every acknowledged document
//! judged on every live node after each cycle. Ignored unless
//! `OSS_JAVA_BIN` and `OSS_JAVA_CP` are set; on RustFS when `RUSTFS_BIN` is.

use super::*;

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Documents in one bulk; the ids of a bulk are kept across its retries.
const PER_BULK: u32 = 20;
/// Ids judged in one range query, under the result window.
const RANGE: u32 = 5_000;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Java,
    Rust,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Java => "java",
            Self::Rust => "rust",
        }
    }
}

/// A live node: what the writers need, and what kills it.
enum Handle {
    Java(JavaNode),
    Rust(usize),
}

fn knob(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn spawn(kind: Kind, n: u64, cluster: &mut Cluster) -> (Handle, SocketAddr) {
    match kind {
        Kind::Java => {
            let name = format!("java-{n}");
            let node = JavaNode::start(&name, &temp(&name), cluster, false);
            let addr = node.addr;
            (Handle::Java(node), addr)
        }
        Kind::Rust => {
            let slot = cluster.add(&format!("rust-{n}"));
            (Handle::Rust(slot), cluster.node(slot).addr())
        }
    }
}

/// Ids of `INDEX` with `n` in `lo..hi`, as the node at `addr` finds them.
fn found_range(addr: SocketAddr, lo: u32, hi: u32) -> Result<Vec<u32>, String> {
    let body = format!(
        "{{\"size\":{MAX_FOUND},\"_source\":false,\"query\":{{\"range\":{{\"n\":{{\"gte\":{lo},\"lt\":{hi}}}}}}}}}"
    );
    let r = call(addr, "POST", &format!("/{INDEX}/_search"), Some(&body));
    if r.status != 200 {
        // A node just started may not serve search yet: judged again within the slack.
        return Err(format!("search answered {}: {}", r.status, r.text()));
    }
    let text = r.text();
    if let Some(shards) = text.split("\"_shards\":").nth(1) {
        let failed: u32 = shards
            .split("\"failed\":")
            .nth(1)
            .map(|rest| {
                rest.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
            })
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        if failed > 0 {
            // Not lost documents: a shard the node refused to answer for.
            let why: String = shards.chars().take(400).collect();
            return Err(format!("PARTIAL answer, {failed} shard(s) failed: {why}"));
        }
    }
    Ok(r.text()
        .split("\"_id\":")
        .skip(1)
        .map(|rest| {
            rest.trim_start()
                .trim_start_matches('"')
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .expect("a numeric id")
        })
        .collect())
}

/// Prints, for each shard of `INDEX`, the head's owner and its claim, and the
/// lease of the owner and of each node the head names, as Java reads them.
fn probe(store: &oss_blob::S3Store, shards: u32) {
    use oss_blob::BlobStore;
    let read = |key: &str| {
        let mut buf = vec![0u8; 1 << 16];
        let got = store.read_register(key, &mut buf).ok().flatten()?;
        buf.get(..got.len).map(<[u8]>::to_vec)
    };
    let lease = |id: &str| -> String {
        let key = oss_format::lease_key("", id).expect("a key");
        match read(&key).and_then(|b| oss_format::NodeLease::parse(&b).ok()) {
            Some(l) => format!(
                "lease {id}: ephemeral={} expires={} revoked={} version={:?} epochs={:?}",
                l.ephemeral_id, l.expires_at_millis, l.revoked, l.version, l.node_log
            ),
            None => format!("lease {id}: none"),
        }
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    eprintln!("WIDE probe now={now}");
    for shard in 0..shards {
        let key = oss_cluster::heads::shard_head_key("", INDEX, shard);
        let Some(head) = read(&key).and_then(|b| oss_format::ShardHead::parse(&b).ok()) else {
            eprintln!("WIDE probe shard {shard}: no head");
            continue;
        };
        eprintln!(
            "WIDE probe shard {shard}: term={} owner={:?} ephemeral={:?} claim_expires={} writers={:?}",
            head.term,
            head.owner_node_id,
            head.owner_ephemeral_id,
            head.lease_expires_at_millis,
            head.writers
        );
        let mut ids: Vec<String> = head.writers.clone();
        if let Some(owner) = &head.owner_node_id {
            if !ids.contains(owner) {
                ids.push(owner.clone());
            }
        }
        for id in ids {
            eprintln!("WIDE probe   {}", lease(&id));
        }
    }
}

/// Whether the node at `addr` finds exactly the ledger, range by range, within the slack.
fn judge_all(addr: SocketAddr, ledger: &BTreeSet<u32>, top: u32) -> Result<(), String> {
    let started = Instant::now();
    loop {
        let _ = call(addr, "POST", &format!("/{INDEX}/_refresh"), None);
        let mut bad = None;
        let mut lo = 0;
        while lo < top && bad.is_none() {
            let hi = (lo + RANGE).min(top);
            let ids = match found_range(addr, lo, hi) {
                Ok(ids) => ids,
                Err(why) => {
                    bad = Some(why);
                    break;
                }
            };
            let set: BTreeSet<u32> = ids.iter().copied().collect();
            let want: BTreeSet<u32> = ledger.range(lo..hi).copied().collect();
            if set != want || ids.len() != want.len() {
                let missing: Vec<_> = want.difference(&set).take(10).copied().collect();
                let extra: Vec<_> = set.difference(&want).take(10).copied().collect();
                bad = Some(format!(
                    "ids {lo}..{hi}: ledger {} found {} (missing {missing:?}, unexpected {extra:?})",
                    want.len(),
                    ids.len()
                ));
            }
            lo = hi;
        }
        let Some(why) = bad else { return Ok(()) };
        if started.elapsed() >= Duration::from_secs(HANDOVER_SLACK_SECS) {
            return Err(why);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Writes `ids` through the node at `addr`, retrying like the writers do.
fn write_ids(addr: SocketAddr, ids: std::ops::Range<u32>, ledger: &mut BTreeSet<u32>) {
    write(addr, ids, ledger);
}

/// What the writers share.
struct Shared {
    nodes: Mutex<Vec<(Kind, SocketAddr)>>,
    ledger: Mutex<BTreeSet<u32>>,
    next: AtomicU32,
    paused: AtomicBool,
    stop: AtomicBool,
    idle: [AtomicBool; 4],
    failed: Mutex<Option<String>>,
    /// Per target kind: bulks tried and acknowledged first time.
    tried: [AtomicU64; 2],
    first_try: [AtomicU64; 2],
    max_ack_ms: AtomicU64,
    retries: AtomicU64,
    /// The first replies that were not an acknowledgement, by target kind.
    refusals: Mutex<Vec<String>>,
}

fn writer(me: usize, kind: Kind, s: &Shared) {
    let mut turn = me;
    while !s.stop.load(Ordering::Acquire) {
        s.idle[me].store(false, Ordering::Release);
        if s.paused.load(Ordering::Acquire) {
            s.idle[me].store(true, Ordering::Release);
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        let first = s.next.fetch_add(PER_BULK, Ordering::AcqRel);
        let mut body = String::new();
        for id in first..first + PER_BULK {
            body.push_str(&format!(
                "{{\"index\":{{\"_index\":\"{INDEX}\",\"_id\":\"{id}\"}}}}\n{{\"n\":{id}}}\n"
            ));
        }
        let started = Instant::now();
        let mut attempt = 0_u32;
        loop {
            let nodes = s.nodes.lock().expect("nodes").clone();
            let mine: Vec<_> = nodes.iter().filter(|(k, _)| *k == kind).collect();
            let pool = if attempt == 0 && !mine.is_empty() {
                mine
            } else {
                nodes.iter().collect()
            };
            turn += 1;
            let Some((target_kind, addr)) = pool.get(turn % pool.len().max(1)).map(|n| **n) else {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            };
            let idx = usize::from(target_kind == Kind::Rust);
            if attempt == 0 {
                s.tried[idx].fetch_add(1, Ordering::AcqRel);
            }
            let reply = try_call(addr, "POST", "/_bulk?refresh=true", Some(&body));
            let ok = reply
                .as_ref()
                .is_some_and(|r| r.status == 200 && r.text().contains("\"errors\":false"));
            if !ok {
                let what = reply.as_ref().map_or_else(
                    || "no complete reply".to_owned(),
                    |r| {
                        format!(
                            "{} {}",
                            r.status,
                            r.text().chars().take(1500).collect::<String>()
                        )
                    },
                );
                let mut seen = s.refusals.lock().expect("refusals");
                let line = format!("{} node, try {attempt}: {what}", target_kind.name());
                if seen.len() < 40 && !seen.iter().any(|l| l == &line) {
                    seen.push(line);
                }
            }
            if ok {
                if attempt == 0 {
                    s.first_try[idx].fetch_add(1, Ordering::AcqRel);
                }
                s.ledger
                    .lock()
                    .expect("ledger")
                    .extend(first..first + PER_BULK);
                let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                s.max_ack_ms.fetch_max(ms, Ordering::AcqRel);
                break;
            }
            attempt += 1;
            s.retries.fetch_add(1, Ordering::AcqRel);
            if started.elapsed()
                > Duration::from_millis(TTL_MS) + Duration::from_secs(HANDOVER_SLACK_SECS)
            {
                *s.failed.lock().expect("failed") =
                    Some(format!("bulk of {first}.. not acknowledged in time"));
                s.stop.store(true, Ordering::Release);
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    s.idle[me].store(true, Ordering::Release);
}

#[test]
#[ignore = "needs OSS_JAVA_BIN and OSS_JAVA_CP (and RUSTFS_BIN for RustFS)"]
fn several_live_nodes_take_concurrent_writes_while_one_is_killed_each_cycle() {
    if std::env::var_os("OSS_JAVA_CP").is_none() || std::env::var_os("OSS_JAVA_BIN").is_none() {
        eprintln!("skipped: OSS_JAVA_BIN and OSS_JAVA_CP are not set");
        return;
    }
    let cycles = knob("OSS_SOAK_CYCLES", 8);
    let minutes = knob("OSS_SOAK_MINUTES", 0);
    let java = knob("OSS_SOAK_JAVA", 2);
    let rust = knob("OSS_SOAK_RUST", 2);
    let window = Duration::from_secs(knob("OSS_SOAK_WINDOW_SECONDS", 8));
    let mut cluster =
        Cluster::new(TTL_MS, 1, std::env::var_os("RUSTFS_BIN").is_some()).expect("a store");
    let probe_store = {
        let (endpoint, bucket, region, access, secret) = cluster.store();
        oss_blob::S3Store::new(oss_blob::S3Config::new(
            &endpoint,
            bucket,
            region,
            oss_blob::Credentials::new(access, secret),
        ))
        .expect("a probe store")
    };
    let shared = Shared {
        nodes: Mutex::new(Vec::new()),
        ledger: Mutex::new(BTreeSet::new()),
        next: AtomicU32::new(0),
        paused: AtomicBool::new(false),
        stop: AtomicBool::new(false),
        idle: [(); 4].map(|()| AtomicBool::new(false)),
        failed: Mutex::new(None),
        tried: [AtomicU64::new(0), AtomicU64::new(0)],
        first_try: [AtomicU64::new(0), AtomicU64::new(0)],
        max_ack_ms: AtomicU64::new(0),
        retries: AtomicU64::new(0),
        refusals: Mutex::new(Vec::new()),
    };
    let mut live: Vec<(Kind, Handle, SocketAddr)> = Vec::new();
    let mut counter = 0_u64;
    for (kind, count) in [(Kind::Java, java), (Kind::Rust, rust)] {
        for _ in 0..count {
            counter += 1;
            let (handle, addr) = spawn(kind, counter, &mut cluster);
            live.push((kind, handle, addr));
        }
    }
    let first_addr = live[0].2;
    let made = call(
        first_addr,
        "PUT",
        &format!("/{INDEX}"),
        Some(r#"{"settings":{"index":{"number_of_shards":4}}}"#),
    );
    assert_eq!(made.status, 200, "{}", made.text());
    shared
        .nodes
        .lock()
        .expect("nodes")
        .extend(live.iter().map(|(k, _, a)| (*k, *a)));
    let begun = Instant::now();
    let mut ran = 0;
    std::thread::scope(|scope| {
        for (me, kind) in [Kind::Java, Kind::Rust, Kind::Java, Kind::Rust]
            .into_iter()
            .enumerate()
        {
            let shared = &shared;
            scope.spawn(move || writer(me, kind, shared));
        }
        for cycle in 1..=cycles {
            if (minutes > 0 && begun.elapsed() >= Duration::from_secs(minutes * 60))
                || shared.stop.load(Ordering::Acquire)
            {
                break;
            }
            std::thread::sleep(window);
            // Kill one node while the writers go on, and bring a fresh one.
            let at = usize::try_from(cycle).unwrap_or(0) % live.len();
            let (kind, handle, addr) = live.remove(at);
            shared
                .nodes
                .lock()
                .expect("nodes")
                .retain(|(_, a)| *a != addr);
            match handle {
                Handle::Java(j) => j.kill(),
                Handle::Rust(slot) => cluster.kill(slot),
            }
            counter += 1;
            let (handle, addr) = spawn(kind, counter, &mut cluster);
            shared.nodes.lock().expect("nodes").push((kind, addr));
            live.insert(at, (kind, handle, addr));
            std::thread::sleep(window);
            if knob("OSS_SOAK_NUDGE", 0) > 0 {
                // A write to every shard makes a node take each one over, as
                // a Java reader needs after a Rust owner dies.
                let first = shared.next.fetch_add(40, Ordering::AcqRel);
                let mut done = BTreeSet::new();
                write(live[0].2, first..first + 40, &mut done);
                shared.ledger.lock().expect("ledger").extend(done);
            }
            // Stop the writers between bulks, then judge every live node.
            shared.paused.store(true, Ordering::Release);
            while !shared.idle.iter().all(|i| i.load(Ordering::Acquire))
                && !shared.stop.load(Ordering::Acquire)
            {
                std::thread::sleep(Duration::from_millis(50));
            }
            if shared.stop.load(Ordering::Acquire) {
                break;
            }
            let ledger = shared.ledger.lock().expect("ledger").clone();
            let top = shared.next.load(Ordering::Acquire);
            let mut errors = Vec::new();
            for (k, _, a) in &live {
                if let Err(why) = judge_all(*a, &ledger, top) {
                    errors.push(format!("cycle {cycle}, a {} node at {a}: {why}", k.name()));
                }
            }
            if !errors.is_empty() {
                probe(&probe_store, 4);
                // Does a write to every shard, which makes a node take each
                // one over, make the missing documents visible?
                let java_addr = live
                    .iter()
                    .find(|(k, _, _)| *k == Kind::Java)
                    .map(|(_, _, a)| *a);
                if let Some(a) = java_addr {
                    let mut nudged = ledger.clone();
                    let base = top + 1_000;
                    write_ids(a, base..base + 40, &mut nudged);
                    let after = judge_all(a, &nudged, base + 40);
                    eprintln!("WIDE diag after a write to every shard: {after:?}");
                }
                shared.stop.store(true, Ordering::Release);
                *shared.failed.lock().expect("failed") = Some(errors.join("; "));
                break;
            }
            let t = |i: usize| {
                (
                    shared.first_try[i].load(Ordering::Acquire),
                    shared.tried[i].load(Ordering::Acquire),
                )
            };
            eprintln!(
                "WIDE cycle={cycle} killed={} acked={} first_try_ok java={:?} rust={:?} retries={} max_ack_ms={}",
                kind.name(),
                ledger.len(),
                t(0),
                t(1),
                shared.retries.load(Ordering::Acquire),
                shared.max_ack_ms.swap(0, Ordering::AcqRel)
            );
            ran = cycle;
            shared.paused.store(false, Ordering::Release);
        }
        shared.stop.store(true, Ordering::Release);
    });
    for line in shared.refusals.lock().expect("refusals").iter() {
        eprintln!("WIDE refusal {line}");
    }
    if let Some(why) = shared.failed.lock().expect("failed").take() {
        panic!("{why}");
    }
    assert!(ran >= 1, "no cycle ran");
    eprintln!(
        "WIDE done cycles={ran} acknowledged={} elapsed_s={}",
        shared.ledger.lock().expect("ledger").len(),
        begun.elapsed().as_secs()
    );
    for (_, handle, _) in live {
        match handle {
            Handle::Java(j) => j.kill(),
            Handle::Rust(slot) => cluster.kill(slot),
        }
    }
}
