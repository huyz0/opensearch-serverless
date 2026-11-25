# Mixed Java/Rust fleet: several nodes, concurrent writers, a kill each cycle

## Serves

Parity plan R15, the Java-retire gate. [mixed-fleet-long-soak](../mixed-fleet-long-soak/SPEC.md) hands one shard-set over between two live nodes with one writer. This runs two Java and two Rust nodes (counts set by `OSS_SOAK_JAVA` and `OSS_SOAK_RUST`) on one bucket with four writers at once, two aimed at each kind, and kills one node a cycle while the writers go on.

## Change

A test, `crates/node/tests/mixed_fleet/soak_wide.rs` (a module of `mixed_fleet.rs`, ignored unless `OSS_JAVA_BIN` and `OSS_JAVA_CP` are set; RustFS when `RUSTFS_BIN` is). A four-shard index. Each writer sends bulks of 20 documents, with new ids, to the live nodes of its kind (and to the others when a try fails), retrying a bulk with the same ids until it is acknowledged, and a fully acknowledged bulk goes into the ledger. Each cycle: the writers run for a window (`OSS_SOAK_WINDOW_SECONDS`, default 8), one node is killed (SIGKILL) and a fresh node of the same kind starts, the writers run for another window, then they pause between bulks and every live node is judged against the ledger by id range. A failure prints what a write to every shard does for the node that lacked documents.

## Acceptance criteria

- AC-1: after each cycle every live node finds every acknowledged document and no other, whichever kind it is and whichever kind owned the shard.
- AC-2: a bulk is acknowledged within the lease TTL plus the bound of [mixed-fleet-soak](../mixed-fleet-soak/SPEC.md) AC-6, by some node, whichever kind it is sent to; a bulk that is not fails the run.
- AC-3: the run prints, by kind of the node a bulk was first sent to, how many bulks were acknowledged on the first try, and each cycle's slowest acknowledgement, so a kind that cannot take writes shows.

## S3 request budget

None added: the test only observes.

## Switch

None. A test.
