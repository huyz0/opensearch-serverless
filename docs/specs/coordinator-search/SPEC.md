# The coordinator runs one search over its shards

## Serves

Parity plan R7.6, node assembly: the glue that takes a search over an index's shards, asks each shard for its hits, merges them, fetches the page's hits from the shards that ranked them, reduces the metric aggregations and writes the response. The shards are a port (`Shards`): in one process they are the node's readers, and a later piece forwards to another node. A `top_hits` aggregation and bucket aggregations are separate pieces; a search asking for one is refused by the aggregation reduce.

## Change

`oss-coordinator` gains `search`: the `Shards` port, `Params`, `SearchFailure` and `search`.

## Acceptance criteria

- AC-1: the page is the merged order over all shards (by score, or by the request's sort), starting at `from` and holding at most `size`, and each hit is the body its own shard rendered, written in that order.
- AC-2: each shard is asked once to run its query and, for the page's hits, once to render them, naming its hits' positions in the order the page needs them; a shard the page does not draw on is not asked to render.
- AC-3: the response is `{"took":T,"timed_out":false,"_shards":{"total":N,"successful":N,"skipped":0,"failed":0},"hits":{"total":{"value":V,"relation":"eq"},"max_score":M,"hits":[...]}}`, `max_score` written as Java writes a float and `null` when no shard had one; the relation is `gte` when the totals are capped.
- AC-4: when the request does not track the total, `total` is not written.
- AC-5: a shard whose query fails is counted in `_shards.failed` and listed in `_shards.failures` with its number, its index and its error, and the hits are merged from the shards that answered; the response still succeeds.
- AC-6: when every shard fails the search fails with the first shard's error, and nothing is written.
- AC-7: a shard that fails to render the page's hits fails the search, naming the shard.
- AC-8: the metric aggregations of the shards that answered are reduced and written after the hits as `"aggregations":{...}`; a search without any writes none.

## Test plan

`crates/coordinator/tests/it/search.rs` over a fake `Shards` holding canned hits: one test per criterion, bodies asserted exactly.

## S3 request budget

Zero: the port's callers read; this module reads nothing.
