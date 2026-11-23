# The writer pushes acknowledged writes to subscribed readers

## Serves

Parity plan R7.6, the tail overlay's push half: the `acked`, `flush`, `send` and `confirm` of the shell's `TailPush`. Builds on [writer-tail-keep](../writer-tail-keep/SPEC.md), [writer-tail-fetch](../writer-tail-fetch/SPEC.md) and [tail-wire](../tail-wire/SPEC.md).

## Change

- `TailKeep::acked` returns the shard's position after the batch (the count of records queued for pushing, `None` when nothing was pushed), and `TailKeep::position` reads it.
- Each subscription in `TailServe` starts confirmed through the shard's position when it subscribed and numbers its pushes from 0. `TailServe` gains `targets(uuid, shard, end)` (numbers a push for each subscriber), `confirm` and `drop_sent` (act only on the subscription the push was sent under), `confirmed`, and `stop` (drops a shard's subscribers and keep but not its claim).
- `oss-node` gains `tail_push::TailPush` over a `TailServe`, a `TailLink` (`send(target, PushRequest) -> PushResponse`) and `ShardFacts`:
  - `acked(index, uuid, shard, records, push)`: overlay off drops the shard's keep and subscribers; `push` false only begins the keep; else records are kept and queued for the shard's subscribers. A shard whose queue passes 20,000 records drops its subscribers and its queued records (a reader with a gap re-fetches when it next opens the shard).
  - `flush()`: drains the queue, gives each subscribed node one `PushRequest` with every queued shard it follows, in order, and sends it. An answer naming a shard unwanted unsubscribes that node from it; a failed send unsubscribes it from every shard in the request; otherwise each shard's push is confirmed through its end position, out of order answers waiting for earlier ones.
- `acked` holds the queue's lock across the keep's position and the enqueue, and `flush` is serialized, so positions, numbers and sends follow the queue's order. A lost push and an unwanted answer both just unsubscribe here; the shell also records, for the waiting piece, whether the node may still answer under its grant (`mayStillServe`, `lapses`), which that piece adds.
- Not changed: scheduling the flush after the linger (the caller's timer), the threads sends run on, waiting for readers to confirm (`awaitSearchNodes`) and the grant lapse it needs, the reader's receiver and overlay, and the transport link.

## Acceptance criteria

- AC-1: acked records are sent to each subscribed node in one request, shards and records in the order acked; a node not subscribed to a shard gets none of its records; no subscribers, no send.
- AC-2: an answer confirms the subscription through the push's end position; a subscription starts confirmed through the position when it subscribed.
- AC-3: answers out of order confirm only through the earliest unanswered push; a push answered twice counts once.
- AC-4: an unwanted shard unsubscribes that node from that shard only; a failed send unsubscribes it from every shard in the request.
- AC-5: a node that fetched again since a push was sent keeps its new subscription whatever became of the old push.
- AC-6: `push` false queues nothing and begins the keep; overlay off drops the keep and subscribers; an empty batch does nothing.
- AC-7: past 20,000 queued records for a shard its subscribers are dropped and its queue discarded.
- AC-9: concurrent acks and flushes deliver every record once and confirm through the last position.
- AC-8: a flush with an empty queue sends nothing, and a drained queue is not sent twice.

## Test plan

`crates/node/tests/tail/tail_push.rs` with a recording fake link and fake facts; `crates/node/tests/tail/tail_serve.rs` for the numbering and confirm rules; one test per criterion.

## S3 request budget

Zero: pushes go node to node.

## Bounds

20,000 queued records for a shard, drained by each flush; the keep's bounds as before.

## Switch

None yet: nothing calls it until wired; the overlay setting arrives with the wiring.

## Risks

A lost push must never leave a subscriber that would miss records later: a failed send unsubscribes it, tested.
