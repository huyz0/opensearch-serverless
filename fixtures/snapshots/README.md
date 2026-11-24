# Snapshot records the Java shell wrote

Written by `Gen.java` against the shell's own classes (`RepositoryDescriptor.toBytes`, `SnapshotRecord.toBytes`, and the state register's stream encoding as `SnapshotStore.encode` writes it), at `feature/serverlessshell`. Compile it against the shell's production classpath (see `fixtures/mixed-fleet/README.md`) and run `java -cp .:<classpath> Gen <dir>`.

| File | What it is |
|---|---|
| `repository.json` | a repository record with a boolean, a string and a number setting |
| `snapshot-finished.json` | a finished standard snapshot of one index with one captured shard |
| `snapshot-in-progress.json` | a shallow snapshot naming two indices, one with no captured shard |
| `state-finished.bin`, `state-in-progress.bin` | the register value for each (marker, record, finished flag) |

Tests: `crates/format/tests/snapshot_java_fixtures.rs`.

## What a Rust take wrote

`rust-take/state.bin` (the register value) and `rust-take/marker.json` (the protective marker) are what `a_take_writes_the_register_and_marker_a_java_node_reads` (`crates/node/tests/server/snapshot_take.rs`) wrote for a finished take of one two-shard index whose commits hold a whole, a packed and a node-packed file; rewrite them with `OSS_WRITE_FIXTURES=1`. `Check.java` reads them as a Java node does (`SnapshotRecord.fromStream`, the register's stream encoding, `CommitManifest.location(...).referencedBlob()`, a round trip through `toBytes`) and prints `PASS`; compile and run it as `Gen.java` is, with `java -cp .:<classpath> Check fixtures/snapshots/rust-take`. It was run against the shell at `feature/serverlessshell` when the fixtures were written.

## A Java take a Rust node restores

`java-take/state.bin` is the register value of a finished shallow snapshot `backups/java-1`, written by `GenTake.java` against the shell's own `SnapshotRecord` (one index `logs`, one shard at term 3: `_0.si` whole in `t=3`, `_0.cfs` packed in `t=3/pack#4:6`). `a_snapshot_a_java_node_took_is_restored_by_a_rust_node` (`crates/node/tests/server/snapshot_restore.rs`) places blobs for those locations and restores it. Compile and run `GenTake` as `Gen.java` is: `java -cp .:<classpath> GenTake java-take`.

## What a Rust restore wrote

`rust-restore/` holds the descriptor register value, shard 1's manifest and the new uuid of the index `copy` that `a_restore_writes_the_descriptor_and_manifests_a_java_node_reads` restored; rewrite them with `OSS_WRITE_FIXTURES=1`. `CheckRestore.java` reads them as a Java node does (`IndexDescriptor.fromStream`, `CommitManifest.fromStream`, `location(...).referencedBlob()`) and prints `PASS`: `java -cp .:<classpath> CheckRestore fixtures/snapshots/rust-restore`. Run against the shell at `feature/serverlessshell` (`e71d4233`) when the fixtures were written.

