# A dead node's thousand packed shards are taken as fast as its blob-placed ones

## Serves

Parity plan R11.5, the part the pack soak ([pack-cost-soak](../pack-cost-soak/SPEC.md)) leaves out: takeover of 1,000 shards whose published commits live in node packs. Builds on [takeover-scale](../takeover-scale/SPEC.md) and [node-pack-wiring](../node-pack-wiring/SPEC.md). It decides no default: `serverless.segments.node_pack.enabled` stays off until the collector exists.

## Change

An evidence test, run by hand, in its own file (`crates/node/tests/takeover_packs.rs`) beside `takeover_scale.rs`. A node takes 4,000 documents across 1,000 shards (blob placement, then packs), publishes a commit for each shard, takes 2,000 more that stay in the log, dies, and a second node, also packing in the second run, takes all 1,000 through the proactive pass over the emulator at the Java soak's mean latency. The two takeovers are compared with each other, not with the Java's 57 s: the same rig and the same latency make the ratio meaningful. The rig gains `node_with_packs`; no production code changes.

Not changed, by design and listed: first search of a taken-over packed shard at 1,000 shards, and write p99 while a pass runs (the node suites hold packed reads at small counts, and `takeover-concurrency-default` holds the write p99 against the pass), RustFS.

## Acceptance criteria

- AC-1: in both placements the pass takes every shard the log names, makes no LIST, and no shard loses a document (its highest sequence number on the new node equals the dead node's, including the one that only the log held).
- AC-2: the packed takeover's wall time is at most 1.10 times the blob takeover's.
- AC-3: the packed takeover makes no more requests than the blob takeover, within a tenth.

## Test plan

`crates/node/tests/takeover_packs.rs`, ignored, run by hand with `--nocapture`: it fails if a shard the log names is not taken, a document is lost, a LIST is made, or a ratio is over its bound.

## S3 request budget

Unchanged by this change. Asserted: no LIST, and the packed requests within a tenth of the blob requests.

## Switch

None.

## Risks

The emulator's latency has no tail and the numbers are provisional. A replay that publishes into a pack waits the pack linger (1 ms here, 50 ms shipped), so a longer linger adds to a takeover in proportion to the shards a thread takes in turn; this rig uses 1 ms and does not show it.
