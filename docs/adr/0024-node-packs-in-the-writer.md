# 0024: Node packs in the writer: one PUT for the segments a node publishes together

**Status:** Proposed

## Context

A publish writes one blob per new segment of one shard ([segment publish](../specs/segment-publish/SPEC.md)). A node holding thousands of small shards that publish within the same second pays a PUT per shard per publish, however small the segment is. The Java shell measured this (1,000-shard soak, 2026-11-12): node packs cut PUT-class requests per publish from 2.09 to 0.68, with first-search latency no worse, at a takeover cost the shell accepted (the Java's `NodePackWriter`, `NodePackStore`, `NodePackLedger`). Reads already work: the Rust reader follows a manifest's `@<node>/e=<epoch>/<ordinal>#<offset>:<length>` locations ([blob formats](../specs/blob-formats/SPEC.md)). Nothing in Rust writes a pack, so a mixed fleet gets packs only from Java nodes.

## Decision

Port the Java's pack writer, as-is on the wire, in `oss-writer::pack`, in pieces: the format writers ([node-pack-format](../specs/node-pack-format/SPEC.md)), the store, batching writer and ledger (`node-pack-writer` spec), the publisher's placement, and the collector.

- **One pack per window.** A publish hands its small segment blobs (at most `max_blob_bytes`, default 8 MiB; larger blobs keep their own PUT) to the node's one pack writer. The first blob into an empty batch becomes the leader, waits up to `linger` (default 50 ms) or until the batch holds 64 MiB, then writes the batch as one blob with create-if-absent. No thread of its own; a node whose publishes never overlap pays the PUT it paid without packs.
- **Fenced by reachability, not by lock.** A pack is created once under `packs/<node>/e=<epoch>/<ordinal>` and never changed. It becomes reachable only through a shard's manifest swap, itself fenced by the register's term. A stale writer's pack is therefore inert. A swap made more than `SWAP_DEADLINE` (5 minutes) after its pack landed is refused, which is what lets collection delete a pack nothing names once it is three deadlines old.
- **No LIST.** Names are derived: the epoch is the wall clock at writer start, ordinals count up, and a per-node ledger register (`packs/<node>/ledger`) records each epoch with how far collection has got and the highest ordinal seen. The ledger is written when a writer starts packing and when a collection pass moves, never per pack. A node that never packs never writes it.
- **Streaming.** A pack is a PUT body made of the header and each part's source, pulled through one 64 KiB buffer: no pack is held in memory (asserted by a counting-allocator test over a 64 MiB pack).
- **A switch, `serverless.segments.node_pack.enabled`, default off** in this repository until the R11 exit measures, under the emulator with latency, what the Java measured: PUT-class requests per publish down, cold open and warm search within 5%, takeover within the bar the owner set. The flip is its own PR. With it off, publish is unchanged and nothing in this ADR runs; tests pin both values.

## Consequences

- Requests: a publish window of `n` shards is 1 PUT plus its register swaps instead of `n` PUTs plus swaps; the swaps still scale with shards, the blobs no longer do. 0 LIST. A ledger create at first pack, a swap per collection pass that moves it.
- Memory: the batch holds `Part`s (names and open sources) for the shards publishing in one linger, bounded by `max_pack_bytes` of source length and by the publish pool; bytes stream.
- Idle indices make zero requests: nothing runs without a publish.
- Latency: a publish waits up to `linger` for its pack (50 ms default), on its own publishing thread.
- Cost of failure: a failed pack fails every publish in it; each retries on its next publish. A spent ordinal is never reused.
- Collection is a follow-up and must be safe before packs are on anywhere: until it exists the switch stays off, so no pack is ever written that nothing can delete.
- Rules out: a per-shard pack, packs keyed by anything but computed names, a LIST to find packs.
