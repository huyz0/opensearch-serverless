# A node retires: it publishes what it holds and gives its heads back

## Serves

Roadmap R12 (ready, drain, join): a node told to stop leaves the cluster in one step instead of letting its lease lapse. Builds on [writer-give-back](../writer-give-back/SPEC.md) and [node-leases](../node-leases/SPEC.md). Decision: [ADR 0048](../../adr/0048-retiring-a-node.md).

## Change

- `WriterRole::retire(now)` publishes every shard that holds writes no commit holds, puts each held shard to sleep as the hibernation pass does at an idle time of 0, and gives back the head of each one (repeated until a pass gives none). It answers (a `HibernatePass`) how many shards it slept and gave back, the shards that failed to publish, and the shards busy (written to meanwhile).
- `Node::retire(self)` stops the HTTP server and the node's timers first, so no write wakes a shard again, then runs `retire` on the writer role, then drops the node as before: the member lease is released.
- `main` retires the node on `SIGTERM` or `SIGINT`, then exits 0. A retire that finds a failed shard still releases the lease and the heads it could, and exits 1.
- No setting: stopping is not a behaviour that can be off. A killed process (`SIGKILL`, a crash) still lapses in one lease lifetime, as before.

## Acceptance criteria

- AC-1: after `retire`, the head of a shard that took a write names no owner, and the document is in a published commit (a second node on the same store finds it).
- AC-2: a second node writes to that shard at once after `retire`, with no wait for the first node's lease, and sees both documents.
- AC-3: retiring a role that holds a shard with an unpublished write publishes it and gives its head back; a second retire finds nothing and makes no request.
- AC-4: a role holding more shards than one give-back pass may take (`GIVE_BACK_PER_PASS`) has every head given back by one retire.
- AC-5: after `Node::retire` the HTTP port refuses connections and the member lease is released.
- AC-6: a retire of a node holding no shard slept and gave back nothing, and still releases the lease.
- AC-7: the `oss-node` binary, sent `SIGTERM`, retires, prints what it did, exits 0, and the head of a shard it wrote names no owner.

## Test plan

`crates/node/tests/server/boot_retire.rs` over the fs store for AC-1, AC-2, AC-5, AC-6; `crates/node/tests/writer_retire.rs` over the proactive rig for AC-3 and AC-4.

## S3 request budget

Per held shard: the publish it owed (one blob PUT and one register swap, as any refresh) and the give-back's 2 requests (one GET and one swap, [writer-give-back](../writer-give-back/SPEC.md) AC-1); one lease release. Requests scale with held shards at the moment of a stop, once, never with elapsed time.

## Switch

None, by the argument above: a stop happens once, adds no request while running, and without it the node's shards wait out a lease lifetime.
