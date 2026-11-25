//! docs/specs/snapshots-default-on: a Rust node with the take on snapshots an
//! index a Java node writes, deletes the index, a Java node and the Rust
//! node restore the snapshot and find what it captured, and the Rust node deletes it.

use super::*;

/// The total hits of a match-all search of `index` on the node at `addr`,
/// retried while the index is being activated.
fn total_hits(addr: SocketAddr, index: &str) -> u64 {
    let started = Instant::now();
    loop {
        let r = call(
            addr,
            "POST",
            &format!("/{index}/_search"),
            Some(r#"{"size":0,"track_total_hits":true}"#),
        );
        let text = r.text();
        let total = text
            .split("\"total\":{\"value\":")
            .nth(1)
            .map(|rest| {
                rest.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
            })
            .and_then(|n| n.parse().ok());
        if let (200, Some(n)) = (r.status, total) {
            return n;
        }
        assert!(
            started.elapsed() < Duration::from_secs(HANDOVER_SLACK_SECS),
            "{index}: {} {text}",
            r.status
        );
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[test]
#[ignore = "needs RUSTFS_BIN, OSS_JAVA_BIN and OSS_JAVA_CP"]
fn a_snapshot_taken_beside_a_java_writer_survives_the_index_and_restores_on_java() {
    if std::env::var_os("OSS_JAVA_CP").is_none() || std::env::var_os("OSS_JAVA_BIN").is_none() {
        eprintln!("skipped: OSS_JAVA_BIN and OSS_JAVA_CP are not set");
        return;
    }
    let Some(mut cluster) = Cluster::new(TTL_MS, 1, true) else {
        eprintln!("skipped: RUSTFS_BIN is not set");
        return;
    };
    cluster.set("OSS_SNAPSHOTS_TAKE_ENABLED", "true");
    cluster.set("OSS_SNAPSHOTS_RESTORE_ENABLED", "true");
    let mut ledger = BTreeSet::new();

    // A Java node creates the index and writes; a Rust node with the take on reads it.
    let j1 = JavaNode::start("java-1", &temp("snap-j1"), &cluster, false);
    let made = call(
        j1.addr,
        "PUT",
        &format!("/{INDEX}"),
        Some(r#"{"settings":{"index":{"number_of_shards":2}}}"#),
    );
    assert_eq!(made.status, 200, "{}", made.text());
    write(j1.addr, 0..BATCH, &mut ledger);
    let r1 = cluster.add("rust-1");
    let rust = cluster.node(r1).addr();
    judge("rust finds java's batch", rust, &ledger);

    // The Rust node takes a shallow snapshot of it.
    let repo =
        r#"{"type":"fs","settings":{"location":"x","remote_store_index_shallow_copy":true}}"#;
    let registered = call(rust, "PUT", "/_snapshot/backups", Some(repo));
    assert_eq!(registered.status, 200, "{}", registered.text());
    let started = Instant::now();
    loop {
        let taken = call(
            rust,
            "PUT",
            "/_snapshot/backups/s1?wait_for_completion=true",
            Some(&format!("{{\"indices\":\"{INDEX}\"}}")),
        );
        if taken.status == 200 {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(HANDOVER_SLACK_SECS),
            "the take: {} {}",
            taken.status,
            taken.text()
        );
        std::thread::sleep(Duration::from_millis(500));
    }

    // The index is deleted by the Rust node, which reads the snapshots first.
    let deleted = call(rust, "DELETE", &format!("/{INDEX}"), None);
    assert_eq!(deleted.status, 200, "{}", deleted.text());
    j1.kill();

    // A Java node restores the Rust node's snapshot and finds what it captured.
    let j2 = JavaNode::start("java-2", &temp("snap-j2"), &cluster, false);
    let described = call(j2.addr, "GET", "/_snapshot/backups/s1", None);
    assert_eq!(described.status, 200, "{}", described.text());
    let restored = call(
        j2.addr,
        "POST",
        "/_snapshot/backups/s1/_restore?wait_for_completion=true",
        Some(&format!(
            "{{\"rename_pattern\":\"{INDEX}\",\"rename_replacement\":\"copy\"}}"
        )),
    );
    assert_eq!(restored.status, 200, "{}", restored.text());
    assert_eq!(
        total_hits(j2.addr, "copy"),
        u64::from(BATCH),
        "the restore holds what the snapshot captured"
    );

    // The Rust node restores it too, over what the Java writer left, and finds the same.
    let again = call(
        rust,
        "POST",
        "/_snapshot/backups/s1/_restore?wait_for_completion=true",
        Some(&format!(
            "{{\"rename_pattern\":\"{INDEX}\",\"rename_replacement\":\"rustcopy\"}}"
        )),
    );
    assert_eq!(again.status, 200, "{}", again.text());
    assert_eq!(total_hits(rust, "rustcopy"), u64::from(BATCH));

    // The Rust node deletes the snapshot; a Java node reads it as gone.
    let removed = call(rust, "DELETE", "/_snapshot/backups/s1", None);
    assert_eq!(removed.status, 200, "{}", removed.text());
    let gone = call(j2.addr, "GET", "/_snapshot/backups/s1", None);
    assert_eq!(gone.status, 404, "{}", gone.text());
    j2.kill();
}
