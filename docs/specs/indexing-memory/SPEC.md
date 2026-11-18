# Indexing memory: when a shard's buffer is written out

## Serves

Parity plan R6.4 (roadmap stage 3, [roadmap](../../roadmap.md)) and the bounded-memory rule ([ADR 0004](../../adr/0004-bounded-memory-and-streaming.md)): a node's indexing buffers share one budget, and the shards using most of it are refreshed first. Builds on the [shard engine](../shard-engine/SPEC.md), whose `Shard::indexing_buffer_bytes` is what a shard uses.

## Change

`oss-writer` gains `indexing_memory` (`crates/writer/src/indexing_memory/`), ported from core's `IndexingMemoryController` (OpenSearch, `server/.../indices/IndexingMemoryController.java`), sans I/O: the node gives it what each shard uses, and it says what to do.

- **Budget**: `IndexingBudget::resolve(size, heap, min, max)`. A size that is a percentage of `heap` (default 10%) is raised to `min` (default 48 MiB) and capped at `max` when one is set; a size in bytes is used as it is. As core's `computeBufferSize`.
- **When to check**: `IndexingMemory::bytes_written(n)` adds the estimated size of a write, and says a check is due when the bytes since the last check pass a thirtieth of the budget. Of racing callers exactly one is told so, and the count drops by what it took. The node also checks on its interval (5 s default).
- **What a check decides**: `check(&[ShardUsage])`, where a shard gives its key, the heap its buffer uses, the bytes it is already writing out, and how long it has had no write.
  - A shard's counted use is its buffer less what it is writing out, never below zero.
  - Over budget (counted use above the budget): shards using something are taken largest first and refreshed until the rest fits.
  - Throttling: when counted use plus bytes being written passes one and a half budgets, each shard refreshed that is not yet throttled is throttled; when it does not, every throttled shard is released.
  - Idle: a shard with no write for the inactive time (5 min default) is flushed, so a quiet shard gives its buffer back.
- The caller refreshes (`Shard::refresh`), throttles and releases; the controller keeps only which shards it throttled.
- `Shard::idle_millis` gives the time since a shard's last write, from its clock.

Not changed, by design and listed: the native (off-heap) budget, because the Rust writer has one heap and no native index buffers; the thread pool and the scheduling timer, which are the node's; the shard states `availableShards` filters on, which the node's shard set owns.

## Acceptance criteria

- AC-1: a percentage size is a share of the heap, raised to the floor and capped at the ceiling; a size in bytes is used as it is whatever the floor and ceiling say.
- AC-2: writes below a thirtieth of the budget are not a check; the write that passes it is, once, and the count then starts again; of threads writing at once exactly the right number of checks are due.
- AC-3: under budget refreshes nothing; over it, the largest shards are refreshed first and only as many as bring the rest under the budget.
- AC-4: bytes a shard is already writing out are not counted twice, and a shard whose writing exceeds its buffer counts as zero.
- AC-5: past one and a half budgets the shards refreshed are throttled once; below it every throttled shard is released and none is throttled again.
- AC-6: a shard idle for the inactive time or longer is flushed; one idle for less is not.
- AC-7: `Shard::idle_millis` follows the shard's clock from its last index or delete.

## Test plan

`crates/writer/tests/indexing_memory.rs` (AC-1 to AC-6, pure, with a threaded check for AC-2); `crates/writer/tests/shard.rs` (AC-7).

## S3 request budget

None: nothing here touches the store. A refresh it asks for is the publish's own cost (one blob PUT and one swap per new segment, [segment publish](../segment-publish/SPEC.md)).

## Switch

None: the defaults are core's, and the budget is the node's existing indexing-buffer setting.

## Risks

- A node that never calls `check` never refreshes under pressure: the node wires the interval and the write hook (R6.5/R6.6 soak shows the buffer stays under the budget).
