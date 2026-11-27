# The coordinator merges a top_hits across shards

## Serves

Parity plan R7.2, search fan-out and reduce, the `top_hits` piece `coordinator-agg-reduce` left refused: `InternalTopHits.reduce`, which merges every shard's top `from + size` documents of the aggregation's own sort, applies `from` once over the merge and reports the summed total and the best score.

## Change

- `oss_reader::aggs::metric::top_hits::TopHitsResult` gains `before`: the hits ahead of `from` that the shard dropped from `hits`. They are never rendered from the shard's own result; the merge needs them because a document a shard ranks inside its first `from` can rank inside the merged page.
- `oss-coordinator` gains `reduce::top_hits::reduce_top_hits`, over the shards' results, the aggregation's `from` and `size`, and its sort keys' direction and missing rule. It gives the summed exact total, the best score, and the merged page as a list of `(shard, position)`, where a position counts each shard's `before` then `hits`. It uses `reduce::hits::Merger`, so the ordering, the tie rule and the incompatible-sort-values error are the search's own.
- `reduce::aggs::reduce_shards` still refuses a `TopHits`; the caller reduces `top_hits` with `reduce_top_hits`, which needs the request's `from`, `size` and sort that the value does not carry.

Fetching the page's hits from their shards, and rendering them, is the piece that fetches hits.

## Acceptance criteria

- AC-1: a shard's result keeps the hits it dropped ahead of `from` in `before`, in order, and its `hits` are unchanged.
- AC-2: the merged page is the best `size` hits of all shards' `before` and `hits` after skipping `from`, in score order for a score sort.
- AC-3: with a field sort, the page follows the sort's direction and missing rule; a tie goes to the lower shard.
- AC-4: the total is the sum of the shards' totals, and the best score the highest of theirs, `None` when no shard has one.
- AC-5: sort values of different kinds across shards are refused with `MergeError::IncompatibleSortValues`.
- AC-6: no shards, or shards without hits, give an empty page with total zero.
- AC-7: the merge holds at most twice `from + size` hits plus one shard's answer, as the search's merge.

## Test plan

`crates/reader/src/aggs/metric/top_hits_tests.rs` for AC-1 on a real shard; `crates/coordinator/tests/it/top_hits_reduce.rs` for the rest over hand-built shard results.

## S3 request budget

Zero: the reduce reads nothing.

## Bounds

At most 100 hits a shard (the inner result window), and the pile at most twice `from + size`.

## Switch

None: the shell's reduce has no setting and no measured downside.

## Risks

`before` keeps up to `from` more hit records a shard result than before, within the 100-hit window the shard already reserves.
