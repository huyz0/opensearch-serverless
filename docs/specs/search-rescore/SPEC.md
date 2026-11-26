# `_search` `rescore`

## Serves

`rescore` on the refusal list of `docs/compatibility.md`. Stock runs a second query over the top `window_size` hits of the first and combines the two scores (`QueryRescorerBuilder`, `RescoreProcessor`). The engine has the arithmetic (`lucene_search::rescorer::opensearch_query_rescore`); the reader refused the key.

## Change

- `rescore` parses as one object or an array of objects: `{"window_size": n, "query": {"rescore_query": <query>, "query_weight": f, "rescore_query_weight": f, "score_mode": "total|multiply|avg|max|min"}}`. Defaults are stock's: window 10, both weights 1, `total`.
- The first pass collects `max(from + size, every window)` hits, each rescorer runs in order over the whole list, and `from` is dropped after, as `TopDocsCollectorContext` and `RescoreProcessor` do. The shard's best score is the first rescored hit's.
- Hits past a window are scaled by `query_weight`, and the list is re-sorted when there are any, as stock.
- The rescore query's own fields' norms are read for it; a rescore query needing points opens them.
- Refused as stock does: a `sort` with `rescore` ("Cannot use [sort] option in conjunction with [rescore]."), a window above `index.max_result_window` (the default of `index.max_rescore_window`, which this node does not take as a setting), a missing `rescore_query` ("rescore_query cannot be null"), a `score_mode` stock does not know ("illegal score_mode [x]"), a rescorer of another type, an unknown key. Refused as not served: a `window_size` below 1.
- No new setting: only a new accepted shape, with no measured downside.

## Acceptance criteria

- AC-1: with the default modes a hit's score is its first-pass score plus the rescore query's, and an unmatched hit keeps its first-pass score.
- AC-2: `query_weight`, `rescore_query_weight` and each `score_mode` combine as `QueryRescoreMode` does.
- AC-3: the window bounds what is rescored: a hit outside it is scaled by `query_weight` alone, and the rescored order holds across `from`.
- AC-4: an array of rescorers runs each over the previous one's result.
- AC-5: every refusal above answers 400 with the stated reason.
- AC-6: a rescore adds no object-store request beyond the rescore query's own field reads on a warm shard.

## Tests

Unit: `crates/reader/src/dsl/rescore_tests.rs`. Integration: `crates/node/tests/it/rescore.rs`.

## S3 request budget

None added: the rescore query reads the same blobs a query of its own would.
