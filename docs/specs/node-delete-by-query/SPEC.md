# Delete by query over a writer node's shards

## Serves

R10.7 (delete-by-query), node side. Uses the coordinator's walk ([coordinator-by-query](../coordinator-by-query/SPEC.md)); the reader half (`_seq_no` and `_primary_term` in hits, sort and range on `_seq_no`) is the next change.

## Change

- `oss-node`: `by_query` with `POST /{index}/_delete_by_query`: `ByQueryService` parses the request, walks the index's shards (shard count from the routes) with `walk`, and deletes each hit through the `_bulk` machinery conditional on the hit's `_seq_no` and `_primary_term` (`DeleteChange`). The search is `ShardMatches`, a trait the reader implements in the next change.
- Parameters: `conflicts` (`abort`, `proceed`), `max_docs`, `refresh`, `scroll_size`, `timeout`. Refused 400: `wait_for_completion=false`, `slices` other than 1, `requests_per_second` other than -1.
- Not changed: update-by-query, `_routing` on hits (a custom-routed document is not found by its default shard), the reader.

## Acceptance criteria

- AC-1: the matching documents are deleted and the answer counts `total`, `deleted`, `batches`, `version_conflicts`, `noops`; documents not matched stay.
- AC-2: the `query` object is handed to the search as JSON text.
- AC-3: a document rewritten after the search is a version conflict: with the default `conflicts=abort` the answer is 409 with the conflicting id in `failures`; with `conflicts=proceed` it is 200, counted, and the walk goes on.
- AC-4: `max_docs` bounds the documents deleted.
- AC-5: a missing index is 404, a body without `query` or that is not JSON is 400, and the unsupported parameters above are 400.

## Test plan

`crates/node/tests/by_query_route.rs`: a real writer role and server, the search faked with the hits a test records from its own index responses. Seen failing with the conditional dropped from the delete.

## S3 request budget

Unchanged by this piece: the deletes are `_bulk` writes into the node log, one page of at most 1,000 at a time (admitted to a write budget), and the searches are the reader's.

## Switch

None: a new route; it deletes only what the caller's query selects.

## Risks

- One bulk run per hit: correct and simple, not the fastest. A page could be one group (`apply_group`); left for measurement.
- Until the reader change, the route needs a `ShardMatches`; nothing registers it in `assemble` yet.
