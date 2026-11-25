# index.blocks.read and index.blocks.metadata

## Serves

Parity plan R14.4: the last two block settings the settings update route refuses 501 ([index-blocks](../index-blocks/SPEC.md) did the write blocks).

## Change

- `index.blocks.read`: a reader that opens an index whose settings set it refuses the open, so every search of the index (search, count, field caps, point in time) answers 403 `cluster_block_exception`, "index [name] blocked by: [FORBIDDEN/7/index read (api)];".
- `index.blocks.metadata`: the metadata changes of the index are refused 403 `cluster_block_exception`, "index [name] blocked by: [FORBIDDEN/9/index metadata (api)];": a settings update (one that sets only `index.blocks.*` keys is allowed, so the block can be lifted), an alias add or remove naming the index, and the index's deletion (the last two in a following spec). Metadata reads are not blocked.
- The settings update route accepts both keys as `true` or `false`.

This spec is the read block and the metadata block on the settings update. Until the following spec lands, the metadata block does not stop an alias change or a delete, and the compatibility page says so.

## Acceptance criteria

- AC-1: a reader opening an index with `index.blocks.read` `true` refuses with 403 `cluster_block_exception` and the read-block reason naming the index; with `false` or none it opens.
- AC-2: a refused open is not kept: lifting the block lets the next search open the index.
- AC-3: with `index.blocks.metadata` `true`, a settings update that sets anything but `index.blocks.*` keys is refused 403 with the metadata-block reason and changes nothing; one that sets only `index.blocks.*` keys is applied.
- AC-4: `PUT /{index}/_settings` accepts `index.blocks.read` and `index.blocks.metadata` as `true` or `false`, and refuses any other value 400.
- AC-5: a several-index update (`settings-update-many`) refuses before the first write if any index it resolves to blocks it.

## Test plan

Reader unit tests for AC-1 and AC-2 (the open's settings check); `crates/node/tests/server/settings_route.rs` for AC-3 to AC-5. A reader end-to-end test (`read_block.rs`) covers AC-1 and AC-2 through a real search.

## S3 request budget

None added: both ride on descriptor reads already made (a shard open; the settings update's own resolve; the delete's and alias's own reads).

## Bounds

None.

## Switch

None: a block the client set is a promise.

## Risks

A realtime get on a writer node reads the writer's own shard, not a reader's open, so it is not stopped by the read block.


The read block takes effect when a reader opens the index, so a reader that already holds it open keeps serving reads until it next opens it (the same limit as `index.max_result_window`); lifting it takes effect at once, since a refused open is not kept. The metadata block is read from the descriptor each request already reads, so it is current.
