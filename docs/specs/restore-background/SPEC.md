# Restore in the background

## Serves

Parity plan R14, [ADR 0063](../../adr/0063-a-restore-may-run-in-the-background.md): `POST /_snapshot/{repo}/{name}/_restore` with `wait_for_completion=false` answers `{"accepted":true}` once the checks before the first write pass, and copies on a dedicated thread. Builds on [snapshot-restore](../snapshot-restore/SPEC.md) and [stock-snapshot-restore](../stock-snapshot-restore/SPEC.md).

## Acceptance criteria

- AC-1: With `serverless.snapshots.restore.background.enabled` on and `wait_for_completion` absent or `false`, the answer is `{"accepted":true}` before any shard file is copied (a gate holds the copy; the answer arrives while it is held), and after the copy finishes the restored index answers a search with what the snapshot captured. Test: a held copy, the answer, the release, the search.
- AC-2: Every refusal that precedes the first write in a synchronous restore (missing or in-progress snapshot, uncaptured index, colliding or existing target, system index, bounds, unsupported key, a stock repository's own checks) is answered by the request itself, 4xx or 501, with nothing written and no thread started. Test: one request each, asserting no write and zero running restores.
- AC-3: With the switch off (the default), or with `wait_for_completion=true`, the restore is synchronous exactly as before: the existing restore tests pass unchanged. Test: both, over the same rig.
- AC-4: While the copy runs the index exists with the restore mark: a search answers 503 `cluster_block_exception` and a write finds no index; when the thread ends the mark is gone. Test: a search during the held copy and one after.
- AC-5: A failure in the thread (a store failing at a call, a manifest that cannot land) deletes every index it created, ends the thread, releases its reservation and its slot, logs the failure and adds one to the failed count; the answer was already sent. Test: a fault at each call, asserting no index remains and the pool and slot are free.
- AC-6: The two copy buffers are reserved before the answer and held until the thread ends; a restore the pool cannot hold is 429 and writes nothing; a restore past `serverless.snapshots.restore.background.max` (default 2) running is 429 and writes nothing. Test: a small pool, then the cap.
- AC-7: A drain waits up to `serverless.snapshots.restore.background.grace` (default 30 s) for running restores, then returns; the index a stopped restore leaves is the marked one AC-4 describes, which a delete clears. Test: a drain over a held copy with a short grace.
- AC-8: Requests are the synchronous restore's, per restore, on either source (the budget rows `snapshot.restore_two.requests` and `snapshot.stock_restore.requests` hold unchanged under the setting); no LIST. Test: the same rows read with the setting on.

## Bounds

At most `max` threads and `2 x chunk` bytes each; no new request.

## Switch

`serverless.snapshots.restore.background.enabled` (`OSS_SNAPSHOTS_RESTORE_BACKGROUND_ENABLED`), default off, with `.max` (`..._MAX`, 1 to 16, default 2) and `.grace` (`..._GRACE_MILLIS`, default 30,000): a failure is no longer returned to the caller.

## Risks

A client that never looks again will not learn of a failure; a node killed mid-restore leaves a marked index nobody was told about. Both are the documented difference from stock.

## Build order

1. The settings and the thread starter with its slot and reservation (AC-6). 2. The route's split between the checks and the run (AC-1, AC-2, AC-3). 3. Failure handling and drain (AC-5, AC-7). 4. The mark's visibility and budget rows (AC-4, AC-8).
