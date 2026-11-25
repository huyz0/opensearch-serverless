# index.blocks.write and index.blocks.read_only refuse writes

## Serves

Parity plan R14.4: the behaviour-changing settings the settings update route refuses 501 ([settings-update-route](../settings-update-route/SPEC.md)). This is the first, the write blocks. `index.max_result_window` and the read block follow in their own specs.

## Change

The writer's `IndexInfo` (what a bulk knows of an index, read from the descriptor and confirmed by the change stripes) gains `write_block`: the most severe write block the settings name. A bulk item against a blocked index fails with 403 `cluster_block_exception`, on the node that took the bulk and again on the owner a forwarded group reaches, so a stale view at the sender does not write. The settings update route accepts `index.blocks.write`, `index.blocks.read_only` and `index.blocks.read_only_allow_delete`, each `true` or `false`. `index.blocks.read`, `index.blocks.metadata` and `index.blocks.read_only` on the metadata side stay refused 501 until their own pieces.

## Acceptance criteria

- AC-1: with `index.blocks.write` or `index.blocks.read_only` `true`, every bulk item (index, create and delete; a bulk `update` item is refused 501 by this node already) against the index fails 403 `cluster_block_exception`, reason `index [name] blocked by: [FORBIDDEN/8/index write (api)];`, and nothing is written.
- AC-2: with only `index.blocks.read_only_allow_delete` `true`, an index or create item fails 403 with `index [name] blocked by: [FORBIDDEN/12/disk usage exceeded flood-stage watermark, index has read-only-allow-delete block];` and a delete item succeeds.
- AC-3: setting a block to `false`, or removing it with null, lets writes through again.
- AC-4: a block set through the settings route is seen by a bulk within one confirmation window, with no extra request: the block is read from the descriptor a bulk already reads.
- AC-5: a block value that is not `true` or `false` is refused 400 `illegal_argument_exception`.
- AC-6: an index with no block costs the same requests as before; the request-budget rows are unchanged.

## Test plan

`crates/writer/tests` for AC-1 to AC-3 over the bulk run with a fake `IndexInfo`; `crates/node/tests/server` for AC-3 to AC-5 through the settings route and a bulk, on the emulator; the request-budget rows for AC-6.

## S3 request budget

None added: the block rides on the descriptor read a bulk already makes.

## Bounds

One enum on `IndexInfo`.

## Switch

None: no downside, and a block the client set is a promise.

## Risks

A block set takes effect when the bulk's cached index is next confirmed, one confirmation window (`OSS_INDEX_CONFIRM_MS`, 5 s by default), not instantly, even on the node that took the settings update. A write inside that window passes. Core applies a block in cluster state at once; this node cannot without a request a write.
