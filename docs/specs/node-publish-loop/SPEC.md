# The writer role publishes the shards it holds

## Serves

Parity plan R7.6, node assembly: the writer role stages writes in the node log, and a shard's commit reaches readers only when it is published. `WriterRole::publish_due` is the pass a timer runs: it asks [lazy publish](../writer-lazy-publish/SPEC.md) which held shards are due and publishes each through `ShardPublisher` ([segment publish](../segment-publish/SPEC.md)). Ported from the shell's `BackgroundReconciler` publish pass.

## Change

- `oss-node`'s `writer` module gains `Publishing`: the store, the cluster base, the node id, a `Policy` and a clock, and `WriterRole::with_publishing` / `publish_due`.
- A write staged in the log marks its shard written. `publish_due` visits each held, unreleased shard: when `LazyPublish::due` says so it refreshes the engine, lists the commit's files, publishes them as this node at the shard's term, tells the tracker, and remembers the manifest as the shard's newest commit (`staged`). A failed publish leaves the shard due; the pass reports it and goes on.
- A shard released or opened again is forgotten by the tracker and by its publisher, so a reopened shard is due once again.
- Not changed, by design and listed: the timer that calls `publish_due`, a per-index `refresh_interval` age (every shard uses the node's age), the log ordinal a publish records (unknown, `-1`), pushing a published commit to readers.

## Acceptance criteria

- AC-1: a held shard with a write is published by the first pass, whatever the age; its manifest becomes `newest_commit`; the pass makes one blob PUT and one register swap and no LIST.
- AC-2: a shard with no write publishes nothing and makes no request.
- AC-3: after a publish, a later write is published when its age has passed rounded up to the tick, and not before; a pass with no new write publishes nothing.
- AC-4: with age 0 every pass publishes every shard with a write since its last publish.
- AC-5: a publish the register refuses (a newer term holds it) is reported as failed, leaves the shard due, and the next pass tries it again.
- AC-6: a shard opened again is published again at its first write, as a shard never published.

## Test plan

`crates/node/tests/writer_publish.rs`: a real `WriterRole` on the metered emulator, a manual clock, writes through `RoleTarget`, request counts read from the meter.

## S3 request budget

Per publish one blob PUT and one register swap, no LIST ([segment publish](../segment-publish/SPEC.md)); a pass over shards with nothing to publish makes none. Publishes per shard are bounded by the lazy age, not by writes.

## Switch

`serverless.wal.node_log.publish_max_age_millis` and `serverless.publish.max_unpublished_operations`, registered in the node's registry; age 0 is the eager mode and AC-4 pins it.

## Risks

- A publish runs under the shard's slot lock, so writes to that shard wait for it: one blob PUT and one swap, and the lazy age bounds how often.
