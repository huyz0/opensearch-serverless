# Descriptor-backed bucket heads: membership from the descriptor

## Serves

R11. Ported from the shell's `DescriptorBucketHeads` (`note`, `forget`, `register`, `bucketOf`, `othersOpenOn`, `MEMBERSHIP_CACHE_MILLIS`) at huyz0/OpenSearch `429b4cbc`, on [bucket-heads](../bucket-heads/SPEC.md) and [bucket-head-release](../bucket-head-release/SPEC.md), under [ADR 0026](../../adr/0026-bucket-manifests-in-the-writer.md). A descriptor names its bucket, so every node answers alike from what it reads anyway to route the shard. The answer is kept for an hour per index incarnation, because a node checks the heads of what it holds every few minutes and a descriptor read per member per check would cost more than the bucket saves.

## Change

- `oss_cluster::heads::DescriptorBucketHeads::new(descriptors, clock)` implements `BucketHeads`. `Descriptors::describe(index)` answers an index's uuid and bucket, or none for an index with no descriptor.
- `bucket_of`: a shard other than 0, and an index name starting `#`, are never members and cost nothing. Otherwise the index's bucket is answered from a read less than `MEMBERSHIP_CACHE_MILLIS` (one hour) old, else from one descriptor read. A caller's uuid that is not the cached one forces a read, and one that still differs from the descriptor's is no member (a recreated index). A missing descriptor forgets the entry and answers none; a descriptor naming no bucket answers none.
- `note(index, uuid, bucket)` records an answer from a descriptor a caller has just read, so the head operation that follows costs no second read; `forget(index)` drops it, for an index deleted or moved out of its bucket.
- `register(node_id, open)` registers a node's open shards, read on each release of a member; `others_open_on` is true when a registered node holds shard 0 of another index open whose bucket is the one asked, judged by the open shard's own uuid. An unregistered node holds none.

Not here: the node's `Descriptors` over the descriptor store, `with_bucket_heads` in the node's boot, the publisher's member register.

## Acceptance criteria

- AC-1: a shard other than 0 and an index named `#...` are no members, with no descriptor read.
- AC-2: a bucketed index is a member of the descriptor's bucket with the descriptor's uuid; an index naming no bucket and one with no descriptor are not.
- AC-3: the answer is kept for exactly the cache time: a second ask just under it reads nothing, one at it reads again.
- AC-4: a caller's uuid other than the cached one forces a read; a uuid that still differs from the descriptor's is no member, and one that matches is.
- AC-5: `note` answers without a read, and `forget` makes the next ask read.
- AC-6: `others_open_on` is true for another open member of the bucket on a registered node, false for the excepted index itself, a member of another bucket, a non-zero shard, and an unregistered node.

## Tests

`crates/cluster/tests/leases/descriptor_heads.rs`, with a counting `Descriptors` over a map and a clock the test moves.

## S3 request budget

A member lookup is 0 requests when noted or cached and one descriptor read at most per index per hour; `others_open_on` reads descriptors only for the node's open shards not yet cached. 0 LIST.

## Risks

- A cached answer is an hour old, as in the Java: an index promoted out of its bucket must `forget` it, and the node's descriptor-change listener is what calls it (wiring not here).
