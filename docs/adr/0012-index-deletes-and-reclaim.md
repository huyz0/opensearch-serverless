# 0012: Index deletes finish through a reclaim queue, and purge a shard by listing it

**Status:** Accepted after independent review, 2026-11-16.

## Context

Deleting an index removes four things: its descriptor register, its shards' heads, the aliases' mentions of it and its shards' bytes under `segments/<index>#<uuid>#<shard>/`. Only the descriptor names the other three, so a delete that removed it and then crashed would leave heads and bytes nothing can find. A writer that has not yet seen the delete can also re-create a head, or finish a publish it began, for up to a lease TTL plus the incarnation fence after it.

The Java shell (`IndexLifecycle`, `ReclaimQueue` at `3920dc36`) answers both with an intent: one small immutable object, `reclaim/<minute>/<index>#<uuid>`, written before the descriptor goes and taken two minutes later. Its bytes are `{"name":..,"uuid":..,"shards":n,"had_writer":b}`. A Rust node on the same bucket must file and take the same intents, or a delete begun on one runtime is never finished by the other.

The bytes of a deleted shard are found the only way they can be: a shard's blob names come from manifests written over time and from writers that may have crashed before any manifest, so the purge lists the shard's prefix. A delete is an operator's action. The request budget is per delete, never per index per unit of time.

Shard heads and the snapshot and point-in-time pins that must keep a shard's bytes are not ported yet; they arrive with the writer and the snapshot work.

## Decision

- Port the intent format, the queue and the lifecycle as the Java has them, sharing the key layout and bytes. A Rust intent reads on a Java node and the other way round. The Rust reader refuses a fractional shard count or one past 4,096, which no queue writes.
- A conditional-delete store files an intent before removing the descriptor: one PUT a delete. A store that ignores the condition (tombstone mode) files none, as in the Java.
- A drain walks the queue in key order, which is time order, and stops at the first bucket not yet due: a drain with nothing due is one LIST, whatever the queue holds, and one more per 1,000 due intents in buckets the caller owns. A page that ends inside another node's bucket resumes past it, so a bucket the caller does not own costs at most one more LIST. The Java lists the bucket directory, then each due bucket. Taken intents are deleted one at a time; S3 does not bill DELETE.
- The purge lists each unpinned shard's prefix and deletes what it finds: one LIST a shard of a deleted index, and one more per 1,000 blobs, as the Java's `BlobContainer.delete`. A shard whose writer may still publish is purged when its intent falls due rather than twice.
- Heads and pins are traits (`ShardHeads`, `ShardPins`) that the lifecycle takes explicitly. The crate ships no implementation that answers "no heads" or "no pins": one would purge bytes a Java snapshot names, which cannot be undone, and would drain a Java-filed intent without removing the Java writer's heads. So `delete_index` and `drain` are wired to no entry point until the real heads and pins are ported; the unwired-deletes check holds it: no crate outside `oss-meta` names `IndexLifecycle` or `ReclaimQueue`, and the change that ports heads and pins and wires a delete retires the check.
- A create refused for a name that holds the same uuid answers that record's generation: the uuid is minted per create, so this is the same create retried after a lost response. It costs one read on a refusal only.
- `BlobStore::validate_key` checks a key without a request, as the Java's `validateKey`. The filesystem store applies its own rule, 255-byte components, so a create whose longest shard key the store cannot hold is refused before its descriptor exists.

## Consequences

- Draining is the node runtime's to schedule. The Java drains on every node each collector tick, owning buckets by rendezvous, which costs N LISTs a tick for N nodes: requests that scale with nodes and time, never with indexes. The runtime that wires it may instead drain on one node a minute; either is within the rule, and the choice is recorded when it is made.
- Rust nodes cannot delete an index or drain the queue in a deployment until shard heads and snapshot pins are ported. Until then Java nodes do both, and an intent either runtime files is in the same place with the same bytes.
- The purge's deletes are sequential, one request a blob: about 410,000 round trips for 4,096 shards of 100 blobs. When REST wires `DELETE /<index>`, its purge runs after the acknowledgement, or the delete defers every purge to the reclaim by filing the intent with `had_writer` true, which Java nodes read the same way; batched deletes come with `oss-blob`'s DeleteObjects. That choice is the REST change's, and its spec records it.
- A purge that fails leaves storage nothing serves: it is counted in the delete's answer, not retried.

## Addendum: heads and pins ported, delete wired

Shard heads (`oss_cluster::heads::HeadRegisters`) and point-in-time pins are ported and `DELETE /{index}` and a reclaim drain on each node are wired ([index-delete spec](../specs/index-delete/SPEC.md)); the unwired-deletes check is retired. The purge runs inside the request for a shard no writer held; a shard a writer held waits for the intent. Snapshot pins are an extension point (`SnapshotPins`) with no source: the Rust node takes no snapshots and snapshot management is out of scope, so a deployment that still has Java shallow snapshots of an index must not delete it from a Rust node until a source is supplied. A view that cannot be read, or a capture in progress, pins every shard.

The purge cost note above is answered by the [purge follow-up](../specs/index-delete-purge/SPEC.md): a page of blobs is one batch delete, an index above `inline_purge_max_shards` (16 on a node) is purged at the reclaim, and a shard a pin kept is filed again until the pin ends.
