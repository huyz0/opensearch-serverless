# The first open of a thousand packed shards costs what the blob open does

## Serves

Parity plan R11.5, the part [pack-cost-soak](../pack-cost-soak/SPEC.md) and [pack-takeover-scale](../pack-takeover-scale/SPEC.md) leave out: a reader's first (cold) open of 1,000 shards whose published commits live in node packs. It decides no default: `serverless.segments.node_pack.enabled` stays off until the collector exists.

## Change

An evidence test, run by hand, in its own file (`crates/node/tests/first_search_packs.rs`). One index of 1,000 shards, four documents each, is published twice over a fresh emulator: as blobs, then into node packs (50 ms linger, shared writer). A reader then opens every shard cold, 16 at a time, at the Java soak's mean latency (20 ms, 7 ms jitter), and the two runs are compared with each other. No production code changes.

Not changed, by design and listed: write p99 of a packed node, RustFS, the collector.

## Acceptance criteria

- AC-1: in both placements all 1,000 shards open with every document and no LIST.
- AC-2: the packed opens take at most 1.05 times the blob opens' wall time.
- AC-3: the packed opens make no more requests than the blob opens.

## Test plan

`crates/node/tests/first_search_packs.rs`, ignored, run by hand with `--release --nocapture`.

## S3 request budget

Unchanged by this change. Asserted: no LIST, packed requests at most the blob requests.

## Switch

None.

## Risks

The emulator's latency has no tail and the numbers are provisional.
