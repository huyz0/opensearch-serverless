# 0045: A shallow snapshot is restored by copying its files into a new index, in the Java's order

**Status:** Proposed

## Context

[ADR 0042](0042-snapshots-taken-shallow-first.md) takes shallow snapshots: the record names, for each shard, the commit's files by term directory, with lengths, and copies no data. A restore turns that record into a live index. The shell does it by creating the target index, copying every captured file of every shard into the new index's own first-term directory (a ranged read for a packed file, a write for each), and landing a manifest by compare-and-swap from absent; a failure removes what it created (`SnapshotOperations.restore`, `SnapshotBlobs.copyShardBlobs`). The source index may be deleted by then: the janitor holds its blobs while a snapshot names them ([snapshot-take](../specs/snapshot-take/SPEC.md)).

## Decision

- Restore follows the shell's order and its refusals: read the snapshot (404, or 409 while it is being taken); resolve every target name first (`indices`, `rename_pattern` and `rename_replacement`; a collision between targets, a system index, or an existing index refuses the whole restore before anything is written); then for each index create the descriptor, copy each shard's files, and land the shard's manifest only if none is there; a failure deletes every index this restore created.
- The copy is a streaming ranged read into a streaming write through a bounded buffer. Requests are one read and one write for each captured file, plus one manifest create a shard and one descriptor create an index. They scale with files, that is with bytes and segments, never with documents, and make no LIST. A file with no recorded length is refused (the Java lists a term to find it); a snapshot Rust takes always records lengths.
- Standard snapshots, `include_global_state`, `partial`, `index_settings`, `ignore_index_settings`, `include_aliases` and `wait_for_completion=false` as a background job are refused with 501, as the take refuses its own. A restore is synchronous.
- Behind the take's two switches plus `serverless.snapshots.restore.enabled`, default off, because a restore moves bytes and a mid-way crash can leave a half-restored index that only a delete clears.

## Consequences

- Easier: a deployment can bring a snapshot back without a Java node, and the stuck-take and delete paths already exist.
- Harder: a restore of a large index costs requests and bandwidth in proportion to its files; a crash mid-restore leaves an index whose shards are partly empty (the Java has the same hazard); the first-term directory of the new index is written by the restore, not by a writer.
- Rules out for now: restoring into a running cluster's existing index, a partial restore of an index's shards, and any restore without copying.
