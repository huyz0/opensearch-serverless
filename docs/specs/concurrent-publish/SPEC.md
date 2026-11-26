# Concurrent publish pass, so node packs are shared

## Serves

Parity plan R11.1 (node packs), [node-pack-wiring](../node-pack-wiring/SPEC.md) and [ADR 0024](../../adr/0024-node-packs-in-the-writer.md): a pack holds the small segment blobs of every shard that publishes within the linger. Today they never share: `WriterRole::publish_due` publishes held shards one after another, and one node-wide `Publishing::gate` is held from taking a shard's snapshot to its publish. A node-level test with two due shards and a 1 s linger wrote two packs, not one, so with the switch on every publish is a lone pack that also waits the linger.

## Change

- New node setting `serverless.segments.node_pack.publish_threads` (node scope, default 8, 1 to 64, env `OSS_NODE_PACK_PUBLISH_THREADS`). It applies only while `serverless.segments.node_pack.enabled` is on; with packs off the pass is exactly the serial pass of today and the node-wide gate stays.
- With packs on, `publish_due` hands the shards it finds due to at most `publish_threads` scoped threads (`std::thread::scope`, no pool of its own, no thread outlives the pass), each running today's `publish_shard` for one shard.
- With packs on, the gate is per shard (a map of one `Mutex<()>` a held shard, created on first use, dropped with the shard), so a shard still never has two publishes in flight and its commits are published in the order they were taken, while different shards no longer wait for each other. The checkpoint path takes the same per-shard gate.
- The pass's result is unchanged in meaning: `published` counts shards published, `failed` lists each failed shard with its reason, and a failed shard does not stop the rest.
- Not changed: the pack writer, the publisher, the lazy policy, what a shard publishes, with packs off anything.

## Acceptance criteria

- AC-1: with packs off, `publish_due` publishes due shards serially on the calling thread and takes the node-wide gate, as before; the existing publish tests pass unchanged.
- AC-2: with packs on, two due shards of one pass land in one pack: one pack PUT, one ledger create, two register swaps, and the manifests of both name that pack.
- AC-3: with packs on, no more than `publish_threads` publishes of a pass are in flight at once (observed by a counter around the publish), and with `publish_threads` 1 the pass is serial.
- AC-4: with packs on, a shard never has two publishes in flight: a second pass overlapping the first (from the timer and a retire, say) waits on the shard's gate, and a shard's commits are published in the order taken.
- AC-5: with packs on, a failing shard is reported in `failed` with its reason and the other due shards are published.
- AC-6: with packs on, the pass reports the same `published` and `failed` as the serial pass would for the same shards.
- AC-7: the setting is in the node's registry with its default and bounds, and `OSS_NODE_PACK_PUBLISH_THREADS` is read and bounded like the other pack variables.

## Test plan

`crates/node/tests/roles/writer_packs.rs` over the emulator and its meter (AC-2, AC-4, AC-5, AC-6; AC-3 with a counting wrapper around the store's PUT); AC-1 is the existing publish suite; AC-7 beside the other pack settings. Each written first and seen failing.

## S3 request budget

Packs off: unchanged. Packs on: a pass of `n` due small shards within the linger is 1 pack PUT, the first ledger create, and `n` swaps, instead of `n` packs (`n` pack PUTs) as today; nothing is added per index or per elapsed time, and a pass with nothing due makes no request and starts no thread.

## Switch

`serverless.segments.node_pack.publish_threads`, default 8, which does nothing while packs are off. Packs on is the switch; this makes it do what ADR 0024 measured. Memory: each publish streams through the store's pooled buffers (a 64 KiB buffer each), so the extra memory is at most `publish_threads` buffers and their open snapshots a pass; the bound is per pass, and a request-path refresh or a retire pass publishing a different shard at the same time adds its own, as it always could. The pass starts no more threads than the setting.

## Risks

A node-wide gate removed for packs-on deadlocking or reordering a shard's commits: AC-4 holds it. Lock order with the shard slot lock and `Publishing`'s other mutexes: the per-shard gate is taken first, then the slot, as the node-wide gate was. A thread panicking in a publish: `thread::scope` propagates it, so the pass fails loudly instead of losing a shard.
