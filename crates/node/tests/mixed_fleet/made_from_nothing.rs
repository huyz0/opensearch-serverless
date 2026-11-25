//! A Java node takes over a shard a Rust node made from nothing.

use super::*;

#[test]
#[ignore = "needs RUSTFS_BIN, OSS_JAVA_BIN and OSS_JAVA_CP"]
fn a_java_node_takes_over_a_shard_a_rust_node_made_from_nothing() {
    if std::env::var_os("OSS_JAVA_CP").is_none() || std::env::var_os("OSS_JAVA_BIN").is_none() {
        eprintln!("skipped: OSS_JAVA_BIN and OSS_JAVA_CP are not set");
        return;
    }
    let Some(mut cluster) = Cluster::new(TTL_MS, 1, true) else {
        eprintln!("skipped: RUSTFS_BIN is not set");
        return;
    };
    let mut ledger = BTreeSet::new();

    // AC-10: Rust creates the index and acknowledges the first batch.
    let r1 = cluster.add("rust-1");
    let r1_addr = cluster.node(r1).addr();
    let made = call(
        r1_addr,
        "PUT",
        &format!("/{INDEX}"),
        Some("{\"settings\":{\"index\":{\"number_of_shards\":2}}}"),
    );
    assert_eq!(made.status, 200, "{}", made.text());
    write(r1_addr, 0..BATCH, &mut ledger);
    judge("AC-10 rust reads its own batch", r1_addr, &ledger);

    // AC-10: Rust is killed; a Java node takes over from Rust's commit.
    cluster.kill(r1);
    let j1 = JavaNode::start("java-1", &temp("rj1"), &cluster, false);
    write(j1.addr, BATCH..2 * BATCH, &mut ledger);
    judge("AC-10 java after taking over", j1.addr, &ledger);
    let r2 = cluster.add("rust-2");
    judge("AC-10 second rust", cluster.node(r2).addr(), &ledger);
    j1.kill();
}
