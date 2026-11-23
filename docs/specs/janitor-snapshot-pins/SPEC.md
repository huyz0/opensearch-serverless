# Snapshot pins for the per-shard sweep

## Serves

[ADR 0020](../../adr/0020-janitor-crate.md): "a blob goes only when ... nothing names it (the manifest, a point in time, a snapshot)". The per-shard sweep of a node's held shards ([node-janitor-pass](../node-janitor-pass/SPEC.md)) honoured the manifest and the points in time but not snapshots, which the Java shell writes under `snapshots/`. A Rust node that deletes, in a fleet with Java snapshots, could delete a blob a snapshot holds. The orphan-shard sweep already reads them ([janitor-orphan-sweep](../janitor-orphan-sweep/SPEC.md)); this brings the per-shard sweep to the same rule before the janitor's default turns to deleting.

## Change

- `gc::SnapshotPins` wraps the points-in-time pins: a shard of an index incarnation (name and uuid) a snapshot names answers `Everything`, so the sweep deletes nothing of it. The whole incarnation is held because a record names indexes, not blobs, and a capture in progress has no list of them; this holds at least what the Java's `SnapshotRecord.referencedBlobs` holds.
- `JanitorPass` reads the snapshot records once a pass that has a shard to sweep, with `SnapshotSource::collect_with` through the buffer it reserved, and sweeps over both. A record that cannot be read, or more snapshots or incarnations than `max_views`, ends the pass with nothing swept.
- Cost: one LIST page and a GET a snapshot, once a pass that has a due shard. A shard a snapshot holds is not due again for `relist_after_millis` (an hour by default) unless it publishes a new commit, so a snapshot that lives for months costs one read an hour a node with such a shard, not one a pass; the shard's old blobs are swept within that hour of the snapshot going. A node with nothing due makes no request.

## Acceptance criteria

- AC-1: a shard of an incarnation a snapshot names is not swept: its older-term unreferenced blob stays over repeated passes.
- AC-2: a snapshot of another incarnation of the name, or of another index, holds nothing of the shard, and with no snapshot the blob goes.
- AC-3: a snapshot record that cannot be read ends the pass with a reason and deletes nothing.
- AC-4: reading no snapshot costs the request-budget row `janitor.pass.snapshot_pins.requests`.
- AC-6: passes after the first over a shard a snapshot holds make no request until the recheck, and the shard is swept once the snapshot is gone and the recheck has passed (request-budget row `janitor.pass.snapshot_held.requests`).
- AC-7: more snapshots than the pass may hold (`max_views`) end the pass with a reason and delete nothing.
- AC-5: `SnapshotPins` answers `Everything` for a held incarnation and the inner pins' answer for any other (unit tests).

## Test plan

`crates/node/tests/janitor_snapshot_pins.rs` over the emulator and a real writer role; unit tests in `crates/janitor/src/gc/snapshot_pins.rs`.
