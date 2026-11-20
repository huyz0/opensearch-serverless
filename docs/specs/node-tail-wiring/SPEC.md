# The assembled node serves and uses the tail overlay

## Serves

Parity plan R7.6, the tail overlay's node assembly ([ADR 0018](../../adr/0018-tail-overlay-in-the-reader.md), Accepted). Builds on [node-overlay-wiring](../node-overlay-wiring/SPEC.md), [node-tail-links](../node-tail-links/SPEC.md), [writer-tail-push](../writer-tail-push/SPEC.md) and [node-handlers](../node-handlers/SPEC.md).

## Change

`oss-node` (features `reader` and `writer`) gains `tail_node`:

- `LeaseWriters`: a `LiveWriters` over the node's leases and a clock; a node is live when its lease is unexpired and not revoked.
- `Assembled::wire_tail(parts, budget)` (the one addition to `assemble.rs`) builds, when `serverless.search.tail_overlay.enabled` is on, the reader side through `tail_overlay::wire` (a fetcher over `LeaseWriters`) and the writer side: a `TailKeep`, `TailServe`, `TailPush` over a `TransportTailLink`, facts read from the writer role, and a flush timer. It returns the `handlers::Registry` holding `internal:serverless/tail/fetch` (`TailFetchHandler`) and `internal:serverless/tail/push` (`TailPushHandler`), for whoever starts the transport server. Off, it builds nothing and returns `None`.
- `WriterRole` gains `set_tail_sink` and `max_seq_no`: after a group is staged its records go to the sink as acknowledged, a publish tells it the checkpoint, and a released shard tells it so. With no sink set the write path is unchanged.

A released shard is forgotten by the serve and keep through the same sink (not asserted by a test here: a release follows a failed log write, which the writer role's own tests cover).

Not changed: the transport server and client themselves (the node's `main`), the per-index `index.serverless.tail_overlay` setting (the node setting decides), and waiting for readers to confirm before acknowledging.

## Acceptance criteria

- AC-1: with the setting off (default) `wire_tail` returns `None`, installs no overlay source, and a write makes no tail record and no push.
- AC-2: `LeaseWriters` shows a node live only while its lease is unexpired and not revoked, and gives its address and ephemeral id.
- AC-3: with it on, the registry serves exactly the tail fetch and tail push actions.
- AC-4: with it on, a bulk acknowledged on the writer is kept; a reader's tail fetch over the registry's handler answers it and subscribes the reader; the next acknowledged write is pushed to the reader's handler on flush and held there.
- AC-5: a publish drops what the commit holds from the keep.
- AC-6: a second `wire_tail` returns `None`.

## Test plan

`crates/node/tests/tail_node.rs`: AC-2 over fake leases and clock; AC-1, AC-3, AC-6 over an assembled node; AC-4 and AC-5 over an assembled node whose fetch and push links call the other side's handlers in process. Each is written first and seen failing (the module does not exist).

## S3 request budget

Zero added: fetches and pushes are node to node, and the flush timer touches no store.

## Bounds

The keep's (20,000 records and 16 MiB a shard, 256 MiB a node), the push queue's (20,000 a shard), the builder's, and one flush thread.

## Switch

`serverless.search.tail_overlay.enabled`, default `false` until measured: off, no keep, push, handler, timer or overlay exists.

## Risks

The flush runs on its own thread, never in a write, so a slow reader cannot slow an acknowledgement. Ownership in doubt comes from a caller-supplied predicate until the node's lease view feeds it.
