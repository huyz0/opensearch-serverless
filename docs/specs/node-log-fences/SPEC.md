# Node log write fences: a successor fences a predecessor's log

## Serves

Parity plan R5.3 (roadmap stage 3, [roadmap](../../roadmap.md)). Invariant I-7: a fenced-out writer cannot overwrite its successor's log slot ([invariants](../../invariants.md)). Fencing by conditional create, never a lock or lease ([principles](../../principles.md#write-path)).

## Change

- `oss-writer` gains `log::NodeLogTakeover` (`crates/writer/src/log/takeover.rs`), the fencing half of the shell's `nodelog/NodeLogTakeover.java` at `429b4cbc` (`fenceAll`, `ends`, `named`, the constructors and `LeaseReader`):
  - **Fence first, once.** The first call fences every epoch the predecessor's lease names, from its landed hint, so the predecessor can land nothing after what is read, then reads each epoch's last header below its fence and at or above its trim point. Later calls, and every shard taken from that predecessor, share the result; a second successor shares the fences through `NodeLogStore::fence`.
  - **A stale lease.** With a `LeaseReader`, the lease is read again after the fences; an epoch whose trim point is now above its fence was deleted past it, and is fenced again from that lease's landed hint, at most `MAX_REFENCES` (8) times. The higher trim point is also the floor of the header search.
  - **Unfenced.** `unfenced` reads up to each epoch's first free slot and writes nothing, for a writer that has let go of the shards asked about.
  - A failure leaves nothing cached; the next call starts again, sharing the fences already placed.
- Not changed: reading a shard's records and replaying them (R5.4); wiring into shard open and the writer's own restart (`NodeLogWriter.start`).

## Acceptance criteria

- AC-1 (I-7): after a takeover fences a writer's epoch, the writer's next commit fails `Fenced`, and the ends name the fence's slot, one past the writer's last blob.
- AC-2: three shards in four blobs each: one takeover's `fence_all`, `ends` and `named` together write exactly 1 blob (the fence), list nothing, and `named` holds the three shards; a second successor's takeover of the same lease writes nothing.
- AC-3: a takeover from a lease whose landed hint points below slots the writer deleted after it, rereading the current lease, places its fence above the hole: the writer's next commit fails and the ends name a slot above the new trim point. Without the reread, the fence lands in the hole and the writer's next commit lands.
- AC-4: an epoch with nothing above its trim point names no shard, and reads no blob below its trim point: one write (the fence) and one GET, the fence's look at the slot below it; no header is read.
- AC-5: an unfenced reader writes nothing, the writer's next commit lands, and its ends are each epoch's first free slot.
- AC-6: a failed fence or a failed lease reread fails the call and caches nothing; the next call succeeds.

## Test plan

`crates/writer/tests/it/node_log_takeover.rs` on the emulator, its meter counting `Put`, `Get` and `List`; the writer is a `NodeLogCommitter` with a recording lease. Ported code with its tests written alongside: `scripts/mutants.sh` on the diff stands in for watching each test fail.

## S3 request budget

- One takeover of a predecessor: per epoch its lease names, one fence (a create; GETs from the landed hint to the first free slot, a handful when the hint is fresh) and the GETs reading back from the fence to the last header, stopping at the trim point. Shared by every shard taken from it. No LIST.
- With a reread: the lease read, and a second fence only for an epoch deleted past its first.

## Switch

None: this ports behaviour the Java runs unswitched.

## Risks

- A fence in a hole below the writer's tail, which the writer never reaches: the reread (AC-3).
- A takeover that fences twice per shard: the result is shared (AC-2).
