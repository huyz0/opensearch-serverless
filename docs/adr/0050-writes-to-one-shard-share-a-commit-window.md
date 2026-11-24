# 0050: Writes to one shard share a commit window: the slot lock is released before the wait

**Status:** Proposed

## Context

A write group takes the shard's slot lock, applies its items to the engine, stages their records in the node log's committer and waits for the window holding them to land, all under the slot lock (`WriterRole::group_admitted`). Two write requests to one shard therefore never share a window: each waits its own, and a shard takes one request a window. Measured (provisional, debug build, file store, a 250 ms window, bulks of 20 documents): four clients writing one index take 1,015 ms a bulk and the node does 27 bulks in 6 s; four clients with an index each take 254 ms and do 96. A tenant's top-load target is about 10,000 writes a minute on one index, from many requests, so one request per window per shard is a ceiling the target can reach.

The lock is held across the wait for one reason: order. The engine assigns sequence numbers under the lock, records are staged in the same order, and a failed commit releases the shard so that nothing is acknowledged after a write the log does not hold.

## Decision

1. **Split the commit.** `NodeLogCommitter::stage(shard, term, records)` joins the windows exactly as `commit` does and returns a `Staged` holding the latches of the windows joined; `Staged::wait(&committer)` is the rest of `commit` (drive or linger, then read the results, including `CommitError::Lost` for a window never written). `commit` is `stage` then `wait`, unchanged for every other caller. Everything below concerns windows of this node's one committer, which are written one at a time in order, so when a group's window has finished, every earlier window has.
2. **A slot keeps a pipeline.** Under the slot lock a group applies its items and stages their records, so the log order of one shard's records is its sequence order, as now. It takes a **ticket** (a counter on the slot) and an entry in `in_flight` (its `Staged`, its ticket), then releases the slot lock and waits for its windows. A group with no record to stage (every item refused) takes no ticket. The slot has a sticky `failed` flag, set only under the slot lock and never cleared; a slot with `failed` set admits nothing new (the check `released()` makes today).
3. **After its wait a group finishes in ticket order, and the finish is atomic with the slot.** (a) Wait for the previous ticket to finish, with the slot lock **not** held (a condvar of its own on the pipeline). (b) Take the slot lock. (c) If its own windows failed, or any earlier entry's window carries a failure for this shard (those windows have finished, rule 1), or `failed` is already set, set `failed` and refuse the group, even if its own blob landed (its outcome is unknown to the client, as for a write that timed out). (d) Otherwise run the acknowledgement effects (`publishing.written`, the tail overlay's `acked`, the refresh bookkeeping) **under the slot lock**: they run only while the slot is not failed and so cannot repopulate state a release has cleared, and they run in ticket order, which is sequence order. (e) Advance the ticket, remove the entry, and wake the next. A drop guard does (e) with `failed` set if the group panics or returns early between stage and finish, so a dead thread never stalls the ticket.
4. **A failed slot is released by the last group out, never earlier.** `failed` stops new groups at once, but the slot stays in the node's `shards` map, so a new request is refused rather than opening a slot, until `in_flight` is empty; the group that empties it removes the slot (`release`: the tail overlay and lazy-publish state are cleared then, after every effect of this slot has run or been skipped). The shard reopens from the log only after every window staged for the old slot has landed or failed, so the new slot never continues from a log that a late window of the old slot is about to extend, and no sequence number is issued twice.
5. **Publish, hibernate, evict, give-back, retire and release quiesce the shard first.** The caller takes the slot lock, sets `quiescing` (new groups wait, without the slot lock, on the pipeline's condvar until it clears), and, if `in_flight` is not empty, drops the lock, waits for every entry to finish, and takes the lock again. It then re-checks `failed` and `released()`: if either is set it aborts without taking a snapshot, so a publish never covers a write whose record did not land; otherwise it does its work and clears `quiescing`. The wait is outside the slot lock, so reads are not blocked by it; new writes wait for it (at most about one window, and publishing is infrequent), as they wait for the slot lock today.
6. **Admission stays where it is.** The write limiter's permit is held for the whole group, wait included; the cap on writes waiting on the log (`serverless.write.backpressure.*`) still bounds the groups waiting.

No new request, timer or format: the records, windows and blobs are as before; more groups ride one window, so a node makes fewer PUTs for the same writes.

## What readers see

A realtime get, a refresh, an update and a delete-by-query read the engine under the slot lock, and the engine now holds writes applied but not yet in the log. A read can therefore see a write that a failed window then refuses, and a reopen no longer has. OpenSearch's engine behaves the same (a realtime get sees an operation before its translog sync). It is accepted here: reads of unacknowledged writes are not promised, a client whose write was refused is told so, and an update that was based on it is refused by rule 3 (it staged after the failure, or its own earlier-entry check finds it). A search sees only a refreshed or published commit, as before.

## Why this does not break the invariants

- *Acknowledged means in the log.* A group returns success only after its own windows landed with no failure for the shard, no earlier window of the slot failed and the slot is not failed (rule 3c), checked under the slot lock.
- *No acknowledged write after a missing one.* A write after a lost one is refused whichever thread wakes first: the failure is found either in the sticky flag or by reading the earlier windows' results, both under the slot lock. A blob after a gap can still land (the epoch may survive an admission refusal of the earlier window, which does not end it); after the slot is released the shard replays the later records without the earlier one. Those writes were refused, so replay holds only writes no client was told succeeded: the same state a write that timed out after its blob landed leaves. Replay applies full-document and delete records with their own sequence numbers and versions, so a record whose predecessor is missing is a document put or a delete of a document that may be unseen, both safe; the replayed state is asserted in the tests.
- *No sequence number is issued twice* (rule 4): a reopen cannot read the log until every staged window of the old slot is final.
- *One write log per node, one blob per window, fencing by conditional create* are untouched. A takeover by another node is refused at write time by the admission check of every in-flight group, as when the lock was held. Hedged PUTs resolve to one outcome a window, so they add no failure mode.
- *Lock order*: slot lock, then the committer's lock, as now; the committer never takes the slot lock; the ticket and quiesce waits hold no lock but the pipeline's own small mutex, and nothing waits for the slot lock while holding it. A thread that waits for the previous ticket does not hold the slot lock, which the previous ticket's owner needs.

## Consequences

- A hot shard takes as many requests as one window carries, bounded by the window's byte and record limits and the limiter.
- A failed window now fails every in-flight group of the slot, not only the one that held the lock when it failed: the blast radius is the groups in flight.
- The slot lock is held for the engine apply, staging, and finish only, never for a window; reads wait for that, not for a window.
- The ordering the lock hid is now explicit: staging order (rule 2), finish order (rule 3), quiesce (rule 5).

## Tests and the crash plan

A test store gates and fails PUTs on demand (hold a window's PUT, fail it, or let it land and then answer an error: the landed-but-unknown case, which the committer counts as unpublished). The wake order of threads is forced by holding and releasing windows, not left to the scheduler. Each is seen failing against the rule it protects:

1. Two groups on one shard share a window: one blob, one window of latency, not two.
2. A refused PUT with three groups in flight on one shard: all three refused, none acknowledged; the shard reopens only after the three are final; the replayed state holds none of their writes.
3. A's window fails and B's lands after it, with A's thread forced to run first and then last: B is refused either way (the sticky flag, and the earlier-window read); after the reopen B's record may replay and A's does not, the replayed state is asserted, no client was told either succeeded.
4. A fails, C is staged behind it, a request for the shard arrives before C is final: it is refused, and once C lands the reopened slot issues sequence numbers above C's.
5. The effects of a landed group run after a later group's failure: they are skipped, and the tail overlay and lazy-publish state hold nothing for the released slot.
6. A publish while groups are staged: it waits for them; with one failed, the publish aborts and no segment holds a record the log does not; the same for hibernate and evict.
7. Effects in ticket order: with the `acked` hook recording, many concurrent groups on one shard call it in sequence-number order.
8. A thread that panics between stage and finish: the drop guard fails the slot and advances the ticket; later groups are refused, not hung.
9. A write racing a hibernate or retire: no write acknowledged after the shard is asleep, none lost.
10. Crash (process killed) at four points with groups in flight: before the PUT, between the PUT landing and the acknowledgement, during a quiesce, and after a publish that covered only landed records. After restart every acknowledged write is present; an unacknowledged one may or may not be.

## Switch

None: no downside to more than one writer per shard in flight; the single-client case is unchanged (one group, no earlier entries). If implementation shows a case that cannot be made safe, the fallback is a setting that keeps the lock held across the wait.
