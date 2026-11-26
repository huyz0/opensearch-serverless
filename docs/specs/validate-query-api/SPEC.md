# `_validate/query`

## Serves

Parity plan R10: the stock search-adjacent REST APIs. `GET|POST /{index}/_validate/query` answers whether a query is valid for an index, as stock OpenSearch's `RestValidateQueryAction` and `TransportValidateQueryAction` do when no explanation is asked for.

## Change

`oss-node` (feature `reader`) gains `search_validate`: the two routes of `/{index}/_validate/query` and `SearchService::handle_validate`. The body is `{"query": ...}` or empty. The query is parsed and built as a Lucene query over the mapping of shard 0 of the index, as a search builds it, with no search run. The answer is `{"_shards":{"total":1,"successful":1,"failed":0},"valid":B}`: stock routes a validation to one random shard of each index, so the total is 1 whatever the shard count, and every shard of an index shares its mapping.

A body that fails to parse (not JSON, an unknown query type, an unknown key, a key other than `query`) and a query that fails to build (such as a mapping conflict) answer 200 with `valid: false`, as stock does; the error text appears only with `explain`, which is refused.

Not here: `/_validate/query` without an index, an index that is a list, pattern or alias, and the URL parameters `explain`, `rewrite`, `all_shards` and the indices options, all refused. The `q` family (`q`, `df`, `default_operator`, `analyzer`, `analyze_wildcard`, `lenient`) is served as `_count` serves it, below. A query this node does not support yet answers the 400 refusal a search gives, never `valid: false`.

## Acceptance criteria

- AC-1: a query the search accepts answers 200 `{"_shards":{"total":1,"successful":1,"failed":0},"valid":true}`, over an index of one shard and of two.
- AC-2: no body, `{}` and `{"query":{"match_all":{}}}` are valid; `GET` answers as `POST` does.
- AC-3: an unknown query type, an unknown body key, a body key other than `query` and a body that is not JSON answer 200 with `valid: false` and no other key than `_shards`.
- AC-4: a query that fails to build on the mapping (a `prefix` on an integer field) answers `valid: false`; a query the node does not support yet answers 400 `illegal_argument_exception` "... is not supported by this node yet".
- AC-5: an unknown index answers the 404 `index_not_found_exception` a search does.
- AC-6: `/_validate/query` without an index and the URL parameters `explain`, `rewrite` and `all_shards` answer 400.
- AC-8: with no body, `q` and its companions validate as the `query_string` query a search builds from them (`RestActions.urlParamsToQueryBuilder`): a good one is valid, one that does not parse is `valid: false`, a body's query wins over `q`, a bad `default_operator` or `lenient` is 400 `illegal_argument_exception`, and what `query_string` does not serve (`analyze_wildcard`) is refused as in a search.
- AC-7: validating against a warm open index makes no object-store request.

## Test plan

`crates/node/tests/it/validate_query.rs` over the compat blobs (`compat-term`, and its two-shard variant), through HTTP. AC-7 reads the meter, as `it/count.rs` does.

## S3 request budget

The open of shard 0: a cold open is the descriptor and the commit and one merged ranged read; a warm open index costs nothing. LIST 0.

## Switch

None: a new endpoint, refusing what it does not support.

## Risks

- `explain`, `rewrite` and `all_shards` need Lucene's query `toString`, which lucene-rust does not give yet; they are refused, listed under compatibility.
- The query is validated against shard 0's mapping; a mapping is shared by the shards of an index, so the answer is the one stock gives.
