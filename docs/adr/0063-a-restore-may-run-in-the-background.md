# 0063: A restore may run in the background, behind a setting

**Status:** Accepted after independent review, 2026-11-26

## Context

[ADR 0045](0045-shallow-snapshot-restore.md) made a restore synchronous: the request holds its connection while every file is copied, and `wait_for_completion=false` (the default) answers `{"accepted":true}` only after the copy has finished. For a large snapshot the request outlives most clients' and proxies' timeouts, holds a connection and its request-body reservation for minutes, and a client that gives up cannot tell whether the restore went on. Stock OpenSearch answers `accepted` at once and restores in the background.

The safety a background restore needs is already built: an index is marked `index.restore.in_progress` from its descriptor create until every shard has landed, so a node that dies part-way leaves an index that refuses searches and writes rather than one that answers from empty shards ([snapshot-restore](../specs/snapshot-restore/SPEC.md) AC-9), and a failed restore deletes what it created (AC-3).

## Decision

With `serverless.snapshots.restore.background.enabled` on (default off), a restore with `wait_for_completion=false` copies on a dedicated thread. The request does everything that can fail on a name or a bound first, then answers `{"accepted":true}`:

1. Every check that precedes the first write (the snapshot, the targets, the system indices, the bounds, and for a stock repository its own checks; `restore_stock` takes the same path).
2. The slot and the reservation (below), taken before the first write and released on every refusal after them.
3. **Every index's marked descriptor is created on the request thread**, one create each, the same requests as now. A name taken between the check and the create is therefore a 400 `index_already_exists` and the indices already created are deleted, exactly as the synchronous restore does, and from the moment of the answer every restored index exists and is marked.

The thread then copies each shard, lands each manifest and clears each mark; on a failure it deletes every index the restore created. `wait_for_completion=true` is unchanged and stays synchronous with the setting either way. The restore code splits into a create step and a copy-land-unmark step; the synchronous restore runs both in order.

**Memory.** The thread holds an owned reservation taken directly from the memory budget (`Pool::RequestBodies`, released on drop, not tied to the request) that covers everything it allocates: the two copy buffers, the descriptor buffer and the index-delete service's bytes for its cleanup. The request's own 4 MiB record buffer is released when the handler returns and is not part of it. A restore the pool cannot hold is 429 and writes nothing.

**Concurrency.** At most `serverless.snapshots.restore.background.max` (1 to 16, default 2) restores run at once on a node; another is 429 `too_many_requests` before any write. The slot and the reservation are held by one guard that releases both when the thread ends by any road, a panic included; a thread that cannot be spawned is a 503 with nothing written (or the creates deleted).

**What the client sees.** The restored indices exist, marked, from the answer: searches and writes are refused 503 `cluster_block_exception` until the mark clears. The messages for a marked index (the block and `index_already_exists` on a retry) say "a restore is in progress or was interrupted; delete the index to retry". A failure deletes the indices, so a client that waits sees them vanish; the reason is logged and counted, and no endpoint returns it. The running and failed counts are exposed on the node stats surface. This is where the background restore differs from stock, which reports progress through `_recovery` and the cluster health.

**Shutdown.** A drain sets a cancel flag the thread checks between files and between chunks. Once a drain has started, a new background restore is refused 503 before any write: taking the slot is what checks it. A thread that starts with the flag already set cancels and cleans up at once. A thread that sees the flag stops, deletes the indices it created (a few bounded requests) and ends; the drain waits up to `serverless.snapshots.restore.background.grace` (default 30 s) for that. Only a process that dies before then leaves a marked index, which the restore's own rules handle (delete and restore again).

**Why off by default.** A failure is no longer returned to the caller, and a crash leaves a marked index nobody was told about; the synchronous restore keeps both honest. It is turned on per deployment that restores snapshots larger than its clients' timeouts.

## Alternatives rejected

- **A job register in the store** that records progress and failure, readable from any node. It would let a client learn why a restore failed and let another node adopt a dead node's restore, but it adds a register and its requests per restore and a new API; the marked index already makes the dead case safe. A later ADR can add it if callers need it.
- **Streaming progress in the response** (chunked, held open): keeps the connection, which is the problem.
- **Always background, no setting:** changes the failure contract for every caller.

## Consequences

The request budget rows of a restore are unchanged (the same requests, on another thread). Running and failed background restores are counted on the node stats surface. [ADR 0045](0045-shallow-snapshot-restore.md)'s statement that a restore is synchronous holds with the setting off. The thread count is bounded by the setting, and each holds two buffers.
