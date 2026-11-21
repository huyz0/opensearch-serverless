# A search answers a top-level top_hits across its shards

## Serves

Parity plan R7.6: the coordinator's merge and render of a `top_hits` ([merge](../coordinator-top-hits-reduce/SPEC.md), [render](../coordinator-top-hits-render/SPEC.md)) are part of the search path. A search over several shards whose aggregations hold a top-level `top_hits` answers it as one shard would: the best hits of all the shards, each written by the shard that ranked it.

## Change

- `Shards` gains `top_hits_window(name)` (the `from`, `size` and sort directions of the request's top-level `top_hits` of that name; none by default) and `render_top_hits(shard, query, name, positions)` (the hits of that aggregation, written as the shard's own response writes them).
- `search` merges each such aggregation with `reduce_top_hits`, fetches its page with `render_page` through `render_top_hits`, and writes the body in the aggregation's place. A `top_hits` the shards' request does not describe is refused as before.
- `HitsWriter` receives the aggregation's name.
- The node's shards describe the window from the request and render from the shard's held `top_hits` result (`ShardQuery::render_top_hits`).
- Not here: a `top_hits` inside a bucket aggregation, which still has no reduce.

## Acceptance criteria

- AC-1: a search over two shards writes a `top_hits`'s merged total and best score and its page of hits in merged order, each hit asked of the shard that ranked it, once a shard.
- AC-2: the aggregation's `from` is applied once over all the shards, not once a shard.
- AC-3: a `top_hits` the request does not describe is refused as before.
- AC-4: over an index of two shards on one node, a sorted `top_hits` answers the right number of hits in sort order, and its best is no worse than one shard's.

## Test plan

`crates/coordinator/tests/search_top_hits.rs` (AC-1 to AC-3); `crates/node/tests/it/coordinate.rs` (AC-4).

## S3 request budget

No request added: each shard prefetches its selected hits' stored fields in one round, as a response that wrote them would.

## Bounds

At most 100 hits a shard and aggregation (the inner window); rendered hits are held until the body is written.

## Switch

None: no measured downside.
