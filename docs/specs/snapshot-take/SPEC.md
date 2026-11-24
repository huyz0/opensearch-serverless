# Snapshot take, shallow

## Serves

[ADR 0042](../../adr/0042-snapshots-taken-shallow-first.md): a Rust node takes a shallow snapshot in the Java shell's protocol and record format, behind `serverless.snapshots.enabled` (default false).

## Acceptance criteria

- AC-1: `SnapshotRecord::write` writes the shell's record (identity, `storage_generation`, `shallow`, times, per index uuid, shard count, mapping, settings and each shard's `shard`, `term`, `files`, `lengths`), and `SnapshotState::write` its register value; a record written here equals the Java-written fixtures once parsed, and a Java node reads what Rust wrote in the mixed-fleet run.
- AC-2: a take reserves the name (register create, or a swap over a released one, up to 16 tries; a held name is 400 `snapshot_already_exists`), writes the protective marker and confirms the register still names it, verifies each index's uuid, reads each shard's commit and finishes with a swap; the result reads as `SUCCESS` through `GET /_snapshot/{repo}/{name}`.
- AC-3: the refusals: a standard repository 501 `unsupported_snapshot_storage`; a missing repository 404; a missing index 404 `index_not_found` unless `ignore_unavailable`; a system index 403; a shard with no commit 409 `nothing_published`, or skipped with `partial`; `metadata` 501; an unknown key 400; empty `indices` 400; more than 100 indices or 4,096 shards 400 `snapshot_too_large`; names checked as the read slice does.
- AC-4: a failure after the reservation releases the register and removes the marker, and leaves nothing that names storage; a take racing a delete of an index it names fails or captures the same incarnation, never a mixture.
- AC-5: the janitor does not delete a blob of an index a taken snapshot names (the pins read it), over the emulator.
- AC-6: with the switch off the take route is not registered and no request is made; a take where the record would exceed 16 MiB answers 413 and a failed pool reservation 429.
- AC-7: request budgets: `snapshot.take.requests` as the ADR's cost for a one-index, one-shard take, and no request grows with the bytes of any index.
- AC-8: a Java node lists, describes and plans a restore of a snapshot a Rust node took (the mixed-fleet run); the route stays off until this passes.

## Out of scope

Standard snapshots, restore and snapshot delete.
