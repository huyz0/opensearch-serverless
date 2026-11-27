# Lazy publish: when a shard is due for a commit

## Serves

Parity plan R8.5, the request-cost goal of one write log per node: a publish costs a segment blob and a register swap, so a busy shard must not publish on every write. Ported from `DirtyPublishing.lazyPublishDue` and `BackgroundReconciler.setLazyPublish` (huyz0/OpenSearch `feature/serverlessshell`). Builds on [segment publish](../segment-publish/SPEC.md).

## Change

- `oss-writer` gains `publish::lazy` (`crates/writer/src/publish/lazy.rs`): `LazyPublish`, a sans-IO tracker that answers "is this shard due for a publish now?" and makes no request. The caller (the node writer role, R7) reports each write, asks `due`, calls `ShardPublisher::publish`, then reports `published`.
- A shard is due when it was never published by this node (once, so a new index is searchable at once), or its oldest unpublished write is at least its publish age old, the due time rounded up to the next tick (the write log's window), or it holds at least `max_unpublished_operations` operations beyond its last publish (bounds the replay a successor inherits).
- A shard's publish age is its index's `refresh_interval` when set, the node's `publish_max_age_millis` otherwise, and never by age when the interval is `-1` (size and release only).
- Two node settings with the Java's keys and defaults: `serverless.wal.node_log.publish_max_age_millis` (1,000 ms; 0 is eager as in the Java: every shard with unpublished writes is due, whatever its index's interval) and `serverless.publish.max_unpublished_operations` (10,000; 0 never early).
- Not changed, by design and listed: wiring into a node writer role and the settings into the node's registry (R7 owns both), the tail overlay's raised age and the tenant guardrail's stretched age (the caller passes the age it has settled), the debounce and minimum interval of the non-lazy path.

## Acceptance criteria

- AC-1: a shard never published is due once it has a write, and not before; a shard with no write is never due.
- AC-2: a shard with unpublished writes is not due before its age has passed and is due at it, the age counted from its oldest unpublished write, not its latest.
- AC-3: the due time is rounded up to the tick: age 1,000 ms from a write at 10,001 ms with a 250 ms tick is due at 11,250 ms, not 11,001 ms.
- AC-4: `max_unpublished_operations` makes a shard due before its age; 0 never does.
- AC-5: an index's `refresh_interval` replaces the node's age; `-1` is never due by age, only by operations.
- AC-6: after `published`, a shard is not due until a later write, and that write's age counts from the write, not from the publish; a write whose stamp a publish cleared is aged from the next mark or the next ask that finds it.
- AC-7: with age 0 (eager), whatever the index's interval, every shard with an unpublished write is due and one with none is not.
- AC-8: `forget` drops a shard's state, so a reopened shard is due once again.
- AC-9: the two settings form a valid registry with the Java's defaults.
- AC-11: with the `writer` feature the node's settings registry holds both settings (`the_nodes_settings_form_a_valid_registry`).
- AC-10: a lazy publisher over the emulator, writing every 250 ms to 20 shards for 10 s at age 1 s, makes 2 PUT and 0 LIST per publish and 8 publishes per shard (at 0, 1.25, 2.5 ... 8.75 s) where an eager loop makes 40.

## Test plan

AC-1 to AC-8 and AC-9: unit tests in `lazy.rs`, each seen failing against a stub that answers "not due". AC-10: `crates/writer/tests/it/lazy_publish.rs` over the metered emulator with a manual clock, asserting the request counts and the publish count against what an eager loop would have made.

## S3 request budget

Unchanged per publish: 1 blob PUT and 1 register swap, 0 LIST ([segment publish](../segment-publish/SPEC.md)). Lazy publish makes fewer publishes: a shard written every 250 ms publishes every 1.25 s at the default, a fifth of an eager loop's, and an idle shard makes none. `LazyPublish` itself makes no request.

## Switch

`serverless.wal.node_log.publish_max_age_millis`: 1,000 ms by default, 0 for eager. Measured in the Java (re-measure after #40): at refresh 1 s, 1.31 against 1.81 PUT a write; at 5 s, 0.40 against 1.10. The tests pin both modes. Search reads only published commits until the tail overlay lands, so this is also the longest a write takes to be searchable; the index's `refresh_interval` is the per-index override.

## Risks

- A write never published because its mark was lost: `published` clears the stamp and the next write re-stamps it, and a shard with writes but no stamp is aged from the first `due` that sees it (AC-6): the cost is a delay of at most one age.
- Clock skew between marks and asks: ages use the caller's one monotonic clock, saturating, never wrapping.
