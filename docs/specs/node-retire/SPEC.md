# A node retires: it publishes what it holds and gives its heads back

## Serves

Roadmap R12 (ready, drain, join): a node told to stop leaves the cluster in one step instead of letting its lease lapse. Builds on [writer-give-back](../writer-give-back/SPEC.md) and [node-leases](../node-leases/SPEC.md). Decision: [ADR 0048](../../adr/0048-retiring-a-node.md).

## Change

- `WriterRole::retire(now)` publishes every shard that holds writes no commit holds, puts each held shard to sleep as the hibernation pass does at an idle time of 0, and gives back the head of each one (repeated until a pass gives none). It answers (a `HibernatePass`) how many shards it slept and gave back, the shards that failed to publish, and the shards busy (written to meanwhile).
- `Node::retire(self, timeout_millis)` stops the HTTP server and the node's timers first, so no write wakes a shard again, then runs `retire` on the writer role, then drops the node as before: the member lease is released.
- `main` retires the node on `SIGTERM` or `SIGINT`, then exits 0. A retire that finds a failed shard still releases the lease and the heads it could, and exits 1.
- Retiring is bounded: `OSS_RETIRE_TIMEOUT_MS` (default 25,000, under the usual 30 s grace; at most an hour) is a deadline checked before each shard and each give-back pass. Shards not reached are counted `unfinished` and left to the lease; the lease is released regardless. A timeout of 0 skips publishing and giving back entirely (the stop is then as before, plus the lease release). Shards pinned here, written to meanwhile, or whose head had moved are counted `busy` and left held.
- A second `SIGTERM`/`SIGINT` while retiring ends the process at once with status 130. A signal handler that cannot be installed is a start failure (status 1). Exit status: 0 retired or left to the lease, 1 a shard failed to publish or the deadline passed first, 2 a configuration error. A signal during `start` takes the default action.
- A killed process (`SIGKILL`, a crash) still lapses in one lease lifetime, as before.

## Acceptance criteria

- AC-1: after `retire`, the head of a shard that took a write names no owner, and the document is in a published commit (a second node on the same store finds it).
- AC-2: a second node writes to that shard at once after `retire`, with no wait for the first node's lease, and sees both documents.
- AC-3: retiring a role that holds a shard with an unpublished write publishes it and gives its head back; a second retire finds nothing and makes no request.
- AC-4: a role holding more shards than one give-back pass may take (`GIVE_BACK_PER_PASS`) has every head given back by one retire.
- AC-5: after `Node::retire` the HTTP port refuses connections and the member lease is released.
- AC-8: `OSS_RETIRE_TIMEOUT_MS` defaults to 25,000, accepts 0 to an hour, refuses text and larger values; with 0 the node stops and its shards stay held.
- AC-9: with more shards asleep than one give-back pass takes, and one head moved to another node, every other head is given back, the moved one is counted busy and left, and no record is left behind.
- AC-10: a retire whose deadline has passed makes no request, leaves every shard held and counts them `unfinished`.
- AC-6: a retire of a node holding no shard slept and gave back nothing, and still releases the lease.
- AC-7: the `oss-node` binary, sent `SIGTERM`, retires, prints what it did, exits 0, and the head of a shard it wrote names no owner.
- AC-11: a retire that blocks (a store call that never returns, a request that never finishes) is given up on at `OSS_RETIRE_TIMEOUT_MS` plus a 5 s grace: the process prints it, leaves the shards to the lease and exits 1. The deadline is counted from before the server's shutdown, and is checked inside a give-back pass, per shard.

## Test plan

`crates/node/tests/server/boot_retire.rs` over the fs store for AC-1, AC-2, AC-5, AC-6, AC-7, AC-8; `crates/node/tests/writer_retire.rs` over the proactive rig for AC-3 and AC-4.

## S3 request budget

Measured on the emulator and pinned as `writer.retire.one_shard.requests` (6, provisional): a shard with one unpublished write costs the publish, the hibernation and the give-back's 2 requests. It scales with the shards held at the moment of a stop, not with nodes and bytes: this is the accepted exception, once per stop, bounded by the deadline, and none while running. The alternative is the lapse path: every held shard waits out the lease lifetime, and a successor's take then pays a revocation.

## Switch

`OSS_RETIRE_TIMEOUT_MS`, default 25,000, 0 off (see the ADR for the downside weighed).
