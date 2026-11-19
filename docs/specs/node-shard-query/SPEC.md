# A node runs one shard's query for a coordinator in the same process

## Serves

Parity plan R7.6, the coordinator's search path in one process: the per-shard query a coordinator fans out to, and the fetch of the hits its merge kept. A search transport action stays deferred (ADR 0016), so the coordinator calls the local reader.

## Change

- `oss-node`: `SearchService::query_shard(index, shard, source)` opens the shard if it is not open and runs `source` on it (with its overlay when one is ready), returning a `ShardQuery` holding the shard's `Hits`; `ShardQuery::render(node_id, positions, reserve)` writes the selected hits as the shard's own response writes them. A failed open or query is a `SearchError` naming the index, shard and node.
- `oss-reader`: `OpenShard` carries its shard number, so an explained hit names `[index][n]` instead of always `[index][0]`.

## Acceptance criteria

- AC-1: a shard query finds what the HTTP search of the same index finds (the total and the hits), and the rendered hits of all positions joined by commas equal the response's hit list; a selection follows the order asked.
- AC-2: shard 1 of a two-shard index is queried, and its explained hits are named `[two][1]` while shard 0's are `[two][0]`.
- AC-3: a query the shard refuses (a result window too large) is 400 naming the shard `1` and the index; a missing index is 404; a shard the index does not have is refused.

## Test plan

`crates/node/tests/it/shards.rs` over a compat index and a two-shard variant; AC-2's shard name was verified red by planting the number 0 in the writer and green after restoring it.

## S3 request budget

The shard's open (two GETs, once) and the reads its query and selected hits make, as a search of the shard makes today; no LIST.

## Bounds

The shard's hits are at most the request's window; a `ShardQuery` holds the open shard, which the `open_indexes` pool already bounds.

## Switch

None: no setting, no measured downside; the HTTP search is unchanged.

## Risks

A `ShardQuery` keeps its shard open for as long as it lives, so a coordinator must drop it after rendering; the pool cannot close a shard a query holds.
