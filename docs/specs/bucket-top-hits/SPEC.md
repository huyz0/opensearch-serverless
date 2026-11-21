# top_hits inside a bucket aggregation

## Serves

Parity plan R10: a `top_hits` as the sub-aggregation of a bucket aggregation, the commonest use of `top_hits` (the best document of each term). Builds on [top-hits-agg](../top-hits-agg/SPEC.md) (one shard, top level) and [coordinator-search-top-hits](../coordinator-search-top-hits/SPEC.md) (merge and render, top level).

## Change

The engine (lucene-aggs `AggNode::TopHits` and `SortedTopHits`) already keeps a collector for each owning bucket, so this is the reader and coordinator work around it, in four slices, each its own PR:

1. **Shard** (tested over `terms`, `histogram`, `range` and `filters`; `explain` and `highlight` under a bucket are refused; a sort's refusal names the index uuid). `top_hits` is accepted under every bucket aggregation this node serves; the sub values of every bucket type build a `Value::TopHits` from the engine's per-bucket hits (documents and scores or sort values, with the hits ahead of `from` kept as the top-level result does).
2. **Shard response.** A shard's own response writes each bucket's hits (a one-shard search is a valid response), with one stored-field prefetch over every list the response writes.
3. **Reduce.** The coordinator merges a bucket's `top_hits` across the shards that hold the bucket (matched by key, as the buckets are), then fetches each bucket's page from the shards that ranked it. `reduce_sub` stops answering the first shard's value for a `top_hits`.
4. **Fixtures and docs.** Compatibility notes and the fixtures of slices 1 to 3.

Not here: `top_hits` under a `nested` or `children` aggregation, `significant_terms`, and `composite`, which this node does not serve.

## Acceptance criteria

- AC-1: a `terms` of a keyword field with a `top_hits` sub on one shard answers each bucket's best hits in the sub's sort.
- AC-2: the same under a `histogram`, a `range` and a `filters`.
- AC-3: a `top_hits` under a bucket of a search over several shards holds the best hits of that bucket across all the shards, and each hit is written by the shard that ranked it.
- AC-4: a bucket one shard lacks is merged from the shards that have it.
- AC-5: the bytes a bucket's hits can hold are part of what the shard reserves before it collects, bounded by the shard's documents and, for a large shard, growing with the window.

## Test plan

Slice 1: reader tests over a real index of several segments, comparing each bucket's hits with a `top_hits`-less search filtered to that bucket. Slice 3: a coordinator test with fake shards (as `search_top_hits.rs`) and a node test over two shards.

## S3 request budget

No request added: the buckets and their hits come from the segments the search already reads, and each shard prefetches the selected hits' stored fields in one round.

## Bounds

At most 100 hits a bucket (the inner result window) and the request's bucket limit; held hits are reserved from the request-body pool.

## Switch

None: no measured downside.

## Risks

Memory is buckets times window: a `terms` of 65,535 buckets with a 100-hit window must be refused by the reservation, not by an out-of-memory. Slice 1 settles the unit of reservation before any code.
