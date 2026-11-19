# Janitor: collect a dead node's write log

## Serves

Parity plan R8.4, second piece, invariants I-1 and the fencing rule. Ported from `metadata/ShardSettlement.collectNodeLog` and `publishedPast` at `3920dc36`. Follows [janitor-gc](../janitor-gc/SPEC.md); decision: [ADR 0020](../../adr/0020-janitor-crate.md).

## Change

- `oss_janitor::node_log::Collector::collect(node_id)`: deletes the write log of a node that is not live, once every shard its log names has published past what the log holds.
  - **Nothing** for a node with no lease, a lease that names no epochs, or one that is live (neither revoked nor expired by the collector's clock).
  - **Fence, then judge.** Every epoch the lease names is fenced first (the `NodeLogs` seam), so a node that is not as dead as it looks adds nothing after what is read. Each shard the last headers name must be published past: a shard with no index name is not (the log cannot be judged); an index that is deleted or recreated under another uuid needs nothing; a head still owned by this node is not (settled or taken first, and judged then); a shard the log holds nothing of (sequence number unassigned) needs nothing; otherwise the published commit's local checkpoint must reach the highest sequence number the log holds of it, and a shard with no commit, or a commit recording no checkpoint, does not. One shard not published past gives **StillNeeded** and nothing is changed.
  - **Forget, then delete.** The lease stops naming the epochs by one swap that a node coming back wins (`Membership::forget_node_log`); a lost swap is StillNeeded. Only then is each epoch deleted from its recorded trim point to the fence's end, plus one. A failed delete costs storage, not records, and is an error the next pass repeats from the lease's (now empty) view: the bucket's lifecycle rule on the log prefix removes the rest, as the Java documents.
  - **Dry run** (`serverless.janitor.dry_run`, the same switch): judges exactly the same and answers **WouldCollect** without swapping the lease or deleting anything. It does fence the epochs, because the judgment needs a fenced read: the same fence a takeover puts on a node whose lease has expired, so a node that was only slow loses those epochs and opens new ones. A dry run is not read-only for a dead node's log; it changes nothing the lease and the log's data keep.
- The log itself is read through two seams the node implements over the writer's `NodeLogTakeover` and `NodeLogStore`, because the janitor never depends on the writer: `NodeLogs` (fence a node's epochs and answer what they name, the highest sequence number of a shard, each epoch's end; delete a range of an epoch's slots) and `Commits` (a shard's published commit: absent, or its local checkpoint if recorded). The crate ships no implementation of either.
- Not in this change: the scheduling of which dead nodes to collect (the pass), inherited epochs of a live node's own earlier process, `settleAbandoned` and `settleReleased` (takeover's, R8.2), the node wiring.

## Acceptance criteria

- AC-1: a live node, a node with no lease, and a lease naming no epochs answer Nothing and send no request but the lease read.
- AC-2: a dead node whose shards are all published past is collected: its lease names no epochs afterwards and each epoch is deleted from its trim point through its end, in that order (lease first).
- AC-3: a shard whose commit's checkpoint is below the log's highest sequence number keeps the log: StillNeeded, the lease unchanged, nothing deleted.
- AC-4: a shard with no commit, a commit with no recorded checkpoint, a head still owned by the dead node, and a shard key with no index name each keep the log.
- AC-5: an index deleted, or recreated under another uuid, needs nothing of its shards.
- AC-6: a shard the log names but holds nothing of needs nothing.
- AC-7: the epochs are fenced before any shard is judged.
- AC-8: a lease that changed between the judgment and the forget (the node came back, or another epoch) is StillNeeded and nothing is deleted.
- AC-9: with `dry_run` on the same judgments are made, the answer is WouldCollect for what would be collected, and the lease is not swapped and no slot is deleted (the fence is the only thing done).
- AC-11: an epoch the fence names no end for is deleted through its landed hint; one shard not published past keeps the log whatever its place in the order; a head owned by another node is judged by the commit.
- AC-10: the requests are exactly: the lease read, the lease read and swap of the forget, one head read and one descriptor read a shard judged, plus the seams' own; no LIST.

## Test plan

`crates/janitor/tests/node_log.rs` over the emulator with the real `Membership`, `ShardHeadStore` and `DescriptorStore` and fakes for the two seams that record their calls. A test for each of the judgments above, and the order of the calls.

## S3 request budget

Per dead node collected: 1 lease GET, per shard named 1 head GET and 1 descriptor GET (plus the commit read the seam makes), 1 lease GET and 1 swap, and the seam's deletes (one DELETE a slot today, as `NodeLogStore::delete_range`). No LIST. Triggered by the pass that finds a dead node, never by a timer over nodes; a node collected is not visited again because its lease names no epochs.

## Switch

The one switch, `serverless.janitor.dry_run`, default on.

## Risks

- Deleting a log a successor still needs to replay: the fence first, the commit's checkpoint against the log's headers, the head's owner, and the swap that a returning node wins.
- A node that stays StillNeeded (a shard key with no index name, a head nobody takes) is fenced and judged again each pass, about two reads a shard: the pass must back off on it, and a bucket lifecycle rule on the log prefix is the backstop for a log that is never collectable, as the Java documents. The descriptor is read once a shard, not once an index.
- A seam that answers wrongly: both are traits with the failure mode stated; the real ones are wired and tested at the node.
