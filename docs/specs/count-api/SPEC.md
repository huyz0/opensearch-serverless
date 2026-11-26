# `_count`

## Serves

Parity plan R10: the stock search-adjacent REST APIs. `GET|POST /{index}/_count` answers how many documents match a query, as stock OpenSearch's `RestCountAction` does for the parameters in scope.

## Change

`oss-node` (feature `reader`) gains `count_route`: the two routes of `/{index}/_count`, registered by one line each in `search::routes`, and `SearchService::handle_count`. The request body is `{"query": ...}` or empty; it runs as the search of that query with `size` 0 and an exact total, on a one-shard index through the shard search and on a sharded one through the coordinator's merge, and answers `{"count": N, "_shards": {...}}` with the search's own `_shards` object. A failed search answers its own error body.

Not here: `/_count` without an index (as `/_search` without one), the URL parameters `q`, `df`, `analyzer`, `terminate_after`, `routing`, `preference`, `ignore_unavailable` and the rest (refused as unknown parameters), and a body key other than `query` (refused).

## Acceptance criteria

- AC-1: `POST /{index}/_count` with a query answers `count` equal to the `hits.total.value` of the same query searched with exact tracking, and `_shards` equal to that search's, over an index of one shard.
- AC-2: the same over an index of two shards.
- AC-3: no body, `{}` and `{"query":{"match_all":{}}}` count every document; `GET` answers as `POST` does.
- AC-4: an unknown index answers the 404 `index_not_found_exception` a search does.
- AC-5: a query the search refuses (an unknown query type) answers the same status and error type as the search.
- AC-6: a body key other than `query` (`size`, `sort`, `aggs`, `from`) answers 400 `illegal_argument_exception` saying a `_count` body takes only `query`; an unknown URL parameter answers the router's 400.
- AC-7: a count of a warm open index makes no object-store request.
- AC-8: the URL parameter `min_score` counts only matches scoring at least it, over one shard and over two; `-1` means no minimum, as `RestCountAction` reads it; a value that is not a finite float answers 400 `illegal_argument_exception`.

## Test plan

`crates/node/tests/it/count.rs` over the compat blobs (`compat-term`, and its two-shard variant as `it/coordinate.rs` builds it), through HTTP. AC-7 reads the meter, as `it/search.rs` does.

## S3 request budget

The search's: a cold open is the descriptor and the commit and one merged ranged read; a warm open index costs nothing. LIST 0.

## Switch

None: a new endpoint, refusing what it does not support.

## Risks

- OpenSearch answers an unknown body key with `parsing_exception` ("unknown key"); this node answers `illegal_argument_exception` "not supported by this node yet" like its other refusals. Listed under compatibility.
- The count is the search's exact total, so a huge match set costs what an exact `track_total_hits` costs; stock `_count` does the same.
