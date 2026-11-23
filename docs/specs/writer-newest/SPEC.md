# The writer answers a reader's newest-commit ask

## Serves

Parity plan R7.6, the carry-over from the push work ([reader-push-wire](../reader-push-wire/SPEC.md), [writer-push-sender](../writer-push-sender/SPEC.md)). The answering side of the shell's `NewestCommits` (`handle`, `local`, `SegmentPublisher.newestCommit`): a writer that stages commits writes the register only as a checkpoint, so a reader asks the writer for the commit it last published.

## Change

- `WriterRole` remembers, per held shard, the manifest its last publish produced: `staged(index_uuid, shard, manifest)` (false when the shard is not held, or the manifest is over `MAX_NEWEST_BYTES`) and `newest_commit(index_uuid, shard)`. The memory is dropped when the shard is released and replaced when it is opened again, like `ShardReconciler.newestCommit` answering null for a shard with no publisher.
- `oss-node` gains `newest::NewestHandler`, the transport handler of `internal:serverless/commit/newest` over a `WriterRole`: it decodes a `NewestRequest` and answers a `NewestResponse` with the commit or none. It never reads the store and never waits on a write in flight.
- Not changed: the reader's ask on a cold open (`NewestCommits.newerThan`), the publish call that calls `staged`, registering the handler at node assembly (it is registered behind the transport guard like the push handler).

## Acceptance criteria

- AC-1: after `staged`, the handler answers the manifest for that index uuid and shard, in the shell's wire form.
- AC-2: a shard never published, one not held, another uuid, and a released shard answer none.
- AC-3: a later `staged` replaces the earlier; opening the shard again drops it.
- AC-4: a manifest over `MAX_NEWEST_BYTES` is refused and the earlier answer stands; a shard not held refuses `staged`.
- AC-6: a manifest staged while the shard is released is not kept: the held check and the insert are one step under the shards lock.
- AC-5: a message that does not decode is refused as a bad request; answering makes no store request.

## Test plan

`crates/node/tests/roles/writer_newest.rs`: a real `WriterRole` on the emulator and the handler called with encoded requests; the meter is read before and after.

## S3 request budget

Zero: the answer is memory.

## Bounds

At most one manifest per held shard, each at most `MAX_NEWEST_BYTES` (4 MiB, the largest manifest a reader reads); freed with the shard. There is no node-wide byte total, as the shell holds one manifest per publisher too; the held-shard cap that node assembly adds bounds it.
