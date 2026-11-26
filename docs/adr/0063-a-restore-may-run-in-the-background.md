# 0063: A restore may run in the background, behind a setting

**Status:** Proposed, awaiting independent review

## Context

[ADR 0045](0045-shallow-snapshot-restore.md) made a restore synchronous: the request holds its connection while every file is copied, and `wait_for_completion=false` (the default) answers `{"accepted":true}` only after the copy has finished. For a large snapshot the request outlives most clients' and proxies' timeouts, holds a connection and its request-body reservation for minutes, and a client that gives up cannot tell whether the restore went on. Stock OpenSearch answers `accepted` at once and restores in the background.

The safety a background restore needs is already built: an index is marked `index.restore.in_progress` from its descriptor create until every shard has landed, so a node that dies part-way leaves an index that refuses searches and writes rather than one that answers from empty shards ([snapshot-restore](../specs/snapshot-restore/SPEC.md) AC-9), and a failed restore deletes what it created (AC-3).

## Decision

With `serverless.snapshots.restore.background.enabled` on (default off), a restore with `wait_for_completion=false` is started on a dedicated thread and answered `{"accepted":true}` as soon as every check that runs before the first write has passed (the snapshot, the targets, the system indices, the bounds, the source's own checks for a stock repository). The thread then does what the request did: create each index marked, copy each shard, land each manifest, clear the mark; on a failure it deletes every index it created. `wait_for_completion=true` is unchanged and stays synchronous with the setting either way.

**Memory.** The two copy buffers are reserved from the request-body pool before the answer, held by a reservation that moves into the thread and is released when it ends, not by the request. A restore the pool cannot hold is 429 and writes nothing, as now.

**Concurrency.** At most `serverless.snapshots.restore.background.max` (default 2) restores run at once on a node; another is 429 `too_many_requests` before any write, so the pool and the store are never asked for more than the cap.

**What the client sees.** The restored index exists, marked, from the thread's first create: searches answer 503 `cluster_block_exception` until the mark clears, then answer. A failure deletes the index, so a client that waits sees it vanish; the reason is logged by the node and counted, and no endpoint returns it. This is the one place the background restore differs from stock, which reports progress through `_recovery` and the cluster health.

**Shutdown.** A node that drains waits up to `serverless.snapshots.restore.background.grace` (default 30 s) for running restores, then lets the process end: the marked index is what a crash leaves, which the restore's own rules already handle (delete and restore again).

**Why off by default.** It has a measured downside by construction: a failure is no longer returned to the caller, and a node that stops mid-restore leaves a marked index that nobody was told about. The synchronous restore keeps both honest. It is turned on per deployment that restores snapshots larger than its clients' timeouts.

## Alternatives rejected

- **A job register in the store** that records progress and failure, readable from any node. It would let a client learn why a restore failed and let another node adopt a dead node's restore, but it adds a register and its requests per restore and a new API; the marked index already makes the dead case safe. A later ADR can add it if callers need it.
- **Streaming progress in the response** (chunked, held open): keeps the connection, which is the problem.
- **Always background, no setting:** changes the failure contract for every caller.

## Consequences

The request budget rows of a restore are unchanged (the same requests, on another thread). A new gauge counts running and failed background restores. The thread count is bounded by the setting, and each holds two buffers.
