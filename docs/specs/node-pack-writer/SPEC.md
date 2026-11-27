# Node pack store, writer and ledger

## Serves

Parity plan R11 (node packs), [ADR 0024](../../adr/0024-node-packs-in-the-writer.md), and the S3 cost rule (a PUT per shard publish is the cost this removes). Builds on [node-pack-format](../node-pack-format/SPEC.md).

## Change

- `oss-writer::pack` gains `PackStore` (`write` one pack from parts into `packs/<node>/e=<epoch>/<ordinal>`, create-if-absent, one PUT, streamed through one 64 KiB buffer), `PackWriter` (the leader and linger batching of `NodePackWriter`: `add` blobs, wait for the pack, get a `Landed` per blob whose `file_at` is a manifest location) and `PackLedger` (the epoch register: `read`, `add_epoch`, `update` by compare-and-swap, 16 attempts).
- Not changed: the publisher does not use packs yet, the collector and the setting do not exist yet. Nothing outside `pack` calls this, so no request changes anywhere.

## Acceptance criteria

- AC-1: parts of several shards are one PUT whose bytes are the header then the parts; each landing's offset holds its part's bytes; `file_at` names a range inside the block and refuses one outside it; the location resolves to the pack's key through `ShardRef`.
- AC-2: a taken slot is `Taken` and its bytes are unchanged.
- AC-3: publishes arriving within the linger share one pack (one PUT, one ordinal); a lone publish is its own pack after the linger; a batch at `max_pack_bytes` is written without waiting out the linger.
- AC-4: a failed pack fails every publish in it and its ordinal is spent.
- AC-5: the writer's epoch is recorded in the ledger once, before its first pack, and not again per pack; a second writer's epoch is added beside it.
- AC-6: epochs recorded concurrently all end up in the ledger; recording one already present writes nothing.
- AC-7: a 64 MiB pack of two parts is written with peak heap below 2 MiB.
- AC-8: the next epoch is the clock, or one past the highest epoch in the ledger when that is later.
- AC-9: a leader whose write panics leaves its followers an error, never a wait.

## Test plan

`crates/writer/tests/it/pack_writer.rs` over the in-process emulator and its request meter (AC-1 to AC-6, AC-8, AC-9); `crates/writer/tests/pack_memory.rs` under the counting allocator (AC-7).

## S3 request budget

One pack: 1 PUT (create-if-absent), 0 GET, 0 LIST, however many parts. The first pack of a writer adds one register create (the ledger); later packs add none. Nothing scales with documents, indexes or time.

## Switch

None here: nothing is wired. `serverless.segments.node_pack.enabled` arrives with the publisher's use, default off ([ADR 0024](../../adr/0024-node-packs-in-the-writer.md)).

## Risks

A leader that panics after taking its batch drops its followers' senders, so each gets an error (AC-9); before taking it, the leader only waits on a condition variable, with no code that can fail. A PUT that never returns stalls its batch until the store's own timeout. A pack the Java cannot open: the header is pinned to the Java's bytes by node-pack-format AC-1.
