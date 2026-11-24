# 0050: Writes to one shard share a commit window: the slot lock is released before the wait

**Status:** Proposed

## Context

A write group takes the shard's slot lock, applies its items to the engine, stages their records in the node log's committer and waits for the window holding them to land, all under the slot lock (`WriterRole::group_admitted`). Two write requests to one shard therefore never share a window: each waits its own, and a shard takes one request a window. Measured (provisional, debug build, file store, a 250 ms window, bulks of 20 documents): four clients writing one index take 1,015 ms a bulk and the node does 27 bulks in 6 s; four clients with an index each take 254 ms and do 96. A tenant's top-load target is about 10,000 writes a minute on one index, from many requests, so one request per window per shard is a ceiling the target can reach.

The lock is held across the wait for one reason: order. The engine assigns sequence numbers under the lock, records are staged in the same order, and a failed commit releases the shard so that nothing is acknowledged after a write the log does not hold.

## Decision

1. **Split the commit.** `NodeLogCommitter::stage(shard, term, records)` joins the windows exactly as `commit` does and returns a `Staged` holding the latches of the windows joined; `Staged::wait(&committer)` is the rest of `commit` (drive or linger, then read the results). `commit` is `stage` then `wait`, unchanged for every other caller.
2. **Stage under the slot lock, wait without it.** A group applies its items and stages their records under the slot lock, so the log order of one shard's records is its sequence order, as now. It then releases the lock and waits. The slot keeps, under its lock, the `Staged` handles of its groups in stage order (`in_flight`).
3. **A group fails if any earlier group of the slot failed.** After its own wait, a group takes the slot lock and examines every earlier entry of `in_flight`. Windows are written one at a time in order, so each earlier window of the shard has finished when this group's has. If any earlier window carries a failure for this shard, this group is refused, even when its own blob landed (it may be in the log; its outcome is unknown to the client, as for a write that timed out). The group that sees a failure of its own window releases the slot as today. So no write is acknowledged after one the log does not hold, whatever the order in which the threads wake. Entries are removed when their group has finished, so the list is bounded by the groups in flight.
4. **Acknowledgement side effects run in stage order.** What a landed group does after the wait (`publishing.written`, the tail overlay's `acked`, the refresh bookkeeping) runs in stage order per slot: a group takes a ticket at stage and runs its effects when the previous ticket has finished (a condvar on the slot's in-flight list). The wait is only for threads that were woken by the same or an earlier window, so it is bounded by CPU, not by a store call.
5. **Anything that reads or ends the shard's state drains first.** Publishing, hibernation, give-back, retire, release and a refresh that snapshots the engine take the slot lock and first wait for every `in_flight` entry to land or fail (the same wait, no new window is opened by the wait). A publish therefore never covers a write whose record has not landed, and trim and `unpublished` bookkeeping in the log see records before the publish that covers them.
6. **A released slot admits nothing.** `released()` is checked under the slot lock before applying, as now; a group that finds it refuses without staging. A slot opened again is a new `ShardSlot` with an empty `in_flight`; the old slot's groups still hold the old one.
7. **Admission stays where it is.** The write limiter's permit is held for the whole group, wait included, as now; the cap on writes waiting on the log (`serverless.write.backpressure.*`) still bounds the groups waiting.

No new request, timer or format: the records, windows and blobs are as before; more groups ride one window, so a node makes fewer PUTs for the same writes.

## Why this does not break the invariants

- *Acknowledged means in the log.* A group returns success only after its own windows landed with no failure for the shard and no earlier window of the slot failed.
- *No gap is acknowledged.* A write after a lost one is refused (rule 3), so the log's shard order has no acknowledged gap. A gap can exist in the log: a later group's blob may land after an earlier window failed. After the slot is released, the shard reopens from the log and replays the later records without the earlier ones; those writes were refused (unknown outcome), so the replay holds only writes no client was told succeeded. This is the same state a write that timed out after its blob landed already leaves.
- *One write log per node, one blob per window, fencing by conditional create* are untouched; the committer, the epoch, and the admission check are not changed.

## Consequences

- A hot shard takes as many requests as the window carries, bounded by the window's byte and record limits and the limiter.
- A failed window now refuses every in-flight group of the slot, not only the one that held the lock when it failed: the blast radius is the groups in flight, which is what the failure meant before, but made explicit.
- The slot lock is held for the engine apply and the staging only, so reads and the hibernate pass that take it wait for that, not for a window.
- More concurrency inside one shard exposes an ordering the lock hid: the effects in rule 4 are ordered by ticket, and the drain in rule 5 is the one place a publish waits for the log.

## Tests and the crash plan

Each is seen failing against the old behaviour or the rule it protects:

1. Two groups on one shard share a window: both landed in one blob, latency one window, not two (the throughput measurement, as an assertion on blobs landed).
2. A window that fails (a store that refuses the PUT) with three groups in flight on one shard: all three are refused, none acknowledged, the shard reopens, and a read after the reopen finds none of the three groups' writes that were refused by a failed window and every earlier acknowledged one.
3. The failing case of rule 3: group A's window fails, group B's window (written after) lands; B is refused although its blob landed; after the reopen B's record may replay and A's does not, and no client was told either succeeded.
4. A crash (process killed) with groups in flight on one shard: restart replays the log; every acknowledged write is present; any write not acknowledged may or may not be.
5. A publish while groups are staged: it waits for them to land, and the segment it writes never holds a record the log does not.
6. Effects in stage order: with the `acked` hook recording, many concurrent groups on one shard call it in sequence-number order.
7. A shard's groups racing a hibernate or retire: no write acknowledged after the shard is asleep and none lost.

## Switch

None: no downside to one writer per shard; the single client case is unchanged (one group, no earlier entries). If review finds a case that cannot be made safe, the fallback is a setting that keeps the lock held across the wait.
