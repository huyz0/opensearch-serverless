# An idle writer makes no request for its shards

## Serves

Parity plan R8 exit gate: an idle held shard makes at most 16 requests a shard-hour and a hibernated one none. Invariant I-1 (idle makes no request until the shard is due).

## Change

Two request-budget rows become `enforced`: `held_idle.index_requests_target` (held shard, 0) and a new `hibernated_idle.index_requests_target` (hibernated shard, 0). A test holds four published shards, runs one simulated hour of every periodic pass a writer node runs over its shards (the publish pass every 250 ms, the janitor pass every 60 s, the hibernate pass every 250 ms) by calling each pass the number of times an hour holds, and counts every request. Nothing in the product changes. The node-level cost (lease renewal) is its own rows, `lease.idle.*`.

## Acceptance criteria

- AC-1: an hour of passes over four held, published, swept shards makes no request.
- AC-2: the same hour over the same shards asleep makes no request.
- AC-3: the shards are still held (AC-1) or asleep (AC-2) after the hour, so the zero is not an empty role.

## Test plan

`crates/node/tests/idle_hour.rs`. Red first: the rows are blocked or absent, so the gate refuses. A pass that probed each held shard's head or descriptor (the Java's 12 head reads a shard-hour) would fail AC-1.

## S3 request budget

Unchanged; this pins it at zero.

## Switch

None.

## Risks

The passes are called by the test, not by the node's threads, so a timer added outside these passes is not counted; the timers are listed in the test's comment and a new one needs a line here. The orphan sweep (off by default) and the reclaim drain are not run.
