# Node log store: one node's write log in the object store, by name

## Serves

Parity plan R5.2, part 1: the store half of the log committer (roadmap stage 3, [roadmap](../../roadmap.md)). Invariants: one write log per node, never per shard; fencing by conditional create, never a lock or lease ([principles](../../principles.md#write-path)); no LIST on a write, read or startup path (`s3-cost` skill).

## Change

- `oss-writer` gains `log::NodeLogStore` (`crates/writer/src/log/{mod,store,fence,hedge}.rs`), a port of the shell's `nodelog/NodeLogStore.java` at `429b4cbc`: `append`, `append_hedged` with its read-back, `fence` (first free slot by doubling then halving, aiming ahead of a writer appending back to back and filling the gap from the bottom, sharing another successor's fence), `first_free`, `read_entry` (a 64 KiB probe, a second read only for a longer header), `last_header_below`, `read_slice`, `read_body`, `parse_slice`, `delete_range`.
- Every key is computed by `oss_format::node_log_key`; nothing here lists.
- Differences from the Java, by design: calls block, and a hedged PUT runs on whatever the caller's `Spawn` gives it (the Java takes an `Executor`). `delete_range` sends one DELETE per slot, because the store has no batch delete yet; DELETE is not billed (`S3Op::Delete` is `Free`), so the bill is unchanged, and a batch delete in `oss-blob` would cut only the request count. A read-back compares the slot with the blob chunk by chunk, stopping at the first difference, rather than reading it whole. Every read is bounded by `with_max_read` (default and most: the Java's `int`, 2 GiB), checked before the GET, so a length a blob claims never sizes an allocation past the bound the node sets; the Java checks only the `int`. When one hedged PUT says the slot is taken and the other's thread ends without answering, the outcome is unknown, with no read-back; the Java reads back. Deleting one slot a request is also serial: trimming 1,000 slots is 1,000 round trips, seconds at S3's latency, off the write path; a batch delete (DeleteObjects) in `oss-blob` is the follow-up that cuts it to one.
- Not changed: no committer, buffer window, hedge delay or queue (R5.2 part 2), no fences on shards or terms (R5.3), no replay (R5.4).

## Acceptance criteria

- AC-1: a window of three shards is one PUT; its header is one ranged GET; one shard's slice is one ranged GET and parses to the records its batch held (`one_blob_holds_every_shard_and_each_slice_reads_alone`).
- AC-2: after 37 appends, `fence` from a stale hint lands at slot 37 with one PUT, twelve HEADs, one ranged GET and no LIST; an append to slot 37 is `Taken`; `last_header_below` the fence is slot 36; a second successor's fence from slot 0 shares slot 37 (`a_fence_stops_the_writer_and_every_blob_it_landed_is_below_it`).
- AC-3: a fence racing a writer, paused 1 ms between appends or back to back, stops it with `Taken`, and the writer's last landed slot is the one just below the fence, over 20 rounds each (`a_fence_racing_the_writer_*`); and, placed deterministically by a store whose writer beats the fence's first one, two or three PUTs, the fence fills its gap from the bottom and lands just above the writer's last slot; a writer that always wins ends the fence with `FenceExhausted`.
- AC-4: `delete_range` of 1,200 slots removes exactly those, sends no LIST, and leaves slot 1,200 (`deletes_go_by_name`).
- AC-5: a header longer than the 64 KiB probe takes exactly two ranged GETs and decodes to 3,000 slices; one of 40 KiB takes one; a header, slice or body longer than `with_max_read` is refused before it is fetched, and one exactly at it reads (`a_header_longer_than_the_first_read_takes_a_second`, `a_header_within_the_probe_is_one_read`, `a_read_longer_than_the_bound_is_refused_before_it_is_fetched`).
- AC-6: hedged appends, on a store whose PUTs follow a script: a fast PUT sends no hedge (one PUT); a slow PUT is overtaken by its hedge (`Won`, sooner than the slow PUT); a PUT that landed but answers slowly is `Sent`, not a fence; a PUT that landed and then failed is `Sent` after exactly one read-back GET; a fenced slot fails both PUTs as `Taken` and still holds the fence; a fence that beats both slow PUTs is `Taken`; no delay means no hedge (`crates/writer/tests/node_log_hedge.rs`).

## Test plan

- AC-1 to AC-5: `crates/writer/tests/node_log_store.rs`, ported from the shell's `NodeLogStoreTests`, on the emulator for its meter and on the fs store for the races; `crates/writer/tests/node_log_fence.rs` for the races placed by the store. Each fails if a lookup lists, a fence lands above a slot the writer took, or a long header is read in more than two requests.
- AC-6: `crates/writer/tests/node_log_hedge.rs`, ported from `NodeLogHedgeTests`. Fails if a hedge's own PUT is mistaken for a fence, or a fence for the hedge's own PUT.

## S3 request budget

- Append: 1 PUT, 1 round. Hedged: at most 2 PUTs in 1 round, plus 1 GET only when one PUT found the slot taken and the other did not land (a hedge raced itself or a fence). The committer (part 2) keeps the Java's cap on how often a hedge is sent.
- Fence: HEADs doubling then halving from the lease's landed hint (a handful for a hint seconds old), 1 PUT, and a ranged GET of the slot below; more only while racing a live writer.
- Read a header: 1 ranged GET, 2 when the header passes 64 KiB. A slice: 1 ranged GET. A body: 1 ranged GET.
- Delete a range: 1 DELETE a slot, unbilled. No LIST anywhere.

## Switch

None: a port of the store the Java already runs; there is no other behaviour to compare against. Hedging is off with a zero delay, which is how the committer will switch it.

## Risks

- A fence that lands above a slot the writer took loses acknowledged writes on takeover. AC-2 and AC-3 race it; R5's exit gate takes logs over across languages.
- A hedge read-back that mistakes a fence for its own blob would let a fenced writer acknowledge. The read-back compares every byte; AC-6 covers both directions.
