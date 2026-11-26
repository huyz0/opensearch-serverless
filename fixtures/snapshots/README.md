# Snapshot records the Java shell wrote

Written by `Gen.java` against the shell's own classes (`RepositoryDescriptor.toBytes`, `SnapshotRecord.toBytes`, and the state register's stream encoding as `SnapshotStore.encode` writes it), at `feature/serverlessshell`. Compile it against the shell's production classpath (see `fixtures/mixed-fleet/README.md`) and run `java -cp .:<classpath> Gen <dir>`.

| File | What it is |
|---|---|
| `repository.json` | a repository record with a boolean, a string and a number setting |
| `snapshot-finished.json` | a finished standard snapshot of one index with one captured shard |
| `snapshot-in-progress.json` | a shallow snapshot naming two indices, one with no captured shard |
| `record-with-global-state.json` | a record the shell wrote with a `global_state` member (ADR 0068), carried through opaquely |
| `state-finished.bin`, `state-in-progress.bin` | the register value for each (marker, record, finished flag) |

Tests: `crates/format/tests/snapshot_java_fixtures.rs`.

## What a Rust take wrote

`rust-take/state.bin` (the register value) and `rust-take/marker.json` (the protective marker) are what `a_take_writes_the_register_and_marker_a_java_node_reads` (`crates/node/tests/server/snapshot_take.rs`) wrote for a finished take of one two-shard index whose commits hold a whole, a packed and a node-packed file; rewrite them with `OSS_WRITE_FIXTURES=1`. `Check.java` reads them as a Java node does (`SnapshotRecord.fromStream`, the register's stream encoding, `CommitManifest.location(...).referencedBlob()`, a round trip through `toBytes`) and prints `PASS`; compile and run it as `Gen.java` is, with `java -cp .:<classpath> Check fixtures/snapshots/rust-take`. It was run against the shell at `feature/serverlessshell` when the fixtures were written.

## A Java take a Rust node restores

`java-take/state.bin` is the register value of a finished shallow snapshot `backups/java-1`, written by `GenTake.java` against the shell's own `SnapshotRecord` (one index `logs`, one shard at term 3: `_0.si` whole in `t=3`, `_0.cfs` packed in `t=3/pack#4:6`). `a_snapshot_a_java_node_took_is_restored_by_a_rust_node` (`crates/node/tests/server/snapshot_restore.rs`) places blobs for those locations and restores it. Compile and run `GenTake` as `Gen.java` is: `java -cp .:<classpath> GenTake java-take`.

## What a Rust restore wrote

`rust-restore/` holds the descriptor register value, shard 1's manifest and the new uuid of the index `copy` that `a_restore_writes_the_descriptor_and_manifests_a_java_node_reads` restored; rewrite them with `OSS_WRITE_FIXTURES=1`. `CheckRestore.java` reads them as a Java node does (`IndexDescriptor.fromStream`, `CommitManifest.fromStream`, `location(...).referencedBlob()`) and prints `PASS`: `java -cp .:<classpath> CheckRestore fixtures/snapshots/rust-restore`. Run against the shell at `feature/serverlessshell` (`e71d4233`) when the fixtures were written.


`rust-restore/descriptor-marked.bin` is the descriptor of an index a restore has not finished (the mark `index.restore.in_progress` in its settings). `CheckRestore.java` reads it too: `IndexDescriptor.fromStream`, `routingMetadata()` and `toIndexMetadata(...)` all succeed and carry the mark as an ordinary setting, so a Java node does not fail on it and does not refuse the index either. Run against the shell at `e71d4233`: PASS.

## A Java node restores a Rust take

`ServerlessRustTakeRestoreTests.java` is a test for the shell's `:serverless:testkit` module (copy it to `serverless/testkit/src/test/java/org/opensearch/serverless/testkit/`). It restores, on a Java node, the shallow snapshot a Rust node took over the directory the Rust node wrote, and checks that the index is created. Write the directory with `OSS_WRITE_BUCKET=/tmp/rust-bucket cargo test -p oss-node --all-features --test server a_restored_index_answers` (the copy is taken after the take and before the Rust restore), then run `gt.sh :serverless:testkit:test --tests "*ServerlessRustTakeRestoreTests" -Dtests.rust.bucket=/tmp/rust-bucket -Dtests.security.manager=false` (the security manager refuses reads outside the test's own directories). Run against the shell at `feature/serverlessshell` (`e71d4233`): PASS, the Java restore read the Rust record, copied the shard's blobs and landed the manifest.

What it does not show: a Java writer opening that restored shard fails recovery with a `NullPointerException` in `TranslogHeader.read`, because a commit a Rust node created from nothing has no `translog_uuid` in its commit user data (a commit a Java node created first has one, and the Rust writer keeps what it finds). The mixed-fleet takeover test starts from a Java-created index, so it does not meet this. It is not a take or restore defect and is open as a finding for the writer.

## A Java node reads a Rust take's global state

`rust-take-global/state.bin` is the register value of a take with `include_global_state: true` over a state with four settings and one document in each of the five sets; `a_take_writes_the_global_state_record_a_java_node_reads` (`crates/node/tests/server/snapshot_global_take.rs`) rewrites it with `OSS_WRITE_FIXTURES=1`. `ServerlessRustGlobalStateTests.java` is a test for the shell's `:serverless:testkit` module (copy it to `serverless/testkit/src/test/java/org/opensearch/serverless/snapshot/`, the package of the shell's package-private `SnapshotGlobalState`): `gt.sh :serverless:testkit:test --tests "*ServerlessRustGlobalStateTests" -Dtests.rust.global=<repo>/fixtures/snapshots/rust-take-global -Dtests.security.manager=false`. Run against the shell at `256a2a6d`: 3 tests pass.

`ServerlessRustGlobalRestoreTests.java` (same module, package `org.opensearch.serverless.testkit`) goes the other way: a Java node restores the snapshot `with-state` of the bucket `OSS_WRITE_BUCKET=/tmp/rust-bucket cargo test -p oss-node --all-features --test server a_restored_index_answers` writes, with `include_global_state: true`, and reads the settings and the five sets back through its REST routes: `gt.sh :serverless:testkit:test --tests "*ServerlessRustGlobalRestoreTests" -Dtests.rust.bucket=/tmp/rust-bucket -Dtests.security.manager=false`. Run against the shell at `256a2a6d`: PASS, `global_state_restored: 7`.
