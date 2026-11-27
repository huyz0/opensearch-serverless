# Shard engine: index and delete as InternalEngine plans them

## Serves

Parity plan R6.2, second part (roadmap stage 3, [roadmap](../../roadmap.md)): a shard of the Rust writer applies index, create and delete operations with the Java's sequence numbers, versions, version conflicts and soft deletes, so the documents API answers as the Java's does and a segment holds what a Java writer's would. The node log is the translog (no local translog, as `DiscardingTranslogManager`). Builds on [indexing](../indexing/SPEC.md); [ADR 0017](../../adr/0017-mappings-and-documents-as-the-java-reads-them.md).

## Change

- `oss-writer` gains `index` (`crates/writer/src/index/`), a port of the primary's path through `InternalEngine` (server, OpenSearch 3.10 at `429b4cbc`), as `ServerlessWriterEngine` runs it:
  - `IndexingStrategyPlanner.planOperationAsPrimary` and `DeletionStrategyPlanner.planOperationAsPrimary`: `op_type` `index` and `create`, `if_seq_no` with `if_primary_term`, `version` with `version_type` `internal`, `external` and `external_gte`; `VersionConflictEngineException`'s messages.
  - `VersionType.isVersionConflictForWrites` and `updateVersion`.
  - sequence numbers from the shard's local checkpoint tracker: one a successful operation, the max and processed checkpoints.
  - the current version of an id: the operations since the last refresh (`LiveVersionMap`), else the newest live document with that `_id` in the shard's segments (`VersionsAndSeqNoResolver`), with deletes kept as tombstones.
  - an update as `softUpdateDocument` (the old document gets `__soft_deletes` = 1); a delete as the tombstone `createDeleteTombstoneDoc` builds (`_id`, `_seq_no`, `_primary_term`, `_version` and `__soft_deletes`).
  - each operation's result: `created`, `updated`, `deleted` or `not_found`, `_version`, `_seq_no`, `_primary_term`; or the failure.
  - the request checks `IndexRequest.validate` and `DeleteRequest.validate` make of versions (`validateSeqNoBasedCASParams`, `create`'s rules), with `ActionRequestValidationException`'s numbered message.
  - a refresh commits, with `max_seq_no` and `local_checkpoint` in the commit's user data, and opens a reader with soft deletes applied; a shard opened over a commit continues its sequence numbers from it (`loadSeqNoInfoFromLuceneCommit`).
  - the version map keeps deletes past a refresh until `index.gc_deletes` (60 s) has passed, and a lookup ignores an older one (`resolveDocVersion`); its heap counts toward the shard's indexing buffer with the writer's, which a caller refreshes the shard on, as `IndexingMemoryController` does (the scheduling is R6.4's).
  - a document's dynamic mapping update holds even when its write conflicts, as the Java publishes it before the engine plans the write.
- Recording: `fixtures/segments/record.py` replays a fixture document with `"delete": true` as a `_bulk` delete; `fixtures/segments/updates.json` updates and deletes, a delete of a missing id among them.
- Not changed: a document the engine refuses after planning (a field whose schema an update changed) takes no sequence number here, where the Java's takes one and writes a no-op, and the Java's in-segment message for it (`Inconsistency of field data structures ...`); the in-flight document limit; auto-generated ids and the append-only optimisation (with bulk, R6.3), `_update` and scripts, routing, refresh and segment publish (R6.4), replicas and recovery, the in-flight document limit, `index.mapping` settings.

## Acceptance criteria

- AC-1: every index, create and delete step of `fixtures/compat/documents/` replays through the shard: the status, `result`, `_version`, `_seq_no` and `_primary_term` of each, and each refusal's type and message, equal the recording.
- AC-2: for each fixture of `fixtures/segments/` that updates and deletes, the segment a shard commits reads back with the recorded field infos and document count, and each document's stored fields and doc values, `__soft_deletes` and `_tombstone` among them, equal the recorded ones.
- AC-3: an id's version is found whether its last operation is still in the version map or only in a segment.
- AC-4: a refused operation takes no sequence number and changes nothing.

## Test plan

`crates/writer/tests/it/document_fixtures.rs` replays `fixtures/compat/documents/*.json` through `index::Shard` (AC-1, AC-4); `crates/writer/tests/it/segment_fixtures.rs` replays the fixtures that update and delete through a shard and reads the committed segment back (AC-2); `document_fixtures.rs` replays each fixture a second time with a refresh after every step, and `crates/writer/tests/it/shard.rs` refreshes between operations (AC-3) and checks refusals (AC-4), `gc_deletes` and the request checks.

## S3 request budget

Unchanged: the shard indexes in memory; publishing is R6.4's.

## Switch

None: the operations are the Java's.

## Risks

- A version or sequence number that differs from the Java's breaks optimistic concurrency for clients that move between writers: AC-1 compares every number of every step.
- Looking up a version in segments costs a term lookup per operation on an id not in the version map; the version map holds every id since the last refresh, as the Java's does.
