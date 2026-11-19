# Point-in-time store

## Serves

R10.6, first slice. A point in time freezes an index's commit so a caller can page through it, and promises the garbage collector that the files it names stay. This slice is the record and the store that keeps it, in the Java's formats so Rust and Java nodes on one bucket agree. Not here: capturing a view from the shard commit registers, the REST handlers, forwarding a search to a frozen view, and the janitor's `Pins` reading these records ([janitor-gc](../janitor-gc/SPEC.md) waits for them).

Invariants served:
- **I-1:** every store call is metered.
- **Fencing:** a view changes only by compare-and-swap of its state register, or a conditional create, never a lock or a lease.
- **Idle costs nothing:** nothing reads a register on a timer; the only listing is the explicit `live` and `reap`.
- **Bounded memory:** a record is read through the caller's buffer and refused past 16 MiB.

## Change

Ported from huyz0/OpenSearch `serverless/shell/src/main/java/org/opensearch/serverless/metadata/PointInTime.java`, `PointInTimeStore.java`, `PointsInTime.java` and `RegisterMap.java` at `429b4cbc`.

### In `oss-format`

- `PointInTime` is the record: id, index, uuid (optional), deadline, a capture flag, and each frozen shard's term and files with locations kept as recorded. `parse` and `write` are the Java's `fromStream` and `toBytes`; `pins`, `expired_at`, `with_expiry` and `marker` are its methods of the same names.
- `RELEASED` is the tombstone a released id's state register holds.
- `point_in_time_key` (`pits/<id>`), `point_in_time_state_key` (`pit-states/<id>`) and `looks_like_an_id` (22 URL-safe characters).

### In `oss-meta`

`PointInTimeStore` keeps, for each id, an authoritative state register and an inventory marker that names the index and a deadline no collector reads as past:

- `create` a view or a capture marker; the id must be new, a released id stays taken.
- `finish` replaces a capture with the finished view under the same id, keeping the capture's deadline.
- `extend` moves the deadline later, optionally only from the deadline the caller saw.
- `read`, `release`, `live` and `reap`; expiry is claimed by the same swap that releases, so a keep-alive elsewhere forces a reread.
- A view written before state registers existed, with only an inventory blob, is adopted into a register on first read.

Where it differs from the Java, on purpose: records are bounded; `begin`'s check that the index still has the captured uuid is left to the capture flow that calls it, which needs the descriptor store; a number the Java would read from a string is refused.

## Acceptance criteria

- AC-1: the record reads and writes the bytes the Java's `toBytes` writes, with and without uuid and lengths, and a capture marker writes as the Java's marker does.
- AC-2: a malformed record is refused: wrong types, negative numbers, a repeated field, a shard twice or past 4095, a length naming no file, a view with no shards, a marker with shards, trailing text, past the byte bound or the per-shard file bound.
- AC-3: `pins` matches by uuid when both sides know one and by name otherwise; `expired_at` is true at the deadline; `with_expiry` changes nothing else.
- AC-4: `create` stores a view and a marker in one HEAD, two PUTs; a view reads back in one GET; the same or a released id cannot be created again.
- AC-5: a capture is not readable until `finish` replaces it under the same id, which refuses a gone, released, other-index, other-uuid or capture-shaped finish.
- AC-6: `extend` only moves the deadline later, refuses a capture, an expired view and an unknown id, and a conditional extend loses to a changed deadline; an unconditional one retries, and gives up as contended after 16 attempts.
- AC-7: `release` swaps the register to the tombstone then deletes the marker; once released the id reads as absent, cannot be created or extended, and releases false.
- AC-8: `live` returns the views that have not expired, claiming the expiry of those that have, and ignores inventory names that are no id; `reap` counts those it removed; a view that cannot be read stands in as a placeholder (`is_placeholder`: no index, never expires) that a collector must read as pinning every index, and `reap` leaves it.
- AC-9: a view with only a legacy inventory blob is adopted into a register once and then read in one GET; an entry under another id is refused and not adopted.
- AC-10: a marker that cannot be written leaves the id tombstoned and its marker absent, and the error is the marker's; a marker another creator already wrote is left in place.
- AC-11: `create` and `finish` refuse a record no reader would take (a view with no shards, a file named twice) before anything is stored, so no id is ever left unreadable and unreleasable.
- AC-12: a collector claiming an expired view loses to a keep-alive that wins first and reports the view live.
- AC-13: the request counts of finish, a keep-alive that moves the deadline, one that does not, and a reap are as `S3 request budget` says, per backend.

## Tests

AC-1 to AC-3: `crates/format/tests/point_in_time.rs`. AC-4 to AC-10: `crates/meta/tests/descriptors/point_in_time.rs` over the emulator, S3 on loopback and the filesystem store, with request counts.

## S3 request budget

Create: 1 HEAD, 2 PUT. Read, extend (when the deadline already holds): 1 GET. Extend that moves it: register read, a GET of the marker (a PUT only when it is not already the marker), then the swap. Release: read, swap, delete. `live` and `reap`: one LIST page per 1000 views, then a read and perhaps a swap and a delete for each. Nothing idle.

## Risks

- `live` returns whole records, each at most 16 MiB, and the count of views is bounded only by their deadlines; the buffer the caller passes must hold the largest record or that view reads as a placeholder (the safe direction). A cap, or a reduced form for the collector, comes with the `Pins` implementation.
- The inventory is the janitor's source of truth for a pass. A view the store cannot read pins every index (AC-8), the safe direction.
- Every keep-alive that moves a deadline reads the marker, as the Java does; a legacy-only check could skip it once the Java no longer writes such views.
