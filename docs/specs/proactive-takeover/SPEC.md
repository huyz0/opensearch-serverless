# Proactive takeover of the shards a dead node's log names

## Serves

Parity plan R8.2 and [ADR 0021](../../adr/0021-proactive-takeover-from-the-node-log.md): a dead node's shards that hold unpublished writes are taken before a write asks, without a claims registry. Builds on [node takeover](../node-takeover/SPEC.md), [writer takeover](../writer-takeover/SPEC.md), [node log takeover](../node-log-replay/SPEC.md) and [janitor node log](../janitor-node-log/SPEC.md).

## Change

- `oss-node` (feature `writer`) gains `ProactiveTakeover`: given a dead member (expired or revoked lease that names log epochs) and the node's leases, only when this node is one of the dead node's reapers (the first `serverless.takeover.proactive.reapers`, default 2, live ingest writers by `reapers_for`; the second starts at the first refresh at or after the lease's expiry plus one refresh interval that still sees epochs named) it fences the epochs as the successor, reads the shards the log names (`NodeLogTakeover::named`), checks each (descriptor and head) and takes those still needed through the existing `HeadOpener` take (acquire, restore, replay, publish, settle).
- `oss-cluster`'s refresh reports a dead member whose lease names epochs (new API; it reads nothing the refresh does not read, or one counted lease read). The node calls the pass when it sees a dead member naming epochs, at most once per (dead node, epoch set) until the epochs change; it is bounded by `serverless.takeover.proactive.max_concurrent` (default 4) and a per-pass deadline `serverless.takeover.proactive.pass_millis` (default 30,000). Each take reserves its engine's worst case from the node pool first; no room defers the shard.
- A switch, `serverless.takeover.proactive.enabled`, default **off**; flipped in its own PR on the evidence ADR 0021 lists.
- Not changed: lazy takeover (stays the fallback, unchanged with the switch off or on), the head, the log and register formats, the janitor crate's collector (it frees the log once the takes publish; the node reuses the reaper's fence for it through an adapter, AC-19), claims (none written or read), bucket heads, per-shard logs of a Java node.

## Acceptance criteria

- AC-1: with the switch off a dead node's shard is taken only by a write, and no proactive request is made (meter equal to today's).
- AC-2: with the switch on, a dead node's log naming shards A and B, unpublished, taken by this node (the winner of both): after the refresh that sees the death, both are held by this node at term + 1, each replayed and published, and the next write to either makes no takeover request.
- AC-3: a shard the dead node held that its log does not name (everything published) is not touched by the pass: no head read, no restore.
- AC-4: with three live nodes and two reapers, only the two reapers fence the dead node's epochs; the third makes no request about it.
- AC-5: the second reaper starts at lease expiry plus one refresh interval if epochs are still named, takes only shards still needed, and costs at most twice the first's checks; if the first reaper is down, the second takes them all (up to its bounds).
- AC-6: more shards named than `max_concurrent` are taken at most that many at once; a shard or header that cannot reserve from the pool is deferred, never refused to a client, and the pass reserves for the headers and the named set before reading them.
- AC-7: a pass past `pass_millis` stops starting takes and lets those in flight finish; shards not started stay lazy.
- AC-8: the same dead node and epoch set starts no second pass on a reaper that completed it; a reaper whose pass deferred shards runs again on a later refresh; a new epoch set starts one.
- AC-9: two reapers disagreeing (stale digests) never produce two owners: the loser's acquire is refused before any restore, and the shard is held once. With no live writer there is no reaper and no request.
- AC-10: a dead node whose lease names no epochs makes no pass and no request beyond the refresh's own.
- AC-11: after the takes publish, the janitor's `collect` of the dead node answers collected, its lease names no epochs, and the live nodes stop reading it on each refresh (meter: the dead lease's reads per refresh go to zero).
- AC-12: a take that fails (log unreadable, publish refused by a newer term) holds nothing, leaves the head naming its writers, and the shard is still taken lazily by the next write.
- AC-13: a named shard whose index name is unknown, whose index was deleted or recreated under another uuid, or whose head is owned by a live node or a third dead node is not restored; a nameless shard is left to the lazy path.
- AC-14: a dead node that logged, published and was then forgotten (lease names no epochs, head still names it as a writer) is left to the lazy path and makes no pass.
- AC-15: the collector's `collect` interleaved with a take (the forget swap landing during a taker's re-read) ends with the shard held once, nothing deleted that a shard still needs, and the log collected afterwards.
- AC-16: the requests of a pass are exactly the budget row's function of epochs, named shards and shards taken (fence creates and probes, header GETs, a descriptor and a head GET a named shard, plus the lazy take's), checked at 3, 30 and 200 simulated nodes to show the pass costs the same at each; 0 LIST. The pre-check decisions are made on the view at pass start. The row is written with the first test, before the code.
- AC-18: a write routed by placement to a shard a reaper holds reaches it or hands it off within a request bound stated in the budget row.
- AC-19: the janitor's collect of the dead node after a pass reuses the reaper's fence (no second fence create) and the new refresh surface distinguishes a revoked member from an expired one.
- AC-20: a header over `max_header_bytes` is refused and its shards stay lazy.
- AC-17: the four settings (`enabled`, `reapers`, `max_concurrent`, `pass_millis`) are registered and validated; a java-written node log (the recorded fixtures) is read by the pass.

## Test plan

`crates/node/tests/proactive_takeover.rs` over the emulator with a fleet of nodes sharing a store and the metered store, extending `writer_takeover.rs`'s two-node set-up with a clock that makes the first node dead. Each seen failing first against a pass that does nothing. AC-4, AC-5, AC-9 with three nodes; AC-11 and AC-15 with the real janitor collector; AC-16 with a simulated fleet. Budget rows added to `docs/request-budgets.md` with the implementation.

## S3 request budget

Per dead node, per reaper (not per node): the fence of its epochs and their headers, a descriptor and a head read a named shard, then per shard taken exactly what a lazy take costs ([node takeover](../node-takeover/SPEC.md)). No LIST, no new PUT, nothing a time or an index count scales. Saves, once the log is collected, the recurring dead-lease read every node makes each refresh. The cost is flat in fleet size by construction and held by AC-16.

## Switch

`serverless.takeover.proactive.enabled`, default off, overridable per cluster. Flips on by the rule in the ADR; existing takeover tests pin the off behaviour, new tests the on.

## Risks

- A reaper that fences a node only partitioned: it loses the epoch and opens a new one, as with any takeover.
- Memory from restoring many shards at once after a zone loss: bounded by AC-6.
- A node taking a shard it will not be routed writes for: a stale digest can make reapers disagree: AC-9. No REST surface changes, so no compatibility criterion.
