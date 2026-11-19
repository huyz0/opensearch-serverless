# Point in time: freezing an index

## Serves

R10.6: `PointInTimeStore::capture`, the write half of a point in time: it freezes every shard's published commit of one index under a fresh id. Invariants: the fencing rule (nothing is held by a lock; the marker and the finished view are compare-and-swap writes), I-1 (every call metered) and idle costs nothing. Follows [point-in-time-store](../point-in-time-store/SPEC.md) and [janitor-point-in-time-pins](../janitor-point-in-time-pins/SPEC.md); ported from the shell's `PointInTimeHandler.freeze` and `PointsInTime.beginPointInTime`/`verifyCaptureIncarnation`.

## Change

`PointInTimeStore::capture(descriptors, index, keep_alive, buf)` returns the finished `PointInTime`.

1. Resolve the descriptor. An absent name or an alias is `NoSuchIndex`; a bucketed index or one laid out in units is `Unsupported` (later slices). Nothing is written for these.
2. Create the capturing marker under a fresh 22-character id (16 random bytes, base64url), expiring at the store's clock plus the keep-alive. It is written before any manifest is read, so the janitor holds everything while the capture runs.
3. Resolve again: a descriptor with another uuid means the index was deleted and made again, and the capture is refused as `Replaced`.
4. Read each shard's manifest register, shards `0..n`. One never published is `NothingPublished(shard)`. The view records each file's name, encoded location and length.
5. Finish the marker into the view under the same id.

A failure after step 2 releases the marker best-effort and reports the first failure.

Not here: the REST handlers, the node wiring, bucketed and unit-layout indexes.

## Acceptance criteria

- AC-1: every shard's commit (term, files, locations, lengths) is frozen under an id that looks like an id, the view expires at the clock plus the keep-alive, and it reads back equal to what `capture` returned.
- AC-2: a capture sends no LIST, and costs exactly the budget below at 1 and at 3 shards.
- AC-3: a shard with nothing published refuses the capture and leaves no live view.
- AC-4: an absent name or an alias is refused; nothing is written.
- AC-5: a bucketed index is refused after one descriptor read and nothing written.
- AC-6: an index replaced between the marker and the incarnation check is refused and its marker released.
- AC-7: a view of one incarnation does not pin a later index of the same name.
- AC-8: `new_id` yields 22 base64url characters, for known byte vectors.

## Tests

`crates/meta/tests/descriptors/point_in_time_capture.rs` over every backend; unit tests in `crates/meta/src/pit/capture.rs`.

## S3 request budget

Exactly: shards + 3 GETs (the descriptor twice, a manifest a shard, the view's finish read), 1 HEAD and 3 PUTs (the marker's register and blob, the view); the filesystem store adds 1 ranged GET. Nothing scales with time, and nothing is sent for an index that is never captured.

## Risks

- A shard publishing between its manifest read and the finish is not in the view's commit of the other shards; each shard's commit is consistent alone, as in the Java.
