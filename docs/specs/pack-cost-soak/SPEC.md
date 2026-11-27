# Pack cost soak: a thousand shards publishing into node packs

## Serves

Parity plan R11.5, the part the commit soak ([commit-cost-soak](../commit-cost-soak/SPEC.md)) does not cover: the writer's request cost and heap at a thousand shards in the second placement mode. [writer-soak](../writer-soak/SPEC.md) holds the blob mode (2.0 requests a publish); this holds `node_pack`. It decides nothing about the default: `serverless.segments.node_pack.enabled` stays off until the collector exists ([pack-collector](../pack-collector/SPEC.md)).

## Change

A test in its own binary (`crates/writer/tests/pack_soak.rs`): 1,000 shards, each with its own directory and publisher over one metered emulator and one shared `PackWriter` (50 ms linger), commit a document and publish for three rounds, the 1,000 publishes of a round running together on a bounded set of threads so packs fill across shards. It counts the requests, the publishes per pack and the peak heap. No production code changes.

Not changed, by design and listed: takeover time, first search and write p99 of a packed node at 1,000 shards (the node suites hold packed takeover and reads at small counts: [node-pack-wiring](../node-pack-wiring/SPEC.md) AC-5; the scale figure is NOT-RUN), the collector.

## Acceptance criteria

- AC-1: with packs on, PUT plus LIST per publish over 3,000 publishes is below the blob mode's 2.0, and no publish lists.
- AC-2: every publish makes exactly one register swap, and the packs written plus the single ledger create account for every other PUT: no blob of its own for a small segment.
- AC-3: packs are shared: on average at least four publishes land in one pack.
- AC-4: the peak heap of 1,000 open shards, their directories and the pack writer stays under 128 MiB (provisional).

## Test plan

`crates/writer/tests/pack_soak.rs`, counting allocator and the emulator's meter.

## S3 request budget

Asserted: 3,000 swaps, one ledger create, the packs written (at most 750), 0 LIST.

## Switch

None: a test only.

## Risks

Heap is measured in a debug build on one machine: a bound against growth, not a size to quote. How many publishes share a pack depends on the thread count and the linger of this rig, not on the cluster.
