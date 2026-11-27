# The coordinator fetches and renders the hits of a merged top_hits

## Serves

Parity plan R7.2, the piece `coordinator-top-hits-reduce` left: the merged page names its hits as a shard and a position; this fetches each from its shard and writes the aggregation's `"hits":{..}` body as a single shard writes it.

## Change

- `TopHitsResult` gains `before_explanations`: the explanations of the hits ahead of `from`, asked only with `explain`, so a hit the merge pulls from there is explained like any other.
- `oss_reader::response::render_selected_hits` takes a shard's `TopHitsResult` and positions (counting `before` then `hits`) and returns each selected hit's JSON object, one entry per position in the order given, written exactly as the shard's own response writes it (its `_source` filter, `docvalue_fields`, sort values and explanation; a highlight plan is carried the same way and is covered by the shard's own tests). One prefetch covers all of them, so the shard's stored fields arrive in one round.
- `oss-coordinator` gains `reduce::top_hits::render_page`: it groups the page's hits by shard, asks each shard once (concurrently, bounded) through a caller-supplied `fetch`, and writes `"hits":{"total":{"value":N,"relation":"eq"},"max_score":..,"hits":[..]}` with the hits in page order. A shard's failure, or an answer of the wrong length, fails the render.

Sending the call to a shard (transport) is the caller's: `fetch` is a closure so the render is tested without a node.

## Acceptance criteria

- AC-1: a shard's result explains the hits ahead of `from` in `before_explanations` when `explain` is asked, and holds none otherwise.
- AC-2: `render_selected_hits` writes, for positions across `before` and `hits`, exactly what the shard's response writes for those hits in a search whose window holds them.
- AC-3: selected hits carry `explain`, `sort`, `docvalue_fields` and `_source` filtering as the response does.
- AC-4: a selection of no positions, or of a position beyond the result, is refused.
- AC-5: `render_page` writes the page's hits in page order with the merged total and best score, a shard's hits being asked in one call.
- AC-6: a failing or panicking shard, or an answer of the wrong length, fails the render with the shard named.
- AC-7: an empty page writes an empty `hits` list with its total and `null` best score when none.

## Test plan

`crates/reader/src/response_selected_tests.rs` for AC-1 to AC-4 over a real shard; `crates/coordinator/tests/it/top_hits_render.rs` for AC-5 to AC-7 with a fake `fetch`.

## S3 request budget

Zero added on the coordinator; each shard prefetches its selected hits' stored fields in one round, which a response that writes them would do anyway.

## Bounds

At most 100 hits a shard (the inner window); rendered hits are held until the body is written.

## Switch

None: no measured downside.

## Risks

The page's rendered bytes are not charged to a memory budget here; that is the piece that holds the node's budget.
