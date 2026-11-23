# Node writer role: apply a write, stage it in the node log, then acknowledge

## Serves

Parity plan R7.6 (node wiring and roles), first piece; the write path of `NodeWritePath`. Invariants: one write log per node, never per shard; a write is durable in the log before it is acknowledged ([principles](../../principles.md#write-path)).

## Change

- `oss-node` gains `writer::WriterRole` behind the `writer` feature (`crates/node/src/writer/`). It holds the shards this node writes, each in a `ShardSlot` that owns its directory and its `oss_writer::index::Shard`, and the node's one `NodeLogCommitter`.
- A write is applied to the shard's engine first (the engine assigns the sequence number), then staged: its `WalRecord` goes through the committer, which records the epoch in the node lease before its first blob and asks admission before a shard's first records enter an epoch. The write is acknowledged only once the blob landed.
- **Release on a failed stage.** When staging fails after the engine applied the write, the shard's slot is dropped: the engine holds an operation the log does not, so the shard must reopen from the log. The write is refused (`appendOrRelease`).
- A bulk group is applied item by item and staged once: `Target::apply_group` (new, defaults to one `apply` per item) lets the role put a whole group in one `commit`. A failed stage fails every item of the group that was written.
- Forwarding: a shard this node does not hold is sent to its owner through the `Routes` trait; `WriterRole` is a `bulk::Target`.
- Not changed: the lease register write (`LeaseWrite` over `Membership::renew`, with node assembly), publishing commits and pushing them (`push_sender` is not called yet), the write.first budget, `serverless.lease.early_revoke`'s call site, the tail overlay, the newest-commit answer, shard heads as the admission source (admission is injected).

## Acceptance criteria

- AC-1: a write to a held shard returns only after exactly one node-log blob landed, and that blob holds a record with the engine's id, seq_no, term and version.
- AC-2: the first write of a process records one epoch in the lease before the first blob; later writes record none.
- AC-3: a bulk group of 50 items in one shard is one blob and one record each, and a group over two shards in one window is one blob.
- AC-4: a write the engine refuses (version conflict) logs nothing and takes no sequence number.
- AC-5: a stage that fails (admission refused) refuses the write and drops the shard's slot; the next write to it reports the shard is not held.
- AC-6: a write to a shard whose owner is another node goes through `Forward` once, and logs nothing here.
- AC-7: a write to a shard nobody holds and no owner names fails with a 503 `node_log_exception` saying the shard is not open on this node, and logs nothing.
- AC-8: a delete and a custom-routed index in one group are staged in the same blob, and `refresh` reaches the engine of a held shard and refuses one not held.
- AC-10: a writer waiting on a shard whose stage failed is refused too: the slot is marked released under its lock, so none lands on an engine holding the refused write.
- AC-9: one item applied alone through `Target::apply` is staged too.

## Test plan

`crates/node/tests/roles/writer_role.rs` on the emulator; each criterion's test is run red before the code.

## S3 request budget

One PUT per window across all shards, unchanged from the committer; the first window of an epoch adds one lease write. No GET, no LIST on the write path.

## Bounds

Held shards are bounded by what the caller opens (admission and the shard cap come with node assembly); each holds an in-memory directory until its first publish. Reserving that heap from the `oss-mem` budget is the node-assembly piece's, and a failed reserve there refuses the open.

## Switch

The `writer` Cargo feature (a build switch, off by default). No behaviour switch: it ports the Java's ordering.

## Risks

- Dropping a slot while another thread writes it: the slot's lock is held through apply and stage.
- A grown mapping not reaching the descriptor: dynamic growth stays in the engine until the mapping-growth wiring piece.
