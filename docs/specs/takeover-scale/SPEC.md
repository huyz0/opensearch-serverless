# A dead node's 1,000 shards are taken as fast as the Java takes them

## Serves

Parity plan R8 exit gate: takeover of 1,000 shards within 10% of the Java's time. ADR 0021 (proactive takeover), invariant I-1.

## Change

An evidence test, run by hand, takes the 1,000 shards a dead node's log names through the proactive pass over the emulator at the Java soak's mean request latency, at the shipped `max_concurrent` and at 16, and compares the time with the Java soak's takeover time. The product change is the two locks under "Product change" below. If the shipped setting misses the bound the finding goes to the owner as a setting question before any default moves.

## Acceptance criteria

- AC-1: a pass over a log naming 1,000 shards with one unpublished write each takes every shard the log names (about 950 of 1,000, three documents each) and makes no LIST.
- AC-2: after the pass no shard lost a document: its highest sequence number on the new node equals the dead node's.
- AC-3: the pass's wall time with 16 takes at once is at most 1.10 times the Java's recorded 1,000-shard takeover (57.0 s, segment_blob with log commits, in the project's soak notes); the figure at the `max_concurrent` this change found (4) is reported beside it, and the default moved in [takeover-concurrency-default](../takeover-concurrency-default/SPEC.md).
- AC-4: opens of different shards of one dead node's log overlap: seven held from seven threads take under three times what one takes. A role-wide open lock and a publisher map locked across each publish made them run one at a time (411 s for 938 shards, at 4 and at 16 alike).

## Test plan

`crates/node/tests/takeover_scale.rs`, ignored, run by hand with `--nocapture`: it fails if a shard is not taken, a document is lost, a LIST is made or the time is over the bound.

## S3 request budget

Unchanged. The requests a shard costs are `takeover.proactive.*`.

## Switch

None.

## Risks

The emulator's latency has no tail; the mean of the Java soak's profile (20 ms, 1 in 50 at 200 ms) is matched with a jittered delay. Numbers are provisional.

## Product change

`WriterRole` opens a shard under a lock of its stripe of 64, not one lock for the role; with a shard cap set every open still takes one more lock first, so the cap is judged against each open before it. `Publishing` keeps one publisher a shard, each behind its own lock, and no longer holds the map's lock across a publish. The node-wide publish gate (`refresh.rs`) is untouched: it serializes the publishes of all shards on a node and is for a later change after the write pipeline (ADR 0050) lands.
