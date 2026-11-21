# Node pack placement in the shard publisher

## Serves

Parity plan R11 (node packs), [ADR 0024](../../adr/0024-node-packs-in-the-writer.md), the S3 cost rule. Builds on [node-pack-writer](../node-pack-writer/SPEC.md) and [segment-publish](../segment-publish/SPEC.md); ported from `SegmentPublisher.writeSegmentBlobs` and `writeToNodePack`.

## Change

- `ShardPublisher::use_packs(Some(PackPlacement))` sends a publish's small segment blobs (header plus files at most `max_blob_bytes`) to the node's `PackWriter`, all of one publish in the same pack, and records each file's manifest location as `@<node>/e=<epoch>/<ordinal>#<offset>:<length>`. `None`, the default, is the publisher as before. The choice is made per publish, so a later switch of a setting acts at the next publish.
- Larger blobs, empty files and everything when packs are off keep their own blobs as before.
- **Swap deadline:** a publish whose pack was asked for more than `SWAP_DEADLINE` (5 minutes) before its register swap is refused with `PublishError::PackExpired` before the swap, writing no register; the next publish packs the files anew. The age counts from before the pack was asked for, on the monotonic clock, so it never undercounts.
- Not changed: no setting yet (the node's wiring and the settings come next, off by default), no collector ([ADR 0024](../../adr/0024-node-packs-in-the-writer.md) keeps the switch off until its own ADR), and no change to any publish without a placement.

## Acceptance criteria

- AC-1: with packs on, a refresh is one pack PUT and one register swap (plus the ledger's create the first time), no blob of its own, and each manifest location holds its file's bytes.
- AC-2: two shards publishing within the linger through one writer share one pack.
- AC-3: a blob over `max_blob_bytes` keeps its own blob; a publish without a placement writes blobs and no pack or ledger; `use_packs(None)` after packs goes back to blobs.
- AC-4: a swap past the deadline is `PackExpired` and writes no register; the next publish succeeds and names packed locations.
- AC-5: the reader opens a commit whose files all live in a pack and counts its documents.

## Test plan

`crates/writer/tests/publish_packs.rs` over the emulator and its meter (AC-1 to AC-4); `crates/node/tests/publish_read.rs` (AC-5), which has both roles. Seen failing with `use_packs` made a no-op: three of the four writer tests failed.

## S3 request budget

A publish of `n` small segments of one shard with packs on: 1 pack PUT (shared with every shard publishing in the linger), 1 swap, 0 GET after the first publish, 0 LIST; the first pack of a writer adds one ledger create. Without a placement: unchanged.

## Switch

`use_packs` is the mechanism; the settings that drive it (`serverless.segments.node_pack.enabled`, max blob bytes, linger), default off, arrive with the node's wiring. Existing publish tests pin the off behaviour; these tests cover the on behaviour.

## Risks

A pack deleted before its manifest is swapped: the deadline check bounds the age but has a residual window while a writer is paused between the check and the swap; the collector's ADR must close it. A reader that cannot follow a node-packed location: AC-5 and the Java-pack fixtures in `crates/format` hold it.
