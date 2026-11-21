# Reader: min_score

## Serves

Parity task R10.4 (hit features); invariant I-13. The search body's `min_score` was refused as not supported yet. lucene-rust already holds the collectors behind it (`MinScoreCollector`, `search_sorted_min_score`, `search_boolean_query_multi_segment_min_score`); this wires them.

## Change

`oss-reader` parses `min_score` as `SearchSourceBuilder.parseXContent` does (`parser.floatValue()`: a number or a string holding one; a null or any other value is an error) and runs the shard's collectors behind OpenSearch's `MinimumScoreCollector`: only the matches scoring at least `min_score` are hits, are counted in `hits.total`, and set `max_score`.

- A search by score, a search with a field sort and a `size: 0` count each apply it.
- A sort that does not use the score still scores the matches to filter them; its hits' `_score` stays null.
- Aggregations beside `min_score` are refused (400, "not supported by this node yet"): OpenSearch's collector wraps the aggregations' collectors too, and this node's do not run behind it. So are `top_hits` windows, which do not take it.

## Acceptance criteria

- AC-1: for each score a plain search returns, `min_score` of that score keeps exactly the matches scoring at least it, in the plain order, and `hits.total` equals the kept count.
- AC-2: a floor above every score keeps none: total 0, `max_score` null, no hits.
- AC-3: the same floor on a field sort and on `size: 0` counts what the score-sorted search counts.
- AC-4: a string holding a number is accepted; a non-number, a null, and `min_score` beside aggregations are 400.

## Test plan

`crates/node/tests/it/search_hits.rs`: `min_score_keeps_and_counts_only_the_matches_scoring_that_much`, a differential check against the same search without the key, over the recorded `compat-term` index. Seen failing with the key ignored (the total stayed 3). The error bodies of a bad value are not recorded from OpenSearch, so they are provisional.

## S3 request budget

Unchanged: the same segments and doc values are read; the floor only filters collected matches. LIST 0, HEAD 0, PUT 0.

## Risks

- A document scoring exactly the floor is kept (`>=`), as Lucene's collector keeps it; the fixture's boundary case is the plain scores themselves.
