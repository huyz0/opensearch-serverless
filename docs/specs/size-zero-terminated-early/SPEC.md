# Size 0 aggregations: terminated_early

## Serves

Roadmap R10.1 and R10.2; invariant I-13. Stock OpenSearch 3.10.0 writes `"terminated_early": true` on a `size: 0` search with aggregations; this node left it out (the one difference listed in [stock-breadth-compare](../stock-breadth-compare/VERIFIED.md) and in `docs/compatibility.md`).

## Change

Recorded from stock 3.10.0 (`fixtures/compat/search-breadth/size-zero-aggs.json`), the flag is written when all of these hold: `size` is 0; the search has an aggregation, a pipeline aggregation alone included; there is no `post_filter` and no `min_score`; the query can be counted from the index (none, `match_all` on an index with no nested mapping, or a `term`); the shard has a segment. `track_total_hits`, a sort, `from` and the URL form of `size` change nothing. A range query, a two-word `match`, a `post_filter`, no aggregation and `size: 1` leave it out. `terminate_after` keeps its own answer. The coordinator already ORs the shards' flags.

Not served, and recorded as such: a one-word `match` (stock counts it as a term and marks it), `terminate_after` and `min_score` beside aggregations (this node refuses them), and the other queries stock may count (a `bool` of one term, for instance).

## Acceptance criteria

- AC-1: every step of `search-breadth/size-zero-aggs.json` gets stock's recorded status and body from an assembled node.
- AC-2: the reader writes the flag exactly under the rule above, including the shapes that leave it out.
- AC-3: the nested-aggregation fixtures, which stock answers without the flag (a nested index wraps a match-all query), still replay.

## Test plan

`a_size_zero_search_with_an_aggregation_is_terminated_early_as_stock_marks_it` (crates/reader/src/search_tests.rs); the breadth replay `an_assembled_node_answers_every_breadth_fixture_as_stock_opensearch_did` over a third fixture; the nested replay is the existing one.

## S3 request budget

Unchanged: no request is added. LIST 0.

## Switch

None: it is a field stock writes, with no downside.

## Risks

The rule is read from nine recordings of one-shard indexes, not from stock's source; a query shape not in them may differ. The listed gaps keep that honest.
