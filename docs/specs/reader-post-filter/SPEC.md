# Reader: post_filter

## Serves

Parity task R10.4 (search features); invariant I-13. The search body's `post_filter` was refused as not supported yet.

## Change

`oss-reader` parses `post_filter` as one query (a malformed one fails as `query` does). The shard collects its hits and their total from the query with the filter required and unscored (`QueryPhase`'s post-filter collector sits in front of the top-docs collector only): the hits, their scores, the total and `max_score` are those of the query with the filter inside it as a `filter` clause. The aggregations, `top_hits` among them, still run over every match of the query.

Refused (400, "[post_filter] with ... is not supported by this node yet"): `min_score`, `terminate_after`, `explain` and `track_scores`, where OpenSearch orders the collectors or builds the explanation differently and this node does not reproduce it.

## Acceptance criteria

- AC-1: with `post_filter`, the hits, scores, total and best score equal those of the query with the same filter as a `filter` clause of a `bool`; fewer than without it.
- AC-2: aggregations are the same with and without the `post_filter`.
- AC-3: `post_filter` beside each refused option, and one that is not a query, are 400; a filter nothing satisfies leaves no hits and a total of 0.

## Test plan

Reader unit tests in `crates/reader/src/search_tests.rs`; `crates/node/tests/it/search_hits.rs` end to end over `compat-term`. Seen failing with the filter not given to the collectors (hits and total were the plain search's). Expected bodies are derived from the equivalent filter query, not recorded from OpenSearch: provisional.

## S3 request budget

Unchanged: the same segments and norms; the filter reads postings through the page cache. LIST 0, HEAD 0, PUT 0.

## Risks

- A filter's postings are read for each hit-collecting pass; the aggregations make their own pass over the query, so a request with both reads the main query twice, as it already does.
