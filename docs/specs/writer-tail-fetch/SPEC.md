# The writer answers a reader's tail fetch

## Serves

Parity plan R7.6, the tail overlay's answering half. The `answerFetch` of the shell's `TailPush` (with `claimedThrough`, `gapAfter`, the subscription table and the grant). Builds on [writer-tail-keep](../writer-tail-keep/SPEC.md) and [tail-wire](../tail-wire/SPEC.md).

## Change

- `TailKeep` gains `snapshot_after(uuid, shard, after)` (the records above `after` and whether they are whole, read under one lock), `begin(uuid, shard, floor)` (begins a keep at a floor if none) and `gap_after(uuid, shard, after)` (a write above `after` was acknowledged and not kept).
- `oss-node` gains `tail_serve::TailServe`, memory only, over a `TailKeep` and a `ShardFacts` (does this node write the shard, is its ownership in doubt, is the overlay on for the shard, the shard's highest sequence number) and a `LeaseClock`:
  - `answer(&FetchRequest) -> FetchResponse` as the shell's `answerFetch`:
    - a shard this node does not write, or any shard while ownership is in doubt, gets no records, no subscription, not complete, and a claim of `i64::MAX`;
    - overlay off for the shard, a `CHECK` ask, or a gap above the commit gets no records and no subscription, with the claim;
    - else the asker is subscribed (a new id, a grant to now plus 600 ms) before the kept records above its commit are read, and `complete` is `TailKeep::complete_after`; a fetch that names the subscription it still holds renews the grant (the later of the two) and gets no records, its id, `complete` true.
  - `subscribers_of(uuid, shard)`, `unsubscribe(uuid, shard, node)` and `released(uuid, shard)`, which also drops the shard's claim.
  - `claim(uuid, shard)` raises the shard's claim to its highest sequence number; the claim is that number the first time it is asked, and never falls.
- Not changed: the transport handler and its registration, the push sender and confirmation, waiting for readers, the overlay setting, and the call from the write path.

## Acceptance criteria

- AC-1: a shard not written here, and any shard while ownership is in doubt, answer the `i64::MAX` claim and nothing else, and subscribe nobody.
- AC-2: overlay off, a `CHECK` ask, and a gap above the asked commit answer the claim with no records and no subscription.
- AC-3: a new fetch is subscribed with ids from 0, answered with the kept records above its commit, and `complete` as `TailKeep::complete_after` says; a second node gets the next id.
- AC-4: a fetch naming its held subscription renews it: same id, `complete` true, no records, a grant that runs to the later time; one naming another id, or `NEW`, is subscribed anew.
- AC-5: the claim is the shard's highest sequence number the first time, is raised by `claim`, never lowered, and dropped by `released`.
- AC-6: `unsubscribe` removes one node; `released` removes all and the keep.
- AC-7: a grant is `now + 600 ms` on the clock, and a renewal never shortens it.

## Test plan

`crates/node/tests/tail_serve.rs` with a fake `ShardFacts` and a fixed clock; one test per criterion.

## S3 request budget

Zero: memory only.

## Bounds

At most one subscription per (shard, subscriber node), and one claim per shard; freed with the shard. A subscription has no expiry here: the push piece must unsubscribe a node it cannot reach, and a sweep of shards no longer written must exist, before this is wired.

## Switch

None yet: nothing calls it until the handler is wired, and the per-index and cluster overlay switch arrives with that wiring.

## Risks

A reader answering under a grant after a new owner took the shard serves stale reads: the ownership-in-doubt answer is the guard, and is tested.
