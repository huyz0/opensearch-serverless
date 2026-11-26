# Restore in the background

## Serves

Parity plan R14, [ADR 0063](../../adr/0063-a-restore-may-run-in-the-background.md): `POST /_snapshot/{repo}/{name}/_restore` with `wait_for_completion=false` answers `{"accepted":true}` once the checks before the first write pass, and copies on a dedicated thread. Builds on [snapshot-restore](../snapshot-restore/SPEC.md) and [stock-snapshot-restore](../stock-snapshot-restore/SPEC.md).

## Acceptance criteria

- AC-1: With the switch on and `wait_for_completion` absent or `false`, the answer is `{"accepted":true}` before any shard file is copied (a gate holds the copy; the answer arrives while it is held), and after the release the restored index answers a search with what the snapshot captured. Test: a held copy, the answer, the release, the search.
- AC-2: Every refusal that precedes the first write in a synchronous restore (missing or in-progress snapshot, uncaptured index, colliding or existing target, system index, bounds, unsupported key, a stock repository's own checks) is answered by the request itself, 4xx or 501, with nothing written and no thread started. A restore from a stock repository stays synchronous whatever the switch says: its answer follows the copy. Test: one request each, asserting no write and zero running restores.
- AC-3: Every index's marked descriptor exists when the answer is sent. A name that appears between the check and the create is 400 `index_already_exists` and the indices created before it are deleted. Test: a target created between check and create, expecting 400 and nothing left; a search straight after the answer is 503, never 404.
- AC-4: While the copy runs the restored index is marked (`index.restore.in_progress`), so a search and a write are refused 503 `cluster_block_exception` by the existing mark tests; when the thread ends the mark is gone. A retry over the marked name is 400 `index_already_exists`; the block's message already says the index is being restored or a restore was interrupted and to delete it; a delete of a marked index clears it, the copy that was running ends in a failure, and the name can be restored again. Test: the mark during the held copy and after; the retry; the delete and the second restore.
- AC-5: A failure in the thread (a store failing at a call, a manifest that cannot land) deletes every index it created, releases the reservation and the slot, logs the failure and adds one to the failed count; the answer was already sent. A panic in the thread releases both through the guard. Test: a fault at each call, and a forced panic, each asserting no index remains and the pool and slot are free.
- AC-6: The reservation covers the two copy buffers, the descriptor buffer and the cleanup's delete bytes, and is taken, with the slot, before the first write and released on every refusal after it. A restore the pool cannot hold is 429 and a restore past `max` running is 429, each writing nothing; a thread that cannot be spawned is 503 with nothing written (read in the code, not forced by a test). The pool bytes stay held after the answer while the copy is gated and fall to their earlier level when the thread ends. Test: a small pool, the cap, and the pool read before, during and after.
- AC-7: A drain sets the cancel flag; the thread stops between files or chunks, deletes the indices it created and ends within the grace, which the drain waits for. Test: a drain over a held copy with a short grace, then no index remains and the name restores again. Once a drain has started a new background restore is 503 before any write, and a thread that starts with the flag set cancels and cleans up at once. Test: a restore request after the drain begins. A thread that outlives the grace leaves the marked index of AC-4.
- AC-8: Requests are the synchronous restore's, on either source (`snapshot.restore_two.requests` and `snapshot.stock_restore.requests` read the same with the setting on); no LIST. The running and failed counts are read from `BackgroundRestores` and a failure is written to the node's standard error; no stats endpoint carries them yet. Test: the rows read with the setting on, and the counts across a success and a failure.
- AC-9: With the switch off (the default), or with `wait_for_completion=true`, the restore is synchronous exactly as before: the existing restore tests pass unchanged. `.max` of 0 or 17 and a `.grace` that is not a number are refused at start. Test: both modes over the same rig; the three settings.

## Bounds

At most `max` threads and `2 x chunk` bytes each; no new request.

## Switch

`serverless.snapshots.restore.background.enabled` (`OSS_SNAPSHOTS_RESTORE_BACKGROUND_ENABLED`), default off, with `.max` (`..._MAX`, 1 to 16, default 2) and `.grace` (`..._GRACE_MILLIS`, default 30,000): a failure is no longer returned to the caller.

## Risks

A client that never looks again will not learn of a failure; a node killed mid-restore leaves a marked index nobody was told about. Both are the documented difference from stock; the `cluster_block_exception` message now names the restore.

## Build order

1. The settings, the owned reservation and the slot guard (AC-6, AC-9). 2. The restore's split into create and copy-land-unmark, and the route's use of it (AC-1, AC-2, AC-3). 3. Failure, panic and cancel (AC-5, AC-7); cancellation between chunks needs a hook in the copy loop of `crates/meta/src/snapshot_restore.rs` or in `RangedSource`. 4. The messages, the stats and the budget rows (AC-4, AC-8). Docs: ADR 0045's pointer, the compatibility entry, the settings how-to and the snapshot-restore spec's how-it-is-held.
