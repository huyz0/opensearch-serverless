# Snapshot records the Java shell wrote

Written by `Gen.java` against the shell's own classes (`RepositoryDescriptor.toBytes`, `SnapshotRecord.toBytes`, and the state register's stream encoding as `SnapshotStore.encode` writes it), at `feature/serverlessshell`. Compile it against the shell's production classpath (see `fixtures/mixed-fleet/README.md`) and run `java -cp .:<classpath> Gen <dir>`.

| File | What it is |
|---|---|
| `repository.json` | a repository record with a boolean, a string and a number setting |
| `snapshot-finished.json` | a finished standard snapshot of one index with one captured shard |
| `snapshot-in-progress.json` | a shallow snapshot naming two indices, one with no captured shard |
| `state-finished.bin`, `state-in-progress.bin` | the register value for each (marker, record, finished flag) |

Tests: `crates/format/tests/snapshot_java_fixtures.rs`.
