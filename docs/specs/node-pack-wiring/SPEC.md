# Node packs wired into the node

## Serves

Parity plan R11.1 (node packs), [ADR 0024](../../adr/0024-node-packs-in-the-writer.md), the S3 cost rule. Builds on [node-pack-placement](../node-pack-placement/SPEC.md), which left the settings and the node's use of `use_packs` for this step.

## Change

- Three node settings, `oss-node::writer::packs`: `serverless.segments.node_pack.enabled` (default `false`), `.max_blob_bytes` (default 8 MiB, 1 to 64 MiB) and `.linger_millis` (default 50, 1 to 5,000). Environment: `OSS_NODE_PACKS`, `OSS_NODE_PACK_MAX_BLOB_BYTES`, `OSS_NODE_PACK_LINGER_MS`; the two sizes alone change nothing while the switch is off.
- The writer role's `Publishing` takes an optional pack configuration. With it, the node makes one `PackWriter` for the life of the process, on the node's first publish while packing is on (so a node that never publishes makes no request for it): the epoch comes from the ledger (`next_epoch` over the wall clock), the ledger records it before the first pack. Every shard's publisher is given a `PackPlacement` for that writer before each publish, so one node's shards that publish within the linger share a pack.
- A failure to make the writer fails that publish (the shard stays due and is retried at the next pass); it never falls back silently to blobs.
- Not changed: the collector. [ADR 0024](../../adr/0024-node-packs-in-the-writer.md) keeps the switch off everywhere until [ADR 0025](../../adr/0025-node-pack-collection.md) is accepted and built, so a pack is written that nothing deletes: this change is for the bounded R11 measurement only (ADR 0025, decision 5). The cluster-register and per-index overrides (`index.serverless.segments.node_pack.enabled`) arrive with the collector and the flip, with the node's environment as the cluster default until then, as for commit staging.

## Acceptance criteria

- AC-1: the three settings are in the node's registry with the defaults above, and their bounds refuse a value just outside each.
- AC-2: `OSS_NODE_PACKS` is off by default; with it on, the sizes default to 8 MiB and 50 ms; a size outside its bounds is refused.
- AC-3: with the switch off, a publish writes its own segment blob, no pack and no ledger, exactly as before.
- AC-4: with it on, a publish of a small segment is one pack PUT and no blob of its own, the manifest names the file by a pack location, and the ledger is created once.
- AC-5: a node taking the shard over reads every document of a packed shard (the register's commit opens from the pack).
- AC-6: a node with the switch on that publishes nothing makes no pack or ledger request.
- AC-7: a later publish by the same node reuses the writer: same epoch, the ledger written once.

## Test plan

`crates/node/tests/roles/writer_packs.rs` over the emulator and its meter (AC-3 to AC-7; the pack-sharing of two shards in one linger is held by `node-pack-placement` AC-2); settings unit tests beside the settings (AC-1) and in `boot/config` (AC-2). Written first and seen failing.

## S3 request budget

Switch off: unchanged. On: a publish of a shard's small segments is one pack PUT (shared by every shard publishing in the linger) and its register swap instead of a blob PUT per segment and the swap; the first pack adds one ledger GET and create. No LIST, nothing per index or elapsed time.

## Switch

`serverless.segments.node_pack.enabled`, default `false`, via `OSS_NODE_PACKS`. It flips only after the collector exists and R11's measurement (PUT-class per publish down, cold open and warm search within 5%, takeover within the owner's bar).

## Risks

Packs nothing deletes while the collector is missing: the switch is for bounded measurement runs. Every lone publish waits the linger (default 50 ms): a measured cost, one reason for the switch.
