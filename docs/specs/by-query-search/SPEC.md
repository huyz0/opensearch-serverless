# Delete by query: the search from the node's own reader

## Serves

R10.7 (delete-by-query), reader half. Completes [node-delete-by-query](../node-delete-by-query/SPEC.md): the route is registered in `assemble` and finds its documents through the node's `SearchService`.

## Change

- `oss-reader`: a sort on `_seq_no` (a long, missing values last, as `FieldSortBuilder` does for a numeric field); `docvalue_fields` of exactly `_seq_no` and `_primary_term`, written as long values. Other `_`-prefixed names and patterns that could match them stay refused.
- `oss-node`: `by_query::SearchMatches`, `ShardMatches` over `SearchService::query_shard`: the newest match is a one-hit search sorted by `_seq_no` descending; a page is a search sorted ascending with `search_after` the last number seen, its hits cut at the newest. Each hit is read back as its `_id`, `_seq_no` and `_primary_term`.
- `assemble` registers `POST /{index}/_delete_by_query` on a node with the `coordinator` feature.
- Not changed: `seq_no_primary_term` in a search body, a range query on `_seq_no`, update-by-query, `_routing`, hits from a shard's unpublished tail (the search sees what the node's reader sees).

## Acceptance criteria

- AC-1: a sort on `_seq_no` is a long sort with missing values last, ascending or descending.
- AC-2: `docvalue_fields` resolves `_seq_no` and `_primary_term` as long fields, each once however often named.
- AC-3: through a node assembled from its roles, a delete by query removes the documents its query matches, keeps the rest, and answers the counts.

## Test plan

Unit tests in `search_sort_tests.rs` and `docvalue_fields_tests.rs`; `crates/node/tests/server/assemble.rs` runs a delete by query against a real writer role, its publisher and the reader.

## S3 request budget

No new request kinds: the searches are the reader's, the deletes `_bulk` writes into the node log.

## Switch

None: a new route that deletes only what the caller's query selects.

## Risks

- A search sees published data: documents written since the last publish are not found until the publisher runs.
- A page re-runs the query per page (`search_after`), so a walk over many matches costs one search each; bounded by `max_docs` and the page size.
