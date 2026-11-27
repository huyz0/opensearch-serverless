//! docs/specs/reader-warm-copy AC-6: a Rust node joining beside a Java shell
//! node, with the copy on. The Java shell does not know Rust's warm actions,
//! so it must answer with an error and not a silence: the joiner is ready
//! well inside the hot-set timeout, and reads what Java wrote.

use super::*;

/// The hot-set timeout of a joiner (`oss_node::warm_join::HOT_SET_TIMEOUT`):
/// a peer that stayed silent would hold the joiner this long.
const SILENCE: Duration = Duration::from_secs(5);

#[test]
#[ignore = "needs OSS_JAVA_BIN and OSS_JAVA_CP"]
fn a_rust_joiner_beside_a_java_peer_is_ready_promptly_and_reads_what_java_wrote() {
    if std::env::var_os("OSS_JAVA_CP").is_none() || std::env::var_os("OSS_JAVA_BIN").is_none() {
        eprintln!("skipped: OSS_JAVA_BIN and OSS_JAVA_CP are not set");
        return;
    }
    // RustFS when RUSTFS_BIN is set, the emulator otherwise.
    let Some(mut cluster) = Cluster::new(TTL_MS, 1, std::env::var_os("RUSTFS_BIN").is_some())
    else {
        eprintln!("skipped: no store");
        return;
    };
    let tier = temp("warm-peer-tier");
    cluster.set("OSS_PAGE_CACHE_DIR", &tier.display().to_string());
    cluster.set("OSS_PAGE_CACHE_BYTES", "67108864");
    cluster.set("OSS_WARM_ON_JOIN", "true");
    cluster.set("OSS_WARM_MAX_WAIT_SECONDS", "60");
    let mut ledger = BTreeSet::new();

    let java = JavaNode::start("java-1", &temp("warm-peer-j1"), &cluster, false);
    let made = call(
        java.addr,
        "PUT",
        &format!("/{INDEX}"),
        Some(r#"{"settings":{"index":{"number_of_shards":1}}}"#),
    );
    assert_eq!(made.status, 200, "{}", made.text());
    write(java.addr, 0..BATCH, &mut ledger);
    judge("java reads its own batch", java.addr, &ledger);

    let started = Instant::now();
    let slot = cluster.add("rust-1");
    let addr = cluster.node(slot).addr();
    let ready = loop {
        if call(addr, "GET", "/_serverless/ready", None).status == 200 {
            break started.elapsed();
        }
        assert!(
            started.elapsed() < Duration::from_secs(70),
            "the joiner was never ready"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    eprintln!(
        "WARM-PEER rust joiner ready {} ms after its start",
        ready.as_millis()
    );
    assert!(
        ready < SILENCE.saturating_sub(Duration::from_secs(1)),
        "ready after {ready:?}: the Java peer held the joiner as a silent peer does"
    );
    judge("rust finds java's batch", addr, &ledger);
}

#[test]
#[ignore = "needs OSS_JAVA_BIN and OSS_JAVA_CP"]
fn a_java_joiner_beside_a_rust_peer_is_ready_promptly_and_reads_what_rust_wrote() {
    if std::env::var_os("OSS_JAVA_CP").is_none() || std::env::var_os("OSS_JAVA_BIN").is_none() {
        eprintln!("skipped: OSS_JAVA_BIN and OSS_JAVA_CP are not set");
        return;
    }
    let Some(mut cluster) = Cluster::new(TTL_MS, 1, std::env::var_os("RUSTFS_BIN").is_some())
    else {
        eprintln!("skipped: no store");
        return;
    };
    let tier = temp("warm-java-joiner-tier");
    cluster.set("OSS_PAGE_CACHE_DIR", &tier.display().to_string());
    cluster.set("OSS_PAGE_CACHE_BYTES", "67108864");
    cluster.set("OSS_WARM_ON_JOIN", "true");
    let mut ledger = BTreeSet::new();

    let slot = cluster.add("rust-1");
    let rust = cluster.node(slot).addr();
    let made = call(
        rust,
        "PUT",
        &format!("/{INDEX}"),
        Some(r#"{"settings":{"index":{"number_of_shards":1}}}"#),
    );
    assert_eq!(made.status, 200, "{}", made.text());
    write(rust, 0..BATCH, &mut ledger);
    judge("rust reads its own batch", rust, &ledger);

    let java = JavaNode::start_with(
        "java-1",
        &temp("warm-java-joiner-j1"),
        &cluster,
        false,
        &[("serverless.search.warm_on_join", "true")],
    );
    // The fork takes seconds; the wait that matters begins when its HTTP
    // port answers, which a peer that stayed silent would stretch by its
    // whole hot-set timeout.
    let started = Instant::now();
    let ready = loop {
        if call(java.addr, "GET", "/_serverless/ready", None).status == 200 {
            break started.elapsed();
        }
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "the Java joiner was never ready"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    eprintln!(
        "WARM-PEER java joiner ready {} ms after its HTTP port answered",
        ready.as_millis()
    );
    assert!(
        ready < Duration::from_secs(30),
        "ready after {ready:?}: a Rust peer that answers the Java action with an error should not hold the joiner"
    );
    judge("java finds rust's batch", java.addr, &ledger);
}
