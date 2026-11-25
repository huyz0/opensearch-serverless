//! docs/specs/mixed-fleet-long-soak: the hand-over of `hand_over`, repeated
//! with a standby of the other kind always running, for as many cycles or
//! minutes as the environment allows. Ignored unless `OSS_JAVA_BIN` and
//! `OSS_JAVA_CP` are set; on RustFS when `RUSTFS_BIN` is set, else on the
//! emulator with latency.

use super::*;

/// A live node of either kind.
enum Live {
    Java(JavaNode),
    Rust(usize),
}

impl Live {
    fn addr(&self, cluster: &Cluster) -> SocketAddr {
        match self {
            Self::Java(j) => j.addr,
            Self::Rust(slot) => cluster.node(*slot).addr(),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Java(_) => "java",
            Self::Rust(_) => "rust",
        }
    }

    /// Stops the node as a killed process stops.
    fn kill(self, cluster: &mut Cluster) {
        match self {
            Self::Java(j) => j.kill(),
            Self::Rust(slot) => cluster.kill(slot),
        }
    }
}

/// A node of kind `kind` ("java" or "rust"), numbered `n`.
fn start(kind: &str, n: u64, cluster: &mut Cluster) -> Live {
    if kind == "java" {
        let name = format!("java-{n}");
        Live::Java(JavaNode::start(&name, &temp(&name), cluster, false))
    } else {
        Live::Rust(cluster.add(&format!("rust-{n}")))
    }
}

fn knob(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[test]
#[ignore = "needs OSS_JAVA_BIN and OSS_JAVA_CP (and RUSTFS_BIN for RustFS)"]
fn live_java_and_rust_nodes_hand_a_shard_over_for_many_cycles_without_losing_a_write() {
    if std::env::var_os("OSS_JAVA_CP").is_none() || std::env::var_os("OSS_JAVA_BIN").is_none() {
        eprintln!("skipped: OSS_JAVA_BIN and OSS_JAVA_CP are not set");
        return;
    }
    let cycles = knob("OSS_SOAK_CYCLES", 6).min(u64::from(MAX_FOUND / BATCH) - 2);
    let minutes = knob("OSS_SOAK_MINUTES", 0);
    let rustfs = std::env::var_os("RUSTFS_BIN").is_some();
    let mut cluster = Cluster::new(TTL_MS, 1, rustfs).expect("a store");
    let mut ledger = BTreeSet::new();
    let begun = Instant::now();

    let owner = start("java", 1, &mut cluster);
    let made = call(
        owner.addr(&cluster),
        "PUT",
        &format!("/{INDEX}"),
        Some(r#"{"settings":{"index":{"number_of_shards":2}}}"#),
    );
    assert_eq!(made.status, 200, "{}", made.text());
    let mut next = BATCH;
    write(owner.addr(&cluster), 0..BATCH, &mut ledger);
    let standby = start("rust", 1, &mut cluster);
    judge(
        "start: standby finds the first batch",
        standby.addr(&cluster),
        &ledger,
    );
    let (mut owner, mut standby) = (owner, standby);
    let mut takeovers: Vec<(&'static str, u128)> = Vec::new();
    let lists_before = (!rustfs).then(|| cluster.sent(oss_blob::S3Op::List));
    let mut ran = 0;

    for cycle in 1..=cycles {
        if minutes > 0 && begun.elapsed() >= Duration::from_secs(minutes * 60) {
            break;
        }
        let killed = owner.kind();
        owner.kill(&mut cluster);
        let t = Instant::now();
        write(standby.addr(&cluster), next..next + BATCH, &mut ledger);
        next += BATCH;
        let took = t.elapsed().as_millis();
        let taker = standby.kind();
        takeovers.push((taker, took));
        // The killed kind comes back as the next standby.
        let fresh = start(killed, cycle + 1, &mut cluster);
        judge("taker after the hand-over", standby.addr(&cluster), &ledger);
        judge(
            "replacement after the hand-over",
            fresh.addr(&cluster),
            &ledger,
        );
        eprintln!(
            "SOAK cycle={cycle} killed={killed} taker={taker} first_ack_ms={took} ledger={}",
            ledger.len()
        );
        owner = standby;
        standby = fresh;
        ran = cycle;
    }

    assert!(ran >= 1, "no cycle ran");
    // AC-2 is the bound `write` enforces; AC-3 compares the ends of the run.
    let by_kind = |kind: &str| {
        let mut v: Vec<u128> = takeovers
            .iter()
            .filter(|(k, _)| *k == kind)
            .map(|(_, ms)| *ms)
            .collect();
        v.sort_unstable();
        v
    };
    for kind in ["java", "rust"] {
        let v = by_kind(kind);
        if let (Some(min), Some(max)) = (v.first(), v.last()) {
            eprintln!(
                "SOAK taker={kind} n={} min={min}ms median={}ms max={max}ms",
                v.len(),
                v[v.len() / 2]
            );
        }
    }
    if takeovers.len() >= 6 {
        let third = takeovers.len() / 3;
        let max = |s: &[(&str, u128)]| s.iter().map(|(_, ms)| *ms).max().unwrap_or(0);
        let (first, last) = (
            max(&takeovers[..third]),
            max(&takeovers[takeovers.len() - third..]),
        );
        assert!(
            last <= 2 * first + u128::from(TTL_MS),
            "take-over drifted: slowest of the first third {first} ms, of the last {last} ms"
        );
    }
    if let Some(before) = lists_before {
        let lists = cluster.sent(oss_blob::S3Op::List) - before;
        eprintln!("SOAK lists={lists} cycles={ran}");
    }
    eprintln!(
        "SOAK done cycles={ran} acknowledged={} elapsed_s={}",
        ledger.len(),
        begun.elapsed().as_secs()
    );
    owner.kill(&mut cluster);
    standby.kill(&mut cluster);
}
