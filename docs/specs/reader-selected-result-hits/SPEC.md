# The reader writes the selected hits of a shard's top-level result

## Serves

Parity plan R7.6, the coordinator's search path: after the merge of several shards' hits keeps a page, each kept hit is fetched from its shard and written as that shard's own response writes it. `render_selected_hits` does this for a `top_hits` result; this is the same for a search's top-level hits.

## Change

`oss_reader::response::render_selected_result_hits(shard, index, node_id, hits, positions, reserve)` returns each selected hit's JSON object, one entry per position in the order given, written exactly as `write_response` writes it (its `_source` filter, `docvalue_fields`, sort values, explanation and highlight plan). One prefetch covers all of them. `render_selected_hits` and it share one writer.

## Acceptance criteria

- AC-1: for positions across a result, each object equals the one the shard's response writes for that hit, the whole result joined by commas equals the response's hit list, and a selection follows the order asked and may repeat.
- AC-2: `explain`, sort values, `docvalue_fields` and `_source` filtering are written as the response writes them.
- AC-3: a selection of no positions, or of a position beyond the result, is refused.

## Test plan

`crates/reader/src/response_selected_tests.rs` over a real shard, each test comparing against the shard's own response. Verified red by planting a fault (every position read as at most 1) and green after restoring.

## S3 request budget

The stored-field reads of the selected hits, announced in one prefetch, as the response's own; none for a hit already read.

## Bounds

One buffer a selected position, the hit's own bytes, granted by `reserve`; positions are at most the result window.

## Switch

None: no setting, no measured downside; nothing calls it yet.

## Risks

None known.
