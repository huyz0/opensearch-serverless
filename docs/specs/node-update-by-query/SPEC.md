# Update by query with a Painless script over a writer node's shards

## Serves

R14.6, "runs no script": `POST /{index}/_update_by_query` with a `script`, on the walk of [node-delete-by-query](../node-delete-by-query/SPEC.md) and the script runner of [node-update-route](../node-update-route/SPEC.md). Served only with `serverless.scripting.painless.native_compile` on; with it off the request is refused 501 before any search.

## Change

- `oss-node`: `POST /{index}/_update_by_query` takes `query` (default `match_all`) and a required `script` (`source`, `lang` painless or absent, `params`). The walk finds each hit; the change reads the document (a realtime get on the shard its id routes to), runs the script over `ctx._source`, `ctx.op`, `ctx._id`, `ctx._index` and `ctx._version`, and writes the result through the `_bulk` machinery conditional on the hit's `_seq_no` and `_primary_term`: `ctx.op` of `index` writes the new source, `delete` deletes, `noop` or `none` writes nothing.
- Parameters as `_delete_by_query`: `conflicts`, `max_docs`, `refresh`, `scroll_size`, `timeout`; refused 400: `slices` other than 1, `requests_per_second` other than -1. A request with no `script` is refused 400 (a rewrite without a script is not served).
- The answer counts `total`, `updated`, `deleted`, `batches`, `version_conflicts`, `noops`.
- A script that does not compile, is not run as Painless runs it, or throws fails the request 400 `script_exception`; documents already changed stay changed, as in OpenSearch.
- Not changed: `_delete_by_query`, the reader, the writer crate. Not served: documents with custom `_routing` (the walk and the read use the default shard), a script `id`.

## Acceptance criteria

- AC-1: the documents the query matches are rewritten by the script (`ctx._source` edits) and the answer counts `total` and `updated`; documents not matched stay.
- AC-2: `ctx.op = 'delete'` deletes the document and counts `deleted`; `ctx.op = 'noop'` leaves it and counts `noops`.
- AC-3: a document rewritten after the search is a version conflict: with `conflicts=abort` the answer is 409 naming it; with `conflicts=proceed` it is 200, counted, and the walk goes on.
- AC-4: `max_docs` bounds the documents changed.
- AC-5: with the switch off the request is 501; a body without `script`, a script that does not compile or throws, a missing index and the unsupported parameters are refused 400 or 404 as stated.

## Test plan

`crates/node/tests/server/update_by_query.rs`: a real writer role and server over the emulator, the search faked as in the delete-by-query test, the switch turned on by the test's own process setting.

## S3 request budget

Per hit: one realtime get plus one staged write, as an `_update` does; the searches are the reader's; no LIST.

## Bounds

A page is at most 1,000 hits and each hit is read and written one at a time, so at most one document source and its script state are held at once; the script runs under ADR 0069's memory and loop limits, and the request body is bounded to 1 MiB.

## Switch

`serverless.scripting.painless.native_compile`, off by default.

## Risks

- One get and one bulk run per hit: correct and simple, not the fastest.
- Between the read and the write another writer may change the document; the write carries the hit's sequence number, so that is a version conflict, not a lost update.
