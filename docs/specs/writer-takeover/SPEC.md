# Writer takeover: replay a predecessor's log and adopt its shard

## Serves

Parity plan R8.1 (shard head and takeover, [roadmap](../../roadmap.md)) and the fencing rule: a node that won a shard's head brings the shard to a published commit under its term, and a replaced writer can land nothing after. Builds on [shard head](../shard-head/SPEC.md) (the acquire), [node log takeover](../node-log-replay/SPEC.md) (the fenced read) and [segment publish](../segment-publish/SPEC.md) (the register swap). Ported from `ShardReconciler.ensureOpen` and its node-log replay, `NodeLogHooks.replayedAndPublished`, and `ShardReconciler.takePublishedThroughAtOpen` (huyz0/OpenSearch `feature/serverlessshell`).

## Change

- `index::Shard::replay` applies a record of a predecessor's log as `InternalEngine` applies a recovered operation: the record's own sequence number, term and version, no conflict check, and a record at or below the sequence number its id already holds is stale and changes nothing. A record with no sequence identity takes the next sequence number and version under the shard's term.
- `oss-writer` gains `takeover` (`crates/writer/src/takeover/`):
  - `Predecessors`: the logs of the nodes a head names, one `NodeLogTakeover` per node, made once by the caller's factory (which reads the node's lease) and shared by every shard of a batch of takes.
  - `adopt`: given the head this node won (term, owner, named writers), the shard's engine and the predecessors, it streams each named writer's unpublished records, oldest first, into the engine, skipping those the opened commit already holds; publishes once under the head's term if any were applied, by the publisher's compare-and-swap of the register; then clears the replayed writers from the head by compare-and-swap. Nothing is published when nothing was applied (a shard taken and not written to costs no publish).
  - `HeadWriters`: the head's writer list cleared by CAS, only while the head is still this node's at this term.
  - `ShardEngine`: the engine over `Shard`, `ShardPublisher` and its directory.
- Not changed, by design and listed: acquiring the head (`ShardHeadStore::acquire`), the assignment claim, wiring into a node's writer role and the node's shared lease read (R7 owns node assembly), the janitor's collection of a replayed log (R8.4), filling the gaps between replayed sequence numbers with no-ops, a bound on how many records or bytes one replay holds before it refreshes (a follow-up; the log's own window and the indexing memory bound what a predecessor can have left), reading only the log past what the commit reaches (the manifest records no log position yet, so every blob holding the shard's records is read and the sequence filter runs after), idempotence of a record with no sequence identity (the node log writes none), staged commits and node packs.

## Acceptance criteria

- AC-1: a replayed index record is a live document with the record's version, sequence number and term, and a replayed delete leaves a tombstone; the shard's next write takes the sequence number after the highest replayed.
- AC-2: a record at or below the sequence number its id holds changes nothing, and a record with no sequence identity takes the next sequence number.
- AC-3: `adopt` applies a shard's records from every named writer, oldest first, and skips those at or below the commit's `max_seq_no`; records of another shard are not read into it.
- AC-4: when records were applied, `adopt` publishes once: one blob PUT and one register swap, the register's manifest at the head's term; the replayed documents are in the published commit.
- AC-5: when nothing was applied (no predecessor, nothing it held unpublished, all of it in the commit, or only stale records), `adopt` makes no PUT and no publish.
- AC-6: a batch of takes from one predecessor makes the factory call (the lease read) once per predecessor, however many shards are taken.
- AC-7: after a publish the head names no replayed writer; a head that moved to another term or owner is not written and the answer says so.
- AC-9: a writer whose log could not be read (no lease) is still named by the head after the take; only the writers read are cleared.
- AC-8: a publish refused by a newer term in the register is `Err`, and the head still names the writers.

## Test plan

AC-1, AC-2: `crates/writer/tests/shard_replay.rs`, each seen failing first against a `replay` that does nothing. AC-3 to AC-8: `crates/writer/tests/takeover.rs` over the emulator with a fenced predecessor log built by the committer, a head in a register, the engine over a real `Shard`, and the emulator's meter.

## S3 request budget

Per shard taken with records to replay: the log's reads (the fence and headers shared by the batch, then each blob holding the shard's records once), 1 blob PUT and 1 register swap for the publish (the register is read once if the publisher knows none), 1 head GET and 1 head CAS to settle. Per shard taken with nothing to replay: no PUT of a commit, and the head settle only when it names writers. 0 LIST. Per batch: one lease read per predecessor, not per shard. Nothing scales with documents or elapsed time.

## Switch

None: the Java replays on every takeover, and a shard taken with unpublished records in a predecessor's log cannot serve correctly without it.

## Risks

- Replaying what the commit already holds twice: stale records change nothing (AC-2), and records at or below the commit's `max_seq_no` are skipped before they reach the engine (AC-3).
- A head cleared before the register holds the replay: settle runs only after the publish returns (AC-4, AC-8).
- A takeover's publish racing a second successor: the register's term fence refuses it (AC-8).
