# Janitor: the pins points in time keep

## Serves

R10.6 and R8.4: the garbage collector's `Pins` for frozen views, so a sweep never deletes a blob a point in time names. Invariants: the fencing rule's safety argument (nothing a view names is deleted), I-1 (every call metered) and idle costs nothing. Follows [point-in-time-store](../point-in-time-store/SPEC.md) and [janitor-gc](../janitor-gc/SPEC.md); decision record [ADR 0020](../../adr/0020-janitor-crate.md).

## Change

`gc::PointInTimePins` implements `gc::Pins` over the live points in time, ported from the shell's `GarbageCollector` use of `livePointsInTime`, `PointInTime.pins`, `referencedBlobs`, `isCapturing` and `isPlaceholder`.

- **One read a pass.** `PointInTimePins::snapshot` reads the live views once (`PointInTimeStore::live`: one LIST page per 1,000 views and a register read each, claiming expiry as it goes); every shard of the pass is then answered from memory with no request. That is safe under the collector's two-sweep grace, provided a sweep that deletes uses a snapshot taken after the sweep that first saw the blob. A snapshot therefore answers each shard once: asked about a shard again it says `Everything`, so a stale snapshot is never the basis of a deletion and a pass must take a fresh one for each round of sweeps. The argument: a blob a sweep may delete has been unreferenced by the live commit since an earlier sweep, so no view made after that sweep's snapshot can name it, and a view made before is in this snapshot.
- **What a view pins of a shard.** A view whose index and incarnation match the shard (by name alone for a record with no uuid) pins the blob of each file of the shard's frozen commit, in every location form (a blob of its own, a range of a segment blob, a node pack), keyed exactly as `ShardRef::resolve` keys a manifest.
- **When it cannot tell.** The answer is `Pinned::Everything`, and the sweep deletes nothing: for a view that could not be read (a placeholder), for a capture of the shard's index that has not finished, and for a view with a file location no reader can follow.
- `oss-format` gains `ShardRef::blob_key`, which `resolve` now uses for each file.

Not here: the node wiring of the pass, and snapshot pins (R14.2).

## Acceptance criteria

- AC-1: with no views, a shard has no pinned blobs.
- AC-2: a view pins the exact blob key of each file of its shard in every location form.
- AC-3: a view of another index, another incarnation or another shard pins nothing of this one; a view with no uuid matches by name; the files of several views are all pinned.
- AC-4: an unfinished capture of the shard's index pins everything, whatever incarnation it names; one of another index does not.
- AC-5: a view that cannot be read, or whose location cannot be followed, pins everything.
- AC-6: a snapshot costs one LIST and one GET a view, and answering any number of shards sends no request.
- AC-8: a snapshot answers each shard once: a view made after it, naming a blob a sweep has already watched, is not lost to a second sweep on the same snapshot, and a fresh snapshot keeps the blob.
- AC-9: an expired view pins nothing and a keep-alive after its claim is refused; a kept-alive view still pins.
- AC-10: every frozen entry of the shard a record names is pinned; more live views than the cap fail the snapshot, so the pass deletes nothing.
- AC-7: a sweep keeps a blob only a view names, over any number of sweeps, and deletes it two sweeps after a snapshot taken after the view's release.

## Tests

`crates/janitor/tests/point_in_time_pins.rs` over the emulator with its meter.

## S3 request budget

Per pass: 1 LIST page per 1,000 live views and 1 GET per view (plus the swap and delete an expiry claim costs). Per shard: nothing.

## Risks

- A snapshot holds every live view's record, each up to 16 MiB; `max_views` fails the pass closed past a count, but the listing has built the views by then, so it is a refusal and not a bound on the read. A reduced form holding only file keys comes if a deployment keeps many large views.
- Answering walks every view for every shard, O(shards x views) CPU a pass and no requests; an index by view name comes if the counts grow.
