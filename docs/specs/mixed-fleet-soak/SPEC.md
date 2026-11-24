# Mixed Java/Rust fleet: a live takeover soak

## Serves

Parity plan R15. The strangler runs Rust nodes beside the Java serverless shell on one bucket. [mixed-takeover](../../../fixtures/mixed-takeover/README.md) shows each side reading what the other left, on recorded bytes. This runs both as live processes on one store, with kills between hand-overs, and judges every acknowledged write.

## Change

A test, `crates/node/tests/mixed_fleet.rs`, ignored unless `RUSTFS_BIN`, `OSS_JAVA_CP` (the shell's production classpath) and `OSS_JAVA_BIN` are set. It starts RustFS, forks real Java shell nodes (`ServerlessBootstrap`) and boots Rust nodes through the suite rig, all on one bucket, and drives them over HTTP. A ledger records each document the moment a node acknowledged it.

## Acceptance criteria

- AC-1: a Java node creates the index and acknowledges the first batch; a Rust node, started after, finds every one of those documents.
- AC-2: the Java node is killed (SIGKILL); a Rust node takes the shard over and acknowledges a second batch; it still finds every document of the first.
- AC-3: a second Java node, started after, finds every document of both batches.
- AC-4: the Rust node is killed; the second Java node takes the shard over and acknowledges a third batch; a second Rust node finds all three batches, and the Java node finds the same count.
- AC-7: AC-1 to AC-4 hold for a one-shard index with the buckets switch on in every node (the Java node with `serverless.buckets.enabled`, the Rust nodes with `OSS_BUCKETS_ENABLED`), so the index lives in a bucket; the Java node also finds its own batch.
- AC-5: no acknowledged document is missing and none is found twice at any step; a run that loses one fails and names its id.
- AC-6: the writes of a hand-over are acknowledged within the lease TTL plus a bound the test states, so a node that cannot take over fails the run rather than waiting forever.

## S3 request budget

None added: the test only observes. The nodes' own counts are unchanged.

## Switch

None. A test.
