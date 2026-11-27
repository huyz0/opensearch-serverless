# Reindex between two indexes of a writer node, with an optional Painless script

## Serves

R14.6's script gap and the reindex API: `POST /_reindex` copies the documents a `source.query` matches in one index into another, optionally through a Painless script. On the walk of [node-delete-by-query](../node-delete-by-query/SPEC.md) and the script runner of [node-update-route](../node-update-route/SPEC.md). A script is served only with `serverless.scripting.painless.native_compile` on; a request with a script and the switch off is 501 before any search.

## Change

- `oss-node`: `POST /_reindex` takes `source.index` and `dest.index` (one concrete index each), `source.query` (default `match_all`), `dest.op_type` (`index`, the default, or `create`), `script` (optional), `conflicts` and `max_docs`. The walk finds the source's hits; for each one the change reads the document from the source (a realtime get on the shard its id routes to), runs the script over `ctx._source`, `ctx.op`, `ctx._id`, `ctx._index` and `ctx._version` when there is one, and writes the document to the destination under the same `_id`: `ctx.op` of `index` writes, `noop` or `none` writes nothing, `delete` deletes the destination's document of that id.
- `op_type: create` makes an existing destination document a version conflict. The write does not carry the source's sequence number: a reindex copies what the read returned.
- The answer counts `total`, `created`, `updated`, `deleted`, `batches`, `version_conflicts` and `noops`.
- Parameters: `conflicts` (body or query string: `abort` or `proceed`), `max_docs` (body or query string), `refresh`, `timeout`, `scroll_size`; refused 400: `wait_for_completion=false`, `slices` other than 1, `requests_per_second` other than -1, any other body or `source` or `dest` field (`remote`, `_source`, `slice`, `size`, `routing`, `pipeline`, `version_type`), a source or destination that is a list, a pattern or missing, a script `id`, and a source that is the destination.
- A script that does not compile, is not run as Painless runs it, or throws fails the request 400 `script_exception`; documents already written stay written.
- Not served: changes the script makes to `ctx._id`, `ctx._index`, `ctx._routing` and `ctx._version` (the write keeps the source's id and goes to `dest.index`), custom `_routing`.

## Acceptance criteria

- AC-1: the documents the source query matches are written to the destination with the same ids and sources, the source stays, and the answer counts `total` and `created`; documents not matched are not copied.
- AC-2: with a script, `ctx._source` edits are what the destination holds, `ctx.op = 'noop'` writes nothing and counts `noops`, and `ctx.op = 'delete'` deletes the destination's document and counts `deleted`.
- AC-3: with `dest.op_type: create` a document the destination already has is a version conflict: with `conflicts=abort` the answer is 409 naming it, with `proceed` it is 200, counted, and the walk goes on; without `create` it is overwritten and counted `updated`.
- AC-4: `max_docs` bounds the documents written.
- AC-5: a missing source or destination index is 404; a body that is not an object, a missing or list `source.index` or `dest.index`, a source equal to the destination, a field this node does not serve, the unsupported parameters and a script with the switch off are refused as stated; a script error is 400.

## Test plan

`crates/node/tests/server/reindex_route.rs` (rig in `by_query_rig.rs`): a real writer role and server over the emulator with the two indexes held here and the search faked as in the delete-by-query test.

## S3 request budget

Per hit: one realtime get on the source plus one staged write on the destination, as an index through `_bulk`; the searches are the reader's; no LIST.

## Bounds

A page is at most 1,000 hits, each read and written one at a time, so one document source and its script state are held at once; scripts run under ADR 0069's limits and the request body is bounded to 1 MiB.

## Switch

None for a reindex without a script (a new route, it copies only what the caller selects); a script needs `serverless.scripting.painless.native_compile`, off by default.

## Risks

- One get and one bulk run per hit: correct and simple, not the fastest.
- The destination write is not conditional on the source's version: a source document changed between the search and the read is copied as the read saw it, as OpenSearch's scroll-based copy does.
