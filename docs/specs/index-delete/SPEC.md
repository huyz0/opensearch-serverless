# Index delete over REST, with real shard heads and point-in-time pins

## Serves

R7 node assembly; [ADR 0012](../../adr/0012-index-deletes-and-reclaim.md), whose consequence "no Rust delete until heads and pins are ported" this retires. Java source: `IndexAdminHandler` (delete route), `ShardHeadStore.readAllOf/deleteIfUnchanged/releaseAllOf/deleteAllOf`.

## Change

- `oss-cluster`: `HeadRegisters` reads, deletes at a generation, releases and removes the heads of one index incarnation by their derived keys (one request a shard, never a LIST).
- `oss-node`: `ShardHeads` over `HeadRegisters`; `ShardPins` over the live points in time (a view that cannot be read, or a capture of the index, pins every shard; a view of the uuid pins the shards it names); a `SnapshotPins` hook the assembly fills, with no source today because the Rust node takes no snapshots (snapshot management is out of scope, [compatibility](../../compatibility.md)); `DELETE /{index}` over `IndexLifecycle::delete_index` (purge inline when no shard had a writer, else at the intent's reclaim); a reclaim drain timer that takes due intents on this node.
- Retires the unwired-deletes check and its self-test and gate row.
- Not changed: wildcard or comma-separated names (refused 400), aliases as delete targets, batched blob deletes, snapshot pins (an extension point only).

## Acceptance criteria

- AC-1: `HeadRegisters::read_all_of` answers each head naming the uuid (or none) with its generation and whether it names an owner, one GET a shard.
- AC-2: `delete_if_unchanged` removes a head only at the generation read; a head moved since stays.
- AC-3: `delete_all_of` removes only heads naming the uuid and answers how many; another incarnation's head stays.
- AC-4: `release_all_of` clears the owner of a matching head and keeps its term.
- AC-5: a point in time of the index and uuid pins the shards it names; an unreadable view or a capture pins every shard, and a failed listing refuses the purge; a view of another index or uuid pins none.
- AC-6: `DELETE /{index}` of an index answers 200 `{"acknowledged":true}`, the descriptor is gone, its heads are gone and an unowned shard's blobs are purged.
- AC-7: a shard a live point in time names keeps its blobs after the delete.
- AC-8: a delete of an index with an owned head files its intent with `had_writer` true, purges nothing inline, and the drain, once the intent is due, purges it and removes the intent.
- AC-9: a missing index answers 404 `index_not_found_exception`; a wildcard or comma list answers 400.
- AC-10: no crate is barred from naming the lifecycle: the unwired-deletes check is gone.

## Test plan

AC-1..4: `crates/cluster/tests/head_registers.rs` on the emulator. AC-5: unit tests in the node pins module. AC-6..9: `crates/node/tests/index_delete.rs` through a real server on the emulator. Each seen failing against a stub that answers no heads and no pins first.

## S3 request budget

Per delete, shards n: n head GETs, 1 intent PUT, 1 descriptor conditional delete, n head deletes, and per purged shard 1 LIST plus one DELETE a blob; a PIT listing (1 LIST per 1,000 views, one GET each). Requests scale with shards and bytes, never with time. The drain: 1 LIST per tick with nothing due.

## Switch

None: a delete is an operator's action and only ever removes. The purge-inline choice is ADR 0012's open item: inline for a shard with no writer (the Java's behaviour); a shard with a writer waits for the reclaim.

## Risks

- A Java shallow snapshot's blobs are purged: the Rust node cannot read them. Mitigation: `SnapshotPins` hook; recorded in ADR 0012's addendum.
- A tombstone-mode store reads no heads before the delete (the Java's), so a live writer's shard is purged at once.
- Known gaps: every node claims every reclaim bucket (one LIST a tick, as the Java); a failed drain is not counted. The purge cost and pinned shards are in [index-delete-purge](../index-delete-purge/SPEC.md).
