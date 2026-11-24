# 0042: Snapshots are taken shallow first, in the Java's protocol, behind the switch

**Status:** Proposed, 2026-11-24 (awaiting independent review)

## Context

[ADR 0038](0038-snapshots-read-first.md) made snapshots readable and [ADR 0040](0040-repository-registration.md) made repositories writable. Taking a snapshot is the next piece: `PUT`/`POST /_snapshot/{repo}/{name}` in the shell's `SnapshotOperations.capture` and `SnapshotStore.create/finish`. A snapshot is a name register (`snapshot-states/<sha256 of "<repo>#<name>">`, a compare-and-swap value), a discovery marker blob under `snapshots/`, and a record naming each captured index's uuid, shard count, mapping, settings and each shard's commit (a term and the files in it). A standard snapshot also copies every shard blob under `snapshot-data-v2/<generation>/...`; a shallow one only names the live blobs, which the janitor already holds for any index a snapshot names ([janitor-snapshot-pins](../specs/janitor-snapshot-pins/SPEC.md)).

## Decision

- **Shallow first.** A repository whose `remote_store_index_shallow_copy` setting is true (the shell's `shallowByDefault`, the only selector the Java has) takes shallow snapshots: no shard data moves, so a take costs requests that scale with indices and shards, not bytes. A standard repository answers 501 `unsupported_snapshot_storage` with the reason, until a later ADR decides the copy (S3 COPYs scale with files, and need their own budget and crash tests).
- **The Java's protocol, exactly.** (1) Read the repository (404 `repository_missing_exception`). (2) Describe every named index (404 `index_not_found`; `ignore_unavailable` skips; a system index answers 403 `system_index`). (3) Create the name's state register with a reservation record that names each index (uuid, shard count, mapping, settings) and no commits, under a fresh storage generation and marker `v2-<sha256 repo>#<sha256 key>#<generation>`, by `create_register` when the name has none and a compare-and-swap over a released one, retried up to 16 times; a held name answers 400 `snapshot_already_exists`. (4) Write the protective marker blob and re-read the register to confirm it still holds this marker; a failure releases the register and removes the marker. (5) Verify each index's uuid is still the described one (the Java's `verifyCaptureIncarnation`); a changed uuid fails the take. (6) Read each shard's published commit: a shard with none is 409 `nothing_published`, or with `partial` that index is skipped. (7) Finish by a compare-and-swap of the register to the finished record with the finished flag. A failure after (3) releases the register and removes the marker, so a failed take leaves nothing that names storage.
- **Request.** The body takes `indices` (a list or a comma string; empty is 400 `missing_indices`), `ignore_unavailable`, `partial` and `include_global_state` (read and reported false); `metadata` is 501 `unsupported_snapshot_option` and any other key 400 `unknown_parameter`. Names are checked by `validateId` and `#` as the read slice does. `wait_for_completion` (default false) only changes the answer: the take is synchronous either way, and without it the answer is the Java's accepted body.
- **Cost, no LIST:** 1 GET (repository) + per index 1 GET (descriptor) and 1 GET (its descriptor again for the incarnation check, which the Java also makes) + per shard 1 GET (its commit; a bucketed member's commit is read from its bucket entry) + 1 PUT register create + 1 PUT marker + 1 GET register + 1 register swap. The take is bounded at 100 indices and 4,096 shards in total (400 `snapshot_too_large`), so a take's requests are bounded; a bigger one is a later decision. Memory: a reservation or finished record is built into one buffer reserved from the request-body pool (429 when it cannot be), capped at 16 MiB (413 `snapshot_too_large` beyond).
- **Fencing is the register's compare-and-swap**, as the Java's; no lock. A node that dies mid-take leaves a reserved, unfinished snapshot: it reads as `IN_PROGRESS` and holds its indices' blobs (the janitor pins by incarnation) until a delete releases it. That is the Java's behaviour and is accepted; its delete is a later slice.
- **Switch:** the existing `serverless.snapshots.enabled`; with it off no route is registered. Nothing else is added: a take writes only under `snapshot-states/` and `snapshots/`.
- **Placement:** the record writer and the state encoding in `oss-format` (`snapshot_record.rs`, `snapshot_state.rs`, tested against the Java's fixtures and by the Java reading what Rust wrote), the reservation, protective marker, finish and cleanup in `oss-meta`, the take's decisions in `oss-coordinator`'s `snapshot_admin`, the orchestration (describe, shard commits) and route in `oss-node`.

## Open for the build

- The commit a Rust writer publishes (a shard register or bucket entry) must be expressed as the Java's `CommitManifest` shape in the record (`term`, `files`, `lengths`). The first slice of the build proves this by a mixed-fleet test: a snapshot taken by a Rust node is read, and its restore planned, by a Java node. Until that passes the route stays off.
- Restore and snapshot delete remain 501.

## Consequences

Easier: a deployment can snapshot its indices without copying data. Harder: a shallow snapshot is only as durable as the live index's blobs, so a deleted index's blobs must stay pinned while any snapshot names them (the janitor already does this, held by the incarnation). Standard snapshots are not available.

## Alternatives rejected

- Standard (copying) snapshots first: scales with bytes, needs its own cost and crash decisions.
- A Rust-only snapshot format: the Java must read what Rust takes, in both directions.
- Taking asynchronously: the Java is synchronous, and a background take needs a timer or a worker that this node has no use for.
