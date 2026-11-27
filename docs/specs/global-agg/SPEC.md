# Reader: `global` aggregation

## Serves

Roadmap R10.2: OpenSearch's `global` bucket aggregation of a single-shard `_search`: one bucket of every live document of the shard, whatever the query, with sub-aggregations over them, and a shard-level result a coordinator reduces across shards (I-13, REST matches OpenSearch; ADR 0004, bounded memory).

## Change

`oss-reader` parses `global` (as `GlobalAggregationBuilder.parse` does at OpenSearch 3.10.0: its body is an empty object) and serves it as a `filter` of `match_all` ([filter-agg](../filter-agg/SPEC.md)): the result, reduce and answer are the `filter`'s (one count, `"doc_count":n`), and its sub-aggregations run as under `filter`. The search runs the `global` aggregations in a pass of their own under a `match_all` query (`metric::collect_with_global`), the others under the search's query, and puts the results back in request order. `post_filter`, `min_score` and the query do not narrow a `global`.

Not in this change, each refused: a `global` under another aggregation ("[global] sub-aggregation of a [x]", as every unserved nesting), and a `top_hits` under a `global` ("not supported by this node yet").

## Acceptance criteria

- AC-1: `{"global": {}}` parses; any other content fails with OpenSearch's "Unexpected token ... in [name]."
- AC-2: a `global` counts every live document whatever the query, and its sub-aggregations run over them; aggregations beside it still run over the query's matches; results keep request order.
- AC-3: a `global` under another aggregation, and a `top_hits` under a `global`, are refused.

## Test plan

Unit tests in `aggs/bucket/global_tests.rs` (AC-1, AC-3) and over an `oss-testkit` index (AC-2). Expected values come from reading `GlobalAggregationBuilder` and `GlobalAggregator` at 3.10.0; no stock recording exists yet.

## S3 request budget

The `global` pass reads the doc values its sub-aggregations name, through the same page cache and ranged GETs as any aggregation. LIST 0, HEAD 0, PUT 0; an idle index makes none. A `global` beside others adds a second pass over the same segments, not more blobs.

## Switch

None: a `global` aggregation was refused before, and a request without one is unchanged.

## Risks

No stock 3.10.0 recording backs these steps. `GlobalAggregationBuilder` refuses a non-top-level `global` with its own message; this node's is the generic nesting refusal.
