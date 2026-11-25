# The metadata block on alias changes and index deletion

## Serves

Parity plan R14.4: finishes `index.blocks.metadata` ([index-blocks-read-metadata](../index-blocks-read-metadata/SPEC.md) did the settings update).

## Change

An index whose settings set `index.blocks.metadata` to `true` refuses, 403 `cluster_block_exception` with "index [name] blocked by: [FORBIDDEN/9/index metadata (api)];":

- an alias add or remove naming the index (`PUT|POST|DELETE /{index}/_alias/{name}` and each add or remove action of `POST /_aliases`);
- its deletion (`DELETE /{index}`).

Reads are not blocked. An index that does not exist, or has no block, answers as before (an alias add to a missing index is still 404 from the alias protocol).

In `POST /_aliases` every named index is checked before the first action is applied, so a blocked index stops the whole request with nothing changed.

## Acceptance criteria

- AC-1: an alias add naming a metadata-blocked index is refused 403 with the metadata-block reason, and the alias is not created.
- AC-2: an alias remove naming a metadata-blocked index is refused 403, and the alias keeps the index.
- AC-3: `POST /_aliases` with one blocked index among several refuses 403 and applies none of its actions.
- AC-4: `DELETE /{index}` of a metadata-blocked index is refused 403 and the index stays; after the block is lifted by a settings update the delete succeeds.
- AC-5: an index without the block, or with it `false`, is unaffected by all of the above.

## Test plan

Server tests in `crates/node/tests/server/metadata_block.rs` for AC-1 to AC-5 over real routes (alias, delete and settings together on one descriptor store).

## S3 request budget

One descriptor GET for each distinct index a change names, before the alias protocol's own reads (a request, not an index count: an alias request names a handful of indices, and the body is capped at 64 KiB). The delete reads the descriptor once more than before; it is a GET and no LIST. Idle indices make no request.

## Bounds

The descriptor read uses the service's buffer, already reserved by the route.

## Switch

None: a block the client set is a promise.

## Risks

The check and the change are two steps, so a block set between them is not seen by that request (the same window as any descriptor read).
