# Bucket member on delete: a deleted index leaves its bucket

## Serves

R11. Closes the first risk in [node-bucket-wiring](../node-bucket-wiring/SPEC.md): with `serverless.buckets.enabled` on, an index joins a bucket at create, and nothing took it out when the index was deleted, so a bucket's manifest kept an entry for every index ever created in it and filled up (member and size limits, [bucket-assignment](../bucket-assignment/SPEC.md) AC-5). Under [ADR 0026](../../adr/0026-bucket-manifests-in-the-writer.md). With the switch off, or for an index that names no bucket, nothing changes.

## Change

- `oss_meta::Deleted` carries the deleted incarnation's `uuid` and the `bucket` its descriptor named (read with the descriptor the delete already reads: no extra request).
- `DeleteService::with_bucket_leave(leave)`: after the descriptor is deleted, a bucketed index's member is taken out of its bucket through the node's `BucketWiring::leave`, in `DELETE /{index}` and in `DeleteService::remove` (a failed restore's undo). Best effort, as at a failed create: the descriptor is gone, so a member entry left behind is never served, and the delete does not fail for it.
- Boot gives the delete service the leave only when the bucket wiring exists.

## Acceptance criteria

- AC-1: deleting a bucketed index calls leave with its bucket and uuid once; deleting an unbucketed index calls nothing.
- AC-2: a leave that fails does not fail the delete (200, the descriptor gone).
- AC-3: a delete of a missing index, and a `remove` naming another incarnation, call nothing; a `remove` of the named incarnation does.

## Tests

`crates/node/tests/server/index_delete.rs`.

## S3 request budget

Switch off, or an unbucketed index: unchanged, 0 extra. A bucketed index: one GET and one conditional PUT of its bucket's manifest per delete (retried up to 16 times under contention, as a join is), 0 LIST. A delete is not a hot path and the request count is per delete, not per record or elapsed time.

## Risks

- A node that dies between the descriptor delete and the leave leaves an unused member entry, as a node that dies between a join and the descriptor write does; nothing clears those yet.
