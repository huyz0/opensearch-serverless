# `_rank_eval`

## Serves

Parity plan R10: the stock search-adjacent REST APIs. `GET|POST /{index}/_rank_eval` scores how well searches rank documents a person rated, as stock OpenSearch's `RestRankEvalAction` and `TransportRankEvalAction` (modules/rank-eval) do.

## Change

`oss-reader` gains `rank_eval`: the body parser (`RankEvalSpec`, rated requests, ratings, templates), the `precision`, `recall` and `mean_reciprocal_rank` metrics, and the order OpenSearch writes the response maps in. `oss-node` (feature `reader`) gains `search_rank_eval` with the routes and `SearchService::handle_rank_eval`.

Each rated request is a search with `size` forced to the metric's `k` and no `_source`, from its inline `request` or from a `template_id` rendered with `params` over the body's `templates` (the mustache port of [search-template-api](../search-template-api/SPEC.md)). It runs one at a time through `search_bytes`, the path `_msearch` uses, so a sharded index goes through the coordinator. The answer is `{"metric_score":..,"details":{id:{metric_score,unrated_docs,hits,metric_details}},"failures":{id:{"error":..}}}`: the score is the mean of the requests' scores, `details` is written in the order of a Java `HashMap` sized for the requests and `failures` in that of a `ConcurrentHashMap` (both checked against the JDK by `fixtures/rank_eval/`). A search that fails goes under `failures`; with none evaluated the score is `"NaN"`, as Jackson writes it. A body or template that is invalid, or a rated request asking for aggregations, `explain` or a highlighter, fails the whole request with 400.

Not here, refused with 400 "... is not supported by this node yet": `summary_fields`, a rated request with a `sort`, `_rank_eval` without an index or over a list, pattern or `_all`, and every URL parameter (`search_type` and the indices options).

## Acceptance criteria

- AC-1: `precision`, `recall` and `mean_reciprocal_rank` score a rated search as stock does, with their details and defaults, and the response score is the mean over requests.
- AC-2: a request given by `template_id` and `params` renders then ranks like the same inline request.
- AC-3: a two-shard index ranks through the coordinator.
- AC-4: a search that fails (an unknown index) is reported under `failures` without a `status`, and the score is `"NaN"` when nothing was evaluated.
- AC-5: a body that is not a valid evaluation (no requests, no metric, an unknown key, `k` or the threshold out of range, an ambiguous, empty or duplicated rated request, a rated request with `explain`, a body that is not JSON) answers 400.
- AC-6: the refusals above answer 400 "... is not supported by this node yet".
- AC-7: the maps of the response are written in the JDK's iteration order.
- AC-8: a warm index adds no object-store request beyond the searches'.
- AC-9: a template given as `{"id":"<stored script>"}` is read from the stored-script set once and rendered like an inline one; a missing script answers 404 `resource_not_found_exception`, and a node with no stored-script set refuses it as not supported.
- AC-10: `dcg` (`normalize`, `unknown_doc_rating`, `k` default 10) scores a rated search as `DiscountedCumulativeGain` does: the sum of `(2^rating - 1) / log2(rank + 1)`, an unrated hit adding its `unknown_doc_rating` or nothing, `ndcg` dividing by the same sum over the rated documents best first; `metric_details` carries `dcg`, then `ideal_dcg` and `normalized_dcg` when the ideal is not 0, then `unrated_docs`.
- AC-11: `expected_reciprocal_rank` (`maximum_relevance` required, `unknown_doc_rating`, `k` default 10) scores as `ExpectedReciprocalRank` does: each hit with a rating adds `p * prob / rank` and the chance `p` falls by `1 - prob`, where `prob = (2^rating - 1) / 2^maximum_relevance`; a hit with no rating counts as `unknown_doc_rating` or is skipped; `metric_details` carries `unrated_docs`. A missing `maximum_relevance` is a 400 (the Java names its parser `dcg`, so its errors do too).

## Test plan

Unit: `crates/reader/src/rank_eval/` (metrics, parser, and the map orders against `fixtures/rank_eval/map_order.txt`). Integration: `crates/node/tests/it/rank_eval.rs` through HTTP over the compat blobs.

## S3 request budget

None of its own: each rated request costs what a `_search` of that index costs. LIST 0.

## Switch

None: a new endpoint, refusing what it does not support.

## Risks

- The response was ported from the Java source and not recorded from a running OpenSearch; only the map orders were run on the JDK.
- A rated request whose `template_id` names no template answers 400 here; stock fails with a null pointer (500).
- Two rated requests with one id put one entry in stock's map and two here.
- Requests run one after another, so `max_concurrent_searches` is read and ignored.
