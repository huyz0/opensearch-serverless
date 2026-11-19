# A node coordinates a search over its own shards

## Serves

Parity plan R7.6, node assembly: `SearchService::coordinate` runs one search over shards `0..n` of an index that this node's readers serve, through `oss-coordinator::search`. Resolving the index's shard count, the REST route and forwarding to other nodes are separate pieces.

## Change

`oss-node`, with the `reader` and `coordinator` features, gains `SearchService::coordinate` and `ShardError`.

## Acceptance criteria

- AC-1: each shard is asked for its top `from + size` hits from zero, and the merged response applies `from` once: a two-shard index answers the total of both, and the hits drawn from both, each written by the shard that ranked it.
- AC-2: the sort keys' directions order the merge, the score descending when there is no sort.
- AC-3: a shard number the index does not have fails alone: the response counts it in `_shards.failed`, lists it with the shard's error, and still carries the other shards' hits.
- AC-4: the buffers a hit's fields are read into are granted by the `reserve` the caller passes; a refusal fails the search, naming the shard.
- AC-5: `track_total_hits` false leaves the total out, and a bound below the count writes the bound with relation `gte`.

## Test plan

`crates/node/tests/it/coordinate.rs` over the compat index written as two shards (shard 1 a copy of shard 0), bodies asserted.

## S3 request budget

Each shard opens once as for any search (the descriptor and the commit); the merge and the render read nothing more.

## Route

- AC-6: with the `coordinator` feature, `_search` on an index of more than one shard is answered through `coordinate`, the shard count read from shard 0's descriptor as it opens (no request beyond opening each shard once), and a warm search reads nothing from the store; an index of one shard is answered as before.
