# Snapshot take, shallow

## Serves

[ADR 0042](../../adr/0042-snapshots-taken-shallow-first.md): a Rust node takes a shallow snapshot in the Java shell's protocol and record format, behind `serverless.snapshots.enabled` and `serverless.snapshots.take.enabled` (both default false).

## Acceptance criteria

- AC-1: `SnapshotRecord::write` writes the shell's record (identity, `storage_generation`, `shallow`, times, per index uuid, shard count, mapping, settings and each shard's `shard`, `term`, `files`, `lengths`), and `SnapshotState::write` its register value; a record written here equals the Java-written fixtures once parsed, and a Java node reads what Rust wrote in the mixed-fleet run.
- AC-2: a take reserves the name (register create, or a swap over a released one, up to 16 tries; a held name is 400 `snapshot_already_exists`), writes the protective marker and confirms the register still names it, verifies each index's uuid, reads each shard's commit and finishes with a swap; the result reads as `SUCCESS` through `GET /_snapshot/{repo}/{name}`.
- AC-3: the refusals, each with the ADR's status and name: standard repository 501; missing repository 404; missing index 404 unless `ignore_unavailable`; system index 403 (the names `serverless.system_indices.patterns` declares, none by default); no commit 409 `nothing_published` or skipped with `partial` (all skipped is 409); layout or split index 501 `unsupported_snapshot_layout`; `metadata` 501; unknown key 400; empty `indices` 400; over 100 indices or 4,096 shards 400 `snapshot_too_large`; invalid or over-long names 400 `invalid_snapshot_name_exception`; duplicates in `indices` count once.
- AC-4: with the Nth store call failing at each point (after the register create and before the marker; after the marker and before the finish; at the finish swap) the take releases its own lifetime, removes its marker only as the ADR says, returns the original error, and leaves nothing that names storage; two concurrent takes of one name give one 200 and one 400; a delete plus re-take of the name during a failing take is not wiped; a take racing an index delete (before and after the incarnation check) fails 409 `snapshot_conflict` or captures one incarnation, via `index_delete/pins.rs`.
- AC-5: the janitor does not delete a blob of an index a snapshot names, tested after the marker is written and after the finish, over the emulator.
- AC-6: with either switch off the take answers 501 and makes no request; a record over 16 MiB is 413 `snapshot_record_too_large`; a failed pool reservation is 429 (test hook named in the build); the write cap equals the read cap.
- AC-7: budget rows `snapshot.take.requests` (11 for one index, one shard) and `snapshot.take_two.requests` (two indices, two shards each, proving the per-index and per-shard slopes), spent by a gated test; the count is the same for a 1 KB and a 100 MB index; a key over 255 bytes makes no LIST.
- AC-8: a Java node lists, describes and plans a restore of a snapshot a Rust node took, by recorded fixtures and a Java-side test over a Rust-written record; the take switch stays off until this passes.

## Out of scope

Standard snapshots, restore and snapshot delete.
